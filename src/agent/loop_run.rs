use std::io::{self, IsTerminal, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::thread;

use crate::agent::prompting;
use crate::agent::recovery;
use crate::config::Config;
use crate::format_model_banner;
use crate::logging::log_llm_event;
use crate::model_registry::RuntimeModels;
use crate::modes::plan_act::ExecutionMode;
use crate::ollama::client::{AssistantReply, OllamaClient, should_use_native_tool_calls};
use crate::ollama::xml_fallback::ToolCall;
use crate::repo_graph::{
    BuildOptions as RepoGraphBuildOptions, BuildOutcome, RepoGraph, RepoGraphError,
    build_repo_graph,
};
use crate::safety::path_guard::resolve_user_path;
use crate::session::compact::{
    approximate_token_count, compact_messages, compact_messages_with_strategy,
};
use crate::session::store::{ConversationMessage, SessionSnapshot, SessionStore};
use crate::stdin_prompt;
use crate::system_prompt::build_system_prompt;
use crate::tools::registry::{ToolContext, ToolRegistry};

mod auto_test;
pub mod commands;
mod deterministic;
mod footer;
mod interrupt;
mod lifecycle;
mod protocol;
mod quality;
pub(crate) mod reminder;
pub mod slash_commands;
mod spinner;
mod summary;
mod tester;
mod turn;
pub(crate) mod verifier_skill;

// Public re-exports so `lib.rs::run_cli` can hand a `FooterHandle` into
// `Agent::new` and own the matching `FooterLease` for its scope (issue #430).
pub use footer::{FooterHandle, FooterLease};

// Re-export env helpers from `turn` so `src/tui/markdown.rs` can reuse the
// existing POSIX-compliant NO_COLOR and UTF-8 locale logic without duplicating
// it. `mod turn;` itself stays private; only these two fns leak out (issue #431).
pub(crate) use turn::{no_color_requested, unicode_supported};

// Issue #453: expose the precaution prompt selector so integration tests in
// `tests/` (and any future callers) can validate the Act-mode prompt
// selection pipeline without requiring a live Ollama call.
pub use turn::select_precautions_for_prompt;

// Issue #465 / Phase 5: expose Reminder types needed by tests/agent_skill_registry_smoke.rs
// (E2E tests live outside the crate so `pub(crate) mod reminder` cannot be reached
// directly). Production code paths continue to use `super::reminder::...`; these
// `pub use` lines only widen the visibility for integration tests.
pub use reminder::{
    FailureReason as ReminderFailureReason, ReminderInputs, ReminderOutcome,
    SkipReason as ReminderSkipReason,
};

// Issue #459 / Phase 2: expose the Tester Skill orchestrator + types so the
// E2E suite under `tests/tester_skill_smoke.rs` can drive the closure-DI
// boundary directly (LLM call + Bash runner + approver) without an Ollama /
// cargo / node / python dependency. Production code paths in `turn.rs`
// continue to call these via `super::tester::...` — these `pub use` lines
// only widen the visibility for integration tests.
pub use tester::{
    AbortReason as TesterAbortReason, ApprovalMode as TesterApprovalMode,
    MAX_GENERATED_TESTS_PER_TURN as TESTER_MAX_GENERATED_TESTS_PER_TURN,
    MAX_TESTER_LLM_REPLY_BYTES, NotInvokedReason as TesterNotInvokedReason, TesterCandidate,
    TesterLlmError, TesterOutcome, TesterPrompt, TesterRun, check_invocation_gate as tester_gate,
    run_tester_with_strategy, tester_disabled,
};

const DEFAULT_KEEP_TAIL: usize = 24;
const LATE_TURN_KEEP_TAIL: usize = 12;

pub enum AgentEvent {
    Continue(Option<String>),
    Exit,
}

pub struct Agent {
    config: Config,
    models: RuntimeModels,
    client: OllamaClient,
    session_store: SessionStore,
    session: SessionSnapshot,
    work_root: PathBuf,
    native_tools_enabled: bool,
    plan_model_override: Option<String>,
    tool_registry: ToolRegistry,
    repo_context_cache: Option<RepoContextCache>,
    /// Fixed footer handle. Phase A: always disabled (no-op); the handle
    /// shape is plumbed now so Phase B-D can attach `publish_*` calls
    /// without re-touching `Agent::new` callers (issue #430).
    #[allow(dead_code)]
    footer: FooterHandle,
    /// Per-turn cap for the Reminder Sidecar (#452). Reset at the top of every
    /// `handle_user_message`, set to `true` only when an actual sidecar call
    /// was attempted (Completed/Failed); Skipped does not consume the cap.
    reminder_called_this_turn: bool,
    /// Issue #459: per-turn cap for the Tester Skill. Reset at the top of every
    /// `handle_user_message`. Consumed only when a Tester smoke run actually
    /// dispatched (Recorded / Aborted); NotInvoked does not consume the cap.
    pub(super) tester_called_this_turn: bool,
    /// Issue #456: tracks whether `compute_anvil_score` has already run for
    /// the current turn. Reset at the top of every `handle_user_message`,
    /// flipped to `true` after the post-loop compute writes
    /// `session.last_anvil_score`. Used by `maybe_invoke_reminder` to pick
    /// between `AnvilScoreSnapshot::PreviousTurn` (iteration-internal hook,
    /// score is the previous turn's persisted value) and
    /// `AnvilScoreSnapshot::CurrentTurn` (post-loop hook, score is the value
    /// just computed for this turn).
    pub(super) anvil_score_computed_this_turn: bool,
    /// Issue #466: skill registry. ReminderSkill (#465) は trait 実装を直接呼び出す
    /// 既存経路を維持し、本 Issue では VerifierSkill を post-loop fence で invoke
    /// するために registry instance を hold する。dynamic skill loading は許可しない
    /// (DR4-003): static registration only.
    #[allow(dead_code)]
    pub(super) skill_registry: crate::agent::skills::SkillRegistry,
    /// Issue #468: session-lifetime cache of the repository structure graph.
    /// Built once at `Agent::new` (blocking) and never mutated afterward.
    /// Not serialized — RepoGraph state lives outside `SessionSnapshot` to
    /// avoid bloating the session JSON (DR1-004 / S3-006).
    #[allow(dead_code)]
    pub(super) repo_graph: Option<Arc<RepoGraph>>,
    /// Issue #471: per-turn cache of the last case retrieval summary for the
    /// eval log. Set by `try_inject_case_retrieval_message` when a Completed
    /// outcome is obtained. Reset at the top of `run_actor_loop`.
    pub(super) last_case_retrieval_summary: Option<crate::session::eval_log::CaseRetrievalSummary>,
}

#[derive(Clone)]
struct RepoContextCache {
    task: String,
    work_root: PathBuf,
    repo_graph_present: bool,
    last_feedback_kind: Option<String>,
    suspected_files_fingerprint: u64,
    touched_files_fingerprint: u64,
    message: Option<ConversationMessage>,
}

/// Issue #469 DR1-005: SSOT for path-list fingerprinting used by the
/// `RepoContextCache` key. Empty slice yields a process-stable sentinel
/// hash; non-empty path lists are sorted before hashing for stability
/// across feedback ordering.
pub(super) fn fingerprint_paths(paths: &[String]) -> u64 {
    use std::collections::hash_map::DefaultHasher;
    use std::hash::{Hash, Hasher};

    let mut sorted: Vec<&str> = paths.iter().map(|s| s.as_str()).collect();
    sorted.sort_unstable();
    let mut hasher = DefaultHasher::new();
    sorted.hash(&mut hasher);
    hasher.finish()
}

impl Agent {
    pub fn new(
        config: Config,
        models: RuntimeModels,
        client: OllamaClient,
        session_store: SessionStore,
        session: SessionSnapshot,
        footer: FooterHandle,
    ) -> Self {
        let work_root = session
            .active_root
            .clone()
            .unwrap_or_else(|| config.cwd.clone());
        let native_tools_enabled =
            should_use_native_tool_calls(&models.main) && !session.native_tools_disabled;
        let mut skill_registry = crate::agent::skills::SkillRegistry::new();
        skill_registry.register(crate::agent::loop_run::verifier_skill::VerifierSkill);
        let repo_graph = ensure_repo_graph(&work_root, session_store.state_root());
        Self {
            config,
            models,
            client,
            session_store,
            session,
            work_root,
            native_tools_enabled,
            plan_model_override: None,
            tool_registry: ToolRegistry::default(),
            repo_context_cache: None,
            footer,
            reminder_called_this_turn: false,
            tester_called_this_turn: false,
            anvil_score_computed_this_turn: false,
            skill_registry,
            repo_graph,
            last_case_retrieval_summary: None,
        }
    }
}

/// Issue #468 facade: build (or load from cache) the `RepoGraph` once at
/// session start. **All `agent.repo_graph.*` event emission lives here**
/// (DR1-005). Failures are non-fatal: the agent loop always continues.
fn ensure_repo_graph(work_root: &Path, state_root: &Path) -> Option<Arc<RepoGraph>> {
    let opts = RepoGraphBuildOptions::default();
    let outcome = build_repo_graph(work_root, state_root, &opts);
    match outcome {
        Ok(BuildOutcome::Built {
            graph,
            node_count,
            edge_count,
        }) => {
            log_llm_event(
                "agent.repo_graph.completed",
                serde_json::json!({
                    "reason": "built",
                    "node_count": node_count,
                    "edge_count": edge_count,
                    "fingerprint_id": graph_fingerprint_id(&graph),
                }),
            );
            Some(graph)
        }
        Ok(BuildOutcome::CacheHit { graph }) => {
            log_llm_event(
                "agent.repo_graph.completed",
                serde_json::json!({
                    "reason": "cache_hit",
                    "fingerprint_id": graph_fingerprint_id(&graph),
                }),
            );
            Some(graph)
        }
        Ok(BuildOutcome::Skipped { reason }) => {
            log_llm_event(
                "agent.repo_graph.skipped",
                serde_json::json!({"reason": reason}),
            );
            None
        }
        Err(RepoGraphError::Disabled) => {
            log_llm_event(
                "agent.repo_graph.disabled",
                serde_json::json!({"reason": "env_no_repo_graph"}),
            );
            None
        }
        Err(RepoGraphError::CwdCanonicalFailed) => {
            log_llm_event(
                "agent.repo_graph.failed",
                serde_json::json!({"reason": "cwd_canonical_failed"}),
            );
            None
        }
        Err(RepoGraphError::PersistFailed(message)) => {
            log_llm_event(
                "agent.repo_graph.failed",
                serde_json::json!({
                    "reason": "persist_failed",
                    "message": message,
                }),
            );
            None
        }
    }
}

fn graph_fingerprint_id(_graph: &Arc<RepoGraph>) -> String {
    // RepoGraph keeps the fingerprint internal; we expose a short id surface
    // here through the public node/edge counts already logged above. The id
    // itself is currently not part of the public API to keep the surface
    // minimal — a tracking entry to expose it lives with #469's seam work.
    String::new()
}

#[cfg(test)]
mod tests {
    use super::lifecycle::{format_tool_error, should_compact_late_turn};
    use crate::agent::prompting::{
        compact_tool_result, detect_created_project_root, should_skip_system_note,
    };
    use crate::session::store::ConversationMessage;
    use std::path::PathBuf;

    #[test]
    fn detects_created_next_app_root_from_success_line() {
        let output = "Success! Created sample-app at /tmp/work/sample-app";
        assert_eq!(
            detect_created_project_root(output),
            Some(PathBuf::from("/tmp/work/sample-app"))
        );
    }

    #[test]
    fn detects_created_next_app_root_from_create_line() {
        let output = "Creating a new Next.js app in /tmp/work/sample-app.";
        assert_eq!(
            detect_created_project_root(output),
            Some(PathBuf::from("/tmp/work/sample-app"))
        );
    }

    #[test]
    fn tool_errors_are_formatted_for_model_recovery() {
        assert_eq!(
            format_tool_error("failed to stat /tmp/missing: nope"),
            "Error: failed to stat /tmp/missing: nope"
        );
        assert_eq!(
            format_tool_error("Error: file not found"),
            "Error: file not found"
        );
    }

    #[test]
    fn read_results_are_compacted_for_transcript() {
        let long = "a".repeat(20_000);
        let compacted = compact_tool_result("Read", long);
        assert!(compacted.contains("[truncated"));
        assert!(compacted.len() < 13_000);
    }

    #[test]
    fn skips_duplicate_consecutive_system_note() {
        let messages = vec![ConversationMessage::system("same note".to_string())];
        assert!(should_skip_system_note(&messages, "same note"));
        assert!(!should_skip_system_note(&messages, "other note"));
    }

    #[test]
    fn skips_duplicate_system_note_with_tool_results_between() {
        let messages = vec![
            ConversationMessage::system("same note".to_string()),
            ConversationMessage::assistant(String::new(), Vec::new()),
            ConversationMessage::tool("Read".to_string(), "file contents".to_string()),
        ];
        assert!(should_skip_system_note(&messages, "same note"));
    }

    #[test]
    fn late_turn_compaction_triggers_for_edit_heavy_turns() {
        let messages = (0..18)
            .map(|index| ConversationMessage::user(format!("message {index}")))
            .collect::<Vec<_>>();
        assert!(should_compact_late_turn(&messages, 24_000, 4, 1));
        assert!(!should_compact_late_turn(&messages[..8], 24_000, 1, 0));
    }
}
