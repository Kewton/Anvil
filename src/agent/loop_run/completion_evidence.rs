//! Issue #606: post-hoc completion-evidence observation.
//!
//! `success_issue_with_context` historically rejected verifier-only turns
//! (e.g. a Bash turn that ran `cargo test` with exit 0 but produced no
//! repository edits) with per-protocol reject strings such as
//! `"code protocol requires at least one repository edit"`. This module
//! provides the pure data model the agent layer pushes evidence into
//! during a turn (`Bash` hook for `VerifierExitZero`, `Edit/Write` hook
//! for `RepoEdit`), and `success.rs` consumes via
//! `ProtocolKind::evidence_set_satisfies` to short-circuit the reject when
//! observed evidence already satisfies the active protocol (OR semantics).
//!
//! ## Layer rules (CLAUDE.md DR3-002)
//!
//! This module holds pure types, the SSOT path classifier, the DR4-002
//! verifier shell-control gate, and the DR4-003 verifier-command redactor.
//! It **must not** import from `agent::loop_run::Agent` / `photon` (those
//! layers consume it via `pub(crate)` re-export from `loop_run.rs`; DR3-001
//! forbids re-export from `mod.rs`-equivalent facades, so the parent module
//! pulls in only the items it actually wires up).
//!
//! The single allowed cross-module dependency is
//! `crate::session::feedback::mask_secrets`, used by
//! `redact_verifier_command_for_storage` as the upstream SSOT for token /
//! kv / URL-userinfo redaction — the agent → session direction is permitted
//! by CLAUDE.md DR3-002 (the forbidden direction is photon importing
//! session / agent).
//!
//! ## DR1-001 ordering invariant
//!
//! `classify_repo_edit_path` evaluates suffix predicates in a **fixed**
//! sequence:
//!   1. `is_test_file` (covers `__tests__/` / `.test.` / `.spec.` /
//!      `test_*.py` / `*_test.py`)
//!   2. `is_setup_file` (covers `package.json` / `tsconfig.json` / lock files)
//!   3. Docs extension (`.md` / `.mdx` / `.txt` / `.rst`)
//!   4. `is_implementation_file` (covers `.rs` / `.py` / `.ts` / `.tsx` / ...)
//!   5. fallback `Other`
//!
//! Docs comes **before** `is_implementation_file` because `.mdx` is in the
//! impl SSOT (`util::file_classify::is_implementation_file`) — DR1-001 pins
//! that a `README.mdx` returns `Docs`, not `Impl`.
//!
//! ## DR1-004 / #D-07: `UserExplicitDone` is intentionally omitted
//!
//! The original Issue #606 design considered a `UserExplicitDone` variant for
//! "the user said 'done' / 'thanks'", but it was dropped during multi-stage
//! design review (YAGNI: there is no proven turn-completion path that needs
//! it today, and adding the variant pre-commits to a fragile NLP gate).

use std::path::Path;

use crate::tools::bash::BashCommandClass;
use crate::util::file_classify::{is_implementation_file, is_setup_file, is_test_file};

/// Repository edit category emitted by `Edit` / `Write` tool calls. Mirrors
/// the SSOT classifiers in `util::file_classify` but distinguishes Docs from
/// Impl so the protocol receivers can require the appropriate flavour.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum RepoEditCategory {
    Impl,
    Test,
    Docs,
    Setup,
    Data,
    Other,
}

/// Post-hoc observation of one completion-relevant tool invocation in a
/// turn. The agent layer pushes one variant per observed signal — multiple
/// edits in the same turn are recorded as multiple `RepoEdit` entries so the
/// downstream OR-satisfaction can count per category.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub(crate) enum CompletionEvidence {
    /// A successful repository edit (Write / Edit tool call). `count` is
    /// always 1 today but the field is preserved for future aggregation.
    RepoEdit {
        category: RepoEditCategory,
        count: usize,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        path: Option<String>,
    },
    /// A Bash command classified as `BuildTest` that exited with code 0.
    /// `command` is masked through `redact_verifier_command_for_storage`
    /// before persistence (T-2.x); for α-1 logging the raw normalized
    /// command is acceptable inside the in-process `EvidenceSet` because we
    /// never serialize the set itself in α-1.
    ///
    /// ## Issue #651 PR-001: `bound_test_artifacts_count` field
    ///
    /// `Some(n)` means the verifier was executed via the **structured**
    /// `AutoTestRunner::run_structured` path with `n` owned test artifacts
    /// bound to its `Command::new(runner).args(args)` invocation. The
    /// args list is validated against `TaskWorkspaceScope` /
    /// `validate_bound_test_artifacts_for_execution` before spawn, so the
    /// verifier's input is structurally tied to the current task's owned
    /// test paths.
    ///
    /// `None` (default) means the evidence came from the **legacy /
    /// manual** path — typically a Bash `cargo test` outcome promoted to
    /// `VerifierExitZero` by `build_verifier_exit_zero_evidence`, or the
    /// shell-based `AutoTestRunner::run` legacy path. There is no
    /// type-level proof the runner's argv contained any owned test path,
    /// so `TaskContract::evaluate_with_owned_test_artifacts` refuses to
    /// promote it to `Done` under `test_execution_required = true`.
    ///
    /// `#[serde(default)]` keeps existing `session.json` snapshots
    /// readable: a persisted record that did not carry the field decodes
    /// to `None` (treated as unbound), which is the conservative choice.
    VerifierExitZero {
        class: BashCommandClass,
        command: String,
        #[serde(default)]
        bound_test_artifacts_count: Option<usize>,
    },
    /// A non-shell task-kind verifier proved that a documentation artifact
    /// contains the required section surface. This is intentionally not a
    /// Bash verifier: docs completion should not need a fake command exit to
    /// produce completion evidence.
    RequiredSectionsPass {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        path: Option<String>,
    },
    /// A non-shell task-kind verifier proved that a structured data artifact
    /// is present and satisfies its required schema surface. This is used for
    /// CSV / TSV / JSON / JSONL style data-output tasks that should not need
    /// a coding build/test command to complete.
    StructuredDataPass {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        path: Option<String>,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        columns: Vec<String>,
    },
    /// A non-shell deliverable checker proved that a report-like artifact is
    /// complete enough for the active obligation. This is intentionally
    /// generic so docs/research/ops producers can emit completion evidence
    /// without pretending they ran a coding verifier.
    ReportCompletenessPass {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        path: Option<String>,
    },
    /// The model produced an answer-only reply (no tool calls). Reserved
    /// for AnswerOnly protocol acceptance.
    AnswerOnly,
}

/// Push-only accumulator of evidence observations within one turn. The
/// owning `Agent` resets it at the top of `run_actor_loop` (T-1.8) so the
/// per-turn semantics never leak across turns.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct EvidenceSet {
    items: Vec<CompletionEvidence>,
}

impl EvidenceSet {
    pub(crate) fn new() -> Self {
        Self::default()
    }

    pub(crate) fn clear(&mut self) {
        self.items.clear();
    }

    pub(crate) fn push(&mut self, evidence: CompletionEvidence) {
        self.items.push(evidence);
    }

    pub(crate) fn iter(&self) -> impl Iterator<Item = &CompletionEvidence> {
        self.items.iter()
    }

    pub(crate) fn len(&self) -> usize {
        self.items.len()
    }

    #[allow(dead_code)] // Reserved for future α-2 photon outcome evaluation.
    pub(crate) fn is_empty(&self) -> bool {
        self.items.is_empty()
    }
}

/// Classify a repository-edit path into a `RepoEditCategory`. Evaluation
/// order is fixed per DR1-001 — see module-level doc for why Docs comes
/// before Impl (`.mdx` would otherwise be claimed by Impl).
pub(crate) fn classify_repo_edit_path<P: AsRef<Path>>(path: P) -> RepoEditCategory {
    let path = path.as_ref();
    if is_test_file(path) {
        return RepoEditCategory::Test;
    }
    if is_setup_file(path) {
        return RepoEditCategory::Setup;
    }
    if has_docs_extension(path) {
        return RepoEditCategory::Docs;
    }
    if has_structured_data_extension(path) {
        return RepoEditCategory::Data;
    }
    if is_implementation_file(path) {
        return RepoEditCategory::Impl;
    }
    RepoEditCategory::Other
}

/// Returns true only when a repository edit tool completed but left the file
/// content hash unchanged. Missing hashes are treated conservatively as real
/// edits because they represent first observation, creation, or deletion.
pub(crate) fn is_repo_edit_no_op(pre_tool_hash: Option<&str>, current_hash: Option<&str>) -> bool {
    matches!((pre_tool_hash, current_hash), (Some(p), Some(c)) if p == c)
}

fn has_docs_extension(path: &Path) -> bool {
    matches!(
        path.extension().and_then(|ext| ext.to_str()),
        Some("md" | "mdx" | "txt" | "rst")
    )
}

fn has_structured_data_extension(path: &Path) -> bool {
    matches!(
        path.extension().and_then(|ext| ext.to_str()),
        Some("csv" | "tsv" | "json" | "jsonl" | "ndjson" | "parquet")
    )
}

/// Issue #606 T-3.1 (Phase 3 Security pulled forward): refuse to treat a
/// Bash command as VerifierExitZero evidence when it contains shell control
/// operators that can mask a non-zero internal exit (DR4-002). Examples:
///
/// * `cargo test || true` — internal exit is laundered to 0
/// * `npm test ; echo ok` — second command's exit is what `$?` reports
/// * `pytest && false` — explicit follow-on failure that still exits the
///   pipe with 0 if the first command crashed and `pipefail` is unset
///
/// We accept only commands whose shell text does not contain any of the
/// blocked operators. The full normalized command string is checked
/// (newlines included so a multi-line heredoc can't hide an `|| true`).
pub(crate) fn is_completion_verifier_command(command: &str) -> bool {
    !contains_evidence_poisoning_shell_control(command)
}

/// Returns true when `command` contains a shell control character that could
/// mask the real exit code of the inner verifier invocation, or otherwise
/// modify what is actually executed (command substitution, escapes). Pure /
/// stable — exposed at module scope so the `turn.rs` hook can use it without
/// going through `is_completion_verifier_command` when it wants the
/// negative form.
///
/// ## CB-001 (high) fix: quote-agnostic scanning
///
/// The earlier implementation tried to skip text inside single / double
/// quotes to avoid rejecting commands like
/// `cargo test -- --filter 'a | b'`. That parser, however, did **not** honor
/// shell backslash escapes and so could be tricked by an attacker into
/// laundering a control operator past the gate, e.g.
///
/// * `cargo test \" || true` — the leading `\"` is treated as the start of a
///   double-quoted string by the old scanner, which then skips the rest of
///   the input and never sees `||`. But the real shell treats `\"` as a
///   literal quote and evaluates `|| true` as a control operator.
/// * `cargo test "$(whoami)"` — the old scanner skips the entire double-
///   quoted region without ever inspecting `$(`. The real shell still runs
///   command substitution inside double quotes.
/// * ``cargo test "`whoami`"`` — same shape, backtick form.
///
/// Rather than trying to keep a hand-written shell tokenizer correct (which
/// would now have to model `\\`, `\"`, `$''`, `$"..."`, here-docs, ...), we
/// switch to a **conservative quote-agnostic deny list**. Any of the
/// following anywhere in `command` disqualifies the command from becoming
/// `VerifierExitZero` evidence — regardless of whether it sits inside a
/// quote:
///
/// * `;`, `&`, `|`, `<`, `>` — control operators / redirects
/// * backtick — command substitution
/// * `$(` — command substitution
/// * `\n`, `\r` — newline could hide a follow-on `|| true`
/// * `\\` (backslash) — shell escapes can hide any of the above
///
/// This trades a small amount of recall (a legitimate
/// `cargo test -- --filter 'a | b'` will no longer count as evidence) for a
/// hard guarantee that the gate cannot be bypassed by escape / quote
/// confusion. The gate is **evidence-only**: rejecting a command here just
/// means it doesn't count as `VerifierExitZero`; the command still executes
/// normally through the Bash tool.
pub(crate) fn contains_evidence_poisoning_shell_control(command: &str) -> bool {
    let bytes = command.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        let b = bytes[i];
        match b {
            b';' | b'&' | b'|' | b'<' | b'>' | b'`' | b'\n' | b'\r' | b'\\' => {
                return true;
            }
            b'$' => {
                if i + 1 < bytes.len() && bytes[i + 1] == b'(' {
                    return true;
                }
                i += 1;
            }
            _ => {
                i += 1;
            }
        }
    }
    false
}

// --- Issue #608 Phase α-2: verifier command redactor moved to session layer

/// Issue #606 Stage 4 (DR4-003) → Issue #608 Phase α-2 / 設計判断 #7:
/// canonical redactor for the verifier command string stored inside
/// `CompletionEvidence::VerifierExitZero.command` (and, in alpha-2,
/// `SessionSnapshot.last_verifier_command` /
/// `VerifierInvocationRecord.command`).
///
/// The actual redactor SSOT now lives in
/// [`crate::session::feedback::redact_verifier_command_for_storage`] (session
/// layer, owning the mask_secrets + auth_header + control-char + 4096-byte cap
/// pipeline). This agent-layer wrapper exists only for backward source
/// compatibility — the in-process `observe_evidence_from_bash_outcome` hook
/// continues to call this name, and the session layer must not import from
/// the agent layer (DR3-002 single-directional dependency).
pub(crate) fn redact_verifier_command_for_storage(cmd: &str) -> String {
    crate::session::feedback::redact_verifier_command_for_storage(cmd)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    // ----------------------------- classify_repo_edit_path ----------------

    #[test]
    fn repo_edit_category_classifies_test_path() {
        // U-01
        assert_eq!(
            classify_repo_edit_path(PathBuf::from("src/__tests__/foo.ts")),
            RepoEditCategory::Test
        );
        assert_eq!(
            classify_repo_edit_path(PathBuf::from("tests/foo.rs")),
            RepoEditCategory::Test
        );
        assert_eq!(
            classify_repo_edit_path(PathBuf::from("test_calculator.py")),
            RepoEditCategory::Test
        );
    }

    #[test]
    fn repo_edit_category_classifies_setup_path() {
        // U-02 — setup / manifest files are evidence for dependency/config work.
        assert_eq!(
            classify_repo_edit_path(PathBuf::from("Cargo.toml")),
            RepoEditCategory::Setup
        );
        assert_eq!(
            classify_repo_edit_path(PathBuf::from("Cargo.lock")),
            RepoEditCategory::Setup
        );
        assert_eq!(
            classify_repo_edit_path(PathBuf::from("package.json")),
            RepoEditCategory::Setup
        );
        assert_eq!(
            classify_repo_edit_path(PathBuf::from("tsconfig.json")),
            RepoEditCategory::Setup
        );
        assert_eq!(
            classify_repo_edit_path(PathBuf::from("requirements.txt")),
            RepoEditCategory::Setup
        );
        assert_eq!(
            classify_repo_edit_path(PathBuf::from("pyproject.toml")),
            RepoEditCategory::Setup
        );
    }

    #[test]
    fn repo_edit_category_classifies_impl_path() {
        // U-03
        assert_eq!(
            classify_repo_edit_path(PathBuf::from("src/main.rs")),
            RepoEditCategory::Impl
        );
        assert_eq!(
            classify_repo_edit_path(PathBuf::from("src/app/page.tsx")),
            RepoEditCategory::Impl
        );
    }

    #[test]
    fn repo_edit_category_classifies_docs_path() {
        // U-04 — Docs must be evaluated BEFORE `is_implementation_file`
        // because `.mdx` is in the impl SSOT (DR1-001).
        assert_eq!(
            classify_repo_edit_path(PathBuf::from("README.md")),
            RepoEditCategory::Docs
        );
        assert_eq!(
            classify_repo_edit_path(PathBuf::from("docs/intro.mdx")),
            RepoEditCategory::Docs
        );
        assert_eq!(
            classify_repo_edit_path(PathBuf::from("CHANGELOG.rst")),
            RepoEditCategory::Docs
        );
        assert_eq!(
            classify_repo_edit_path(PathBuf::from("notes.txt")),
            RepoEditCategory::Docs
        );
    }

    #[test]
    fn repo_edit_category_classifies_structured_data_path() {
        assert_eq!(
            classify_repo_edit_path(PathBuf::from("output.csv")),
            RepoEditCategory::Data
        );
        assert_eq!(
            classify_repo_edit_path(PathBuf::from("reports/summary.jsonl")),
            RepoEditCategory::Data
        );
    }

    #[test]
    fn repo_edit_category_classifies_other_path() {
        // U-05
        assert_eq!(
            classify_repo_edit_path(PathBuf::from("Makefile")),
            RepoEditCategory::Other
        );
    }

    #[test]
    fn repo_edit_no_op_detector_returns_true_only_for_matching_hashes() {
        assert!(is_repo_edit_no_op(Some("abc"), Some("abc")));
        assert!(!is_repo_edit_no_op(Some("abc"), Some("def")));
        assert!(!is_repo_edit_no_op(None, Some("abc"))); // file didn't exist before
        assert!(!is_repo_edit_no_op(Some("abc"), None)); // file deleted
        assert!(!is_repo_edit_no_op(None, None));
    }

    // ----------------------------- EvidenceSet basics ---------------------

    #[test]
    fn evidence_set_push_and_iterate() {
        let mut set = EvidenceSet::new();
        assert!(set.is_empty());
        set.push(CompletionEvidence::RepoEdit {
            category: RepoEditCategory::Impl,
            count: 1,
            path: None,
        });
        set.push(CompletionEvidence::VerifierExitZero {
            class: BashCommandClass::BuildTest,
            command: "cargo test".to_string(),
            bound_test_artifacts_count: None,
        });
        assert_eq!(set.len(), 2);
        assert!(!set.is_empty());
        let collected: Vec<_> = set.iter().collect();
        assert_eq!(collected.len(), 2);
    }

    #[test]
    fn evidence_set_clear_resets_state() {
        let mut set = EvidenceSet::new();
        set.push(CompletionEvidence::AnswerOnly);
        set.clear();
        assert!(set.is_empty());
    }

    // ----------------------------- security gate --------------------------

    #[test]
    fn verifier_command_with_or_true_is_rejected() {
        // U-21 (DR4-002): `||` is a shell control operator that can mask a
        // failing inner exit code.
        assert!(!is_completion_verifier_command("cargo test || true"));
        assert!(!is_completion_verifier_command("pytest && false"));
        assert!(!is_completion_verifier_command("npm test ; echo ok"));
    }

    #[test]
    fn verifier_command_with_pipe_or_redirect_is_rejected() {
        assert!(!is_completion_verifier_command("cargo test | tee out.log"));
        assert!(!is_completion_verifier_command("cargo test > out.log"));
        assert!(!is_completion_verifier_command("cat input | pytest"));
    }

    #[test]
    fn verifier_command_with_backtick_or_dollar_paren_is_rejected() {
        assert!(!is_completion_verifier_command(
            "cargo test --test `whoami`"
        ));
        assert!(!is_completion_verifier_command(
            "cargo test --test $(whoami)"
        ));
    }

    #[test]
    fn verifier_command_with_embedded_newline_is_rejected() {
        // A multi-line heredoc-style command sneaking in an `|| true`
        // should still be rejected for the bare newline alone.
        assert!(!is_completion_verifier_command("cargo test\necho bypass"));
    }

    #[test]
    fn verifier_command_plain_cargo_test_is_accepted() {
        assert!(is_completion_verifier_command("cargo test"));
        assert!(is_completion_verifier_command("cargo test --workspace"));
        assert!(is_completion_verifier_command("pytest -q"));
        assert!(is_completion_verifier_command("npm test"));
    }

    #[test]
    fn verifier_command_accepts_typical_release_and_module_filters() {
        // CB-001: the new strict gate is still permissive enough for the
        // common verifier shapes that have no shell-meta characters.
        assert!(is_completion_verifier_command("cargo test --release"));
        assert!(is_completion_verifier_command("cargo test foo::bar"));
        assert!(is_completion_verifier_command(
            "cargo test --no-default-features"
        ));
        assert!(is_completion_verifier_command("pytest -k some_name"));
    }

    // -- CB-001 (high): quote-agnostic / backslash-aware shell-control gate --

    #[test]
    fn cb_001_escaped_quote_followed_by_or_true_is_rejected() {
        // The old scanner mis-treated `\"` as a string opener and never saw
        // the trailing `|| true`. The shell evaluates `\"` as a literal
        // quote and `|| true` as a real control operator, so the verifier
        // could exit 0 even when the inner `cargo test` failed.
        assert!(!is_completion_verifier_command("cargo test \\\" || true"));
        // Backslash alone also disqualifies — escapes can hide any operator.
        assert!(!is_completion_verifier_command("cargo test \\; echo bad"));
    }

    #[test]
    fn cb_001_double_quoted_command_substitution_is_rejected() {
        // `"$(...)"` runs command substitution inside double quotes — the
        // old quote-skipping scanner missed it entirely.
        assert!(!is_completion_verifier_command("cargo test \"$(whoami)\""));
        assert!(!is_completion_verifier_command(
            "cargo test --bin \"$(echo a)\""
        ));
    }

    #[test]
    fn cb_001_double_quoted_backtick_is_rejected() {
        // Backticks inside double quotes still run command substitution.
        assert!(!is_completion_verifier_command("cargo test \"`whoami`\""));
    }

    #[test]
    fn cb_001_double_quoted_pipe_is_rejected_by_strict_gate() {
        // Note: this differs from the pre-CB-001 behavior, which
        // accepted operators inside any matched-quote region. The new
        // gate is conservative on purpose: rejecting the evidence is
        // safe (the command itself still executes through Bash), and
        // accepting it requires us to reproduce the full shell quoting
        // grammar correctly — a hand-rolled tokenizer's track record
        // here is what produced CB-001 in the first place.
        assert!(!is_completion_verifier_command("cargo test \"a | b\""));
        assert!(!is_completion_verifier_command(
            "cargo test -- --filter 'a | b'"
        ));
    }

    #[test]
    fn cb_001_bare_backslash_anywhere_is_rejected() {
        // Backslashes can encode any control character via shell escapes
        // — refuse them outright in the verifier-evidence gate.
        assert!(!is_completion_verifier_command("cargo test --bin foo\\bar"));
        assert!(!is_completion_verifier_command("pytest path\\to\\test"));
    }

    // -- CB-002 (medium): dedicated verifier-command storage redactor --------

    #[test]
    fn cb_002_authorization_bearer_header_is_redacted() {
        let out = redact_verifier_command_for_storage(
            "curl -H 'Authorization: Bearer abc123def456' http://localhost/x",
        );
        assert!(
            !out.contains("abc123def456"),
            "Bearer token must be redacted, got: {out}"
        );
        assert!(
            out.contains("Authorization") && out.contains("<REDACTED>"),
            "expected Authorization tag preserved with <REDACTED> sentinel, got: {out}"
        );
    }

    #[test]
    fn cb_002_cookie_header_is_redacted() {
        let out = redact_verifier_command_for_storage(
            "curl -H 'Cookie: session=xyz987' http://localhost/x",
        );
        assert!(
            !out.contains("session=xyz987"),
            "Cookie value must be redacted, got: {out}"
        );
        assert!(
            out.contains("Cookie") && out.contains("<REDACTED>"),
            "expected Cookie tag preserved with <REDACTED> sentinel, got: {out}"
        );
    }

    #[test]
    fn cb_002_x_api_key_header_is_redacted() {
        let out = redact_verifier_command_for_storage(
            "curl -H 'X-API-Key: super-secret-value-123' http://localhost/x",
        );
        assert!(
            !out.contains("super-secret-value-123"),
            "X-API-Key value must be redacted, got: {out}"
        );
        assert!(
            out.contains("X-API-Key") && out.contains("<REDACTED>"),
            "expected X-API-Key tag preserved with <REDACTED> sentinel, got: {out}"
        );
    }

    #[test]
    fn cb_002_control_chars_are_neutralized() {
        let out = redact_verifier_command_for_storage("cargo test\necho bad\twith\ttab");
        assert!(!out.contains('\n'), "newline must be neutralized: {out:?}");
        assert!(!out.contains('\t'), "tab must be neutralized: {out:?}");
        assert!(!out.contains('\r'), "CR must be neutralized: {out:?}");
        // Body content survives (just with spaces in place of controls).
        assert!(out.contains("cargo test") && out.contains("echo bad"));
    }

    #[test]
    fn cb_002_existing_mask_secrets_behavior_is_preserved() {
        // SECRET=... kv shape is still redacted by the wrapped mask_secrets
        // SSOT (`src/session/feedback.rs::kv_secret_regex`).
        let out =
            redact_verifier_command_for_storage("env SECRET=topsecretvalue cargo test --release");
        assert!(
            !out.contains("topsecretvalue"),
            "kv secret must remain redacted by mask_secrets: {out}"
        );
        assert!(
            out.contains("SECRET=***"),
            "expected kv keyword preserved with *** sentinel, got: {out}"
        );
        // URL userinfo redaction still applies.
        let out2 = redact_verifier_command_for_storage("curl https://user:pw@example.com/path");
        assert!(
            !out2.contains("user:pw"),
            "URL userinfo must be masked: {out2}"
        );
    }

    #[test]
    fn cb_002_plain_command_passes_through_unchanged() {
        // Pure-ASCII verifier commands with no secrets / control chars are
        // returned essentially as-is (only the redaction passes touch the
        // string).
        let out = redact_verifier_command_for_storage("cargo test --workspace --all-targets");
        assert_eq!(out, "cargo test --workspace --all-targets");
    }
}
