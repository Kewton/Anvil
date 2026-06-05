//! Issue #978 (parent #974, Issue D): deterministic EvidenceFailed repair
//! operator for missing serde-family Cargo dependencies.
//!
//! When a Rust verifier failure (`cargo test` / `cargo build`) reports that a
//! serde-family crate is undeclared/unresolved (`error[E0432]`/`error[E0433]`,
//! "use of undeclared crate or module `serde`") and the workspace `Cargo.toml`
//! does not declare that crate under `[dependencies]`, this operator inserts
//! the canonical dependency line deterministically — *not* LLM free
//! regeneration — so the existing EvidenceRunner reruns against a buildable
//! manifest on the next verifier pass.
//!
//! This is the EvidenceFailed counterpart to the MissingEvidence Node
//! manifest operator (`node_runner_manifest`): it runs inside the deterministic
//! repair slot (`repair_job_dispatch::handle_repair_job_patch_provider_step`)
//! *before* the LLM verifier-repair pass, mirroring
//! `mechanical_compile_repair::try_apply_mechanical_compile_repair`.
//!
//! Mirrors the replay-fixture shape named in the issue:
//! - `052`: tests/impl import `serde` / `serde_json`, but the crate is absent
//!   from `[dependencies]`.
//!
//! Scope is deliberately narrow. Only the serde-family allowlist is handled
//! (stable crates with a known-safe canonical spec, so the additive edit is
//! always correct); anything else defers to the LLM repair pass. The pure
//! core (`undeclared_known_crates` + `complete_cargo_dependencies`) takes raw
//! text and is fully unit-tested; all `Agent` / filesystem access lives in
//! [`try_apply_cargo_dependency_repair`].
//!
//! `pub(super)` limited / no facade re-export (DR3-001).

use std::path::Path;

use super::Agent;
use super::repair_driver::VERIFIER_REPAIR_PASS_MAX_FILE_BYTES;
use super::repair_patch_validation::{
    RepairIntentEdit, ValidatedVerifierRepairEdit, repair_intent_edits_fingerprint,
    validate_repair_candidate_changed, validate_repair_intent_not_replayed,
};
use super::task_contract::{ArtifactRole, RecoveryTargetHint};
use crate::logging::log_llm_event;
use crate::session::feedback::mask_secrets;

/// The Cargo manifest this operator completes. Relative to the workspace root.
const MANIFEST_RELATIVE_PATH: &str = "Cargo.toml";

/// Allowlist of well-known serde-family crates this operator can declare, with
/// the canonical dependency line. Restricted so the deterministic edit is
/// always safe: these are stable, ubiquitous crates and the spec matches the
/// project's own manifest convention. Order is the deterministic insertion
/// order when more than one is missing.
const KNOWN_DEPENDENCY_LINES: &[(&str, &str)] = &[
    (
        "serde",
        "serde = { version = \"1\", features = [\"derive\"] }",
    ),
    ("serde_json", "serde_json = \"1\""),
];

/// Diagnostic phrases (lowercased) that indicate a crate could not be
/// resolved. Any one co-occurring with a backtick-wrapped known crate name is
/// treated as a missing-dependency signal.
const UNRESOLVED_SIGNAL_PHRASES: &[&str] = &[
    "use of undeclared crate or module",
    "unresolved import",
    "maybe a missing crate",
    "can't find crate",
    "failed to resolve",
];

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum CargoDependencyRepairOutcome {
    Applied { relative_path: String },
    Skipped { reason: &'static str },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct CargoDependencyCompletion {
    /// The full updated manifest text.
    pub(super) contents: String,
    /// The crate names added, in deterministic order. Used for the replay
    /// fingerprint and the operator log.
    pub(super) added: Vec<&'static str>,
}

/// Parse a verifier diagnostic for serde-family crates reported as
/// undeclared/unresolved. Returns the matched crate names in canonical
/// (allowlist) order, de-duplicated.
///
/// Matching is anchored on the backtick-wrapped crate name (`` `serde` ``)
/// so `` `serde_json` `` is never mistaken for `serde`, combined with an
/// unresolved-signal phrase so an unrelated mention of the crate does not
/// trigger a manifest edit.
pub(super) fn undeclared_known_crates(diagnostic: &str) -> Vec<&'static str> {
    let lower = diagnostic.to_ascii_lowercase();
    if !UNRESOLVED_SIGNAL_PHRASES
        .iter()
        .any(|phrase| lower.contains(phrase))
    {
        return Vec::new();
    }
    KNOWN_DEPENDENCY_LINES
        .iter()
        .filter(|(name, _)| lower.contains(&format!("`{name}`")))
        .map(|(name, _)| *name)
        .collect()
}

/// Deterministically complete the `[dependencies]` table of `existing` so it
/// declares every crate in `candidates` (canonical allowlist names).
///
/// - Returns `None` if every candidate is already declared under
///   `[dependencies]` (nothing to add).
/// - Returns `None` if the manifest is structurally ambiguous (a
///   `[dependencies.<name>]` sub-table exists but no bare `[dependencies]`
///   header) — clobbering that shape is not deterministically safe; the LLM
///   repair path owns it.
/// - With a bare `[dependencies]` header, the missing dep lines are inserted
///   immediately after it. With no dependencies table at all, a new
///   `[dependencies]` table is appended.
pub(super) fn complete_cargo_dependencies(
    existing: &str,
    candidates: &[&'static str],
) -> Option<CargoDependencyCompletion> {
    let scan = scan_dependencies_table(existing);
    let added: Vec<&'static str> = KNOWN_DEPENDENCY_LINES
        .iter()
        .map(|(name, _)| *name)
        .filter(|name| {
            candidates.contains(name) && !scan.declared_keys.iter().any(|key| key.as_str() == *name)
        })
        .collect();
    if added.is_empty() {
        return None;
    }
    let dep_lines: Vec<&'static str> = added
        .iter()
        .filter_map(|name| dependency_line_for(name))
        .collect();
    // `added` is drawn from the allowlist, so every name has a line.
    debug_assert_eq!(dep_lines.len(), added.len());

    let contents = if let Some(header_line) = scan.bare_header_line {
        insert_after_line(existing, header_line, &dep_lines)
    } else if scan.has_any_dependencies_table {
        // Only `[dependencies.<name>]` sub-tables, no bare header: ambiguous.
        return None;
    } else {
        append_dependencies_table(existing, &dep_lines)
    };
    Some(CargoDependencyCompletion { contents, added })
}

fn dependency_line_for(name: &str) -> Option<&'static str> {
    KNOWN_DEPENDENCY_LINES
        .iter()
        .find(|(crate_name, _)| *crate_name == name)
        .map(|(_, line)| *line)
}

struct DependenciesScan {
    /// Crate keys declared under the top-level `[dependencies]` table,
    /// including `[dependencies.<name>]` sub-table names.
    declared_keys: Vec<String>,
    /// Line index of a bare `[dependencies]` header, if present.
    bare_header_line: Option<usize>,
    /// True if any `[dependencies]` or `[dependencies.<name>]` table exists.
    has_any_dependencies_table: bool,
}

/// Scan the manifest for the top-level `[dependencies]` table: its declared
/// keys, whether a bare header exists, and whether any dependencies table
/// (bare or sub-table) is present. Conservative line-based parsing — no TOML
/// crate dependency — restricted to the unambiguous shapes the operator acts
/// on.
fn scan_dependencies_table(manifest: &str) -> DependenciesScan {
    let mut declared_keys = Vec::new();
    let mut bare_header_line = None;
    let mut has_any_dependencies_table = false;
    let mut in_bare_dependencies = false;

    for (index, raw_line) in manifest.lines().enumerate() {
        let line = strip_comment(raw_line).trim();
        if line.is_empty() {
            continue;
        }
        if let Some(header) = parse_table_header(line) {
            in_bare_dependencies = false;
            if header == "dependencies" {
                has_any_dependencies_table = true;
                if bare_header_line.is_none() {
                    bare_header_line = Some(index);
                }
                in_bare_dependencies = true;
            } else if let Some(sub) = header.strip_prefix("dependencies.") {
                has_any_dependencies_table = true;
                // `[dependencies.tokio]` / `[dependencies.tokio.something]`:
                // the declared crate key is the first segment.
                if let Some(name) = sub.split('.').next() {
                    let key = unquote_key(name);
                    if !key.is_empty() && !declared_keys.contains(&key) {
                        declared_keys.push(key);
                    }
                }
            }
            continue;
        }
        if in_bare_dependencies
            && let Some(key) = parse_dependency_key(line)
            && !declared_keys.contains(&key)
        {
            declared_keys.push(key);
        }
    }

    DependenciesScan {
        declared_keys,
        bare_header_line,
        has_any_dependencies_table,
    }
}

/// Strip a trailing `# ...` comment, respecting quoted strings so a `#` inside
/// a value (e.g. a URL fragment) is not treated as a comment start.
fn strip_comment(line: &str) -> &str {
    let mut in_single = false;
    let mut in_double = false;
    for (index, ch) in line.char_indices() {
        match ch {
            '\'' if !in_double => in_single = !in_single,
            '"' if !in_single => in_double = !in_double,
            '#' if !in_single && !in_double => return &line[..index],
            _ => {}
        }
    }
    line
}

/// Parse a `[table.header]` line into its dotted path, or `None` if the line
/// is not a single table header (e.g. `[[array.of.tables]]`).
fn parse_table_header(line: &str) -> Option<String> {
    let inner = line.strip_prefix('[')?.strip_suffix(']')?;
    if inner.starts_with('[') || inner.ends_with(']') {
        // Array-of-tables `[[...]]`.
        return None;
    }
    let trimmed = inner.trim();
    if trimmed.is_empty() {
        return None;
    }
    Some(trimmed.to_string())
}

/// Extract the dependency key from a `key = value` line, or `None` if the line
/// is not a simple key assignment.
fn parse_dependency_key(line: &str) -> Option<String> {
    let (key_part, _) = line.split_once('=')?;
    let key = unquote_key(key_part.trim());
    (!key.is_empty()).then_some(key)
}

fn unquote_key(key: &str) -> String {
    let trimmed = key.trim();
    let unquoted = trimmed
        .strip_prefix('"')
        .and_then(|rest| rest.strip_suffix('"'))
        .or_else(|| {
            trimmed
                .strip_prefix('\'')
                .and_then(|rest| rest.strip_suffix('\''))
        })
        .unwrap_or(trimmed);
    unquoted.to_string()
}

fn insert_after_line(manifest: &str, header_line: usize, dep_lines: &[&str]) -> String {
    let mut out = String::with_capacity(manifest.len() + dependency_lines_len(dep_lines));
    let trailing_newline = manifest.ends_with('\n');
    for (index, line) in manifest.lines().enumerate() {
        out.push_str(line);
        out.push('\n');
        if index == header_line {
            for dep in dep_lines {
                out.push_str(dep);
                out.push('\n');
            }
        }
    }
    if !trailing_newline {
        out.pop();
    }
    out
}

fn append_dependencies_table(manifest: &str, dep_lines: &[&str]) -> String {
    let mut out = String::with_capacity(manifest.len() + dependency_lines_len(dep_lines) + 16);
    out.push_str(manifest);
    if !manifest.is_empty() && !manifest.ends_with('\n') {
        out.push('\n');
    }
    if !manifest.is_empty() {
        out.push('\n');
    }
    out.push_str("[dependencies]\n");
    for dep in dep_lines {
        out.push_str(dep);
        out.push('\n');
    }
    out
}

fn dependency_lines_len(dep_lines: &[&str]) -> usize {
    dep_lines.iter().map(|line| line.len() + 1).sum()
}

/// Run the deterministic Cargo serde-dependency operator for the active
/// repair job. Reads the failure diagnostic from `agent.repair_job`, completes
/// the workspace `Cargo.toml` if a serde-family crate is missing, applies the
/// edit through the standard validated-repair-edit machinery, records it, and
/// returns `Applied` so the caller reruns the verifier. Any non-applicable
/// condition returns `Skipped` (the caller falls through to the LLM pass).
pub(super) fn try_apply_cargo_dependency_repair(agent: &mut Agent) -> CargoDependencyRepairOutcome {
    let Some(context) = agent.repair_job.clone() else {
        return skipped("missing_repair_job");
    };
    let diagnostic = cargo_dependency_diagnostic_for_job(&context);
    let candidates = undeclared_known_crates(&diagnostic);
    if candidates.is_empty() {
        return skipped("no_missing_dependency");
    }
    let Some((canonical_path, original_contents)) = read_manifest(&agent.work_root) else {
        return skipped("manifest_unavailable");
    };
    let Some(completion) = complete_cargo_dependencies(&original_contents, &candidates) else {
        return skipped("already_declared_or_ambiguous");
    };

    let added_label = completion.added.join(",");
    let edit_payload = [RepairIntentEdit {
        old_string: "<cargo-dependencies>",
        new_string: &added_label,
        replace_all: false,
    }];
    let fingerprint = repair_intent_edits_fingerprint(
        &context.failure_signature,
        MANIFEST_RELATIVE_PATH,
        &edit_payload,
    );
    if validate_repair_candidate_changed(&original_contents, &completion.contents).is_err() {
        return skipped("no_change");
    }
    if validate_repair_intent_not_replayed(&context.applied_repair_intents, &fingerprint).is_err() {
        log_cargo_dependency_skipped(agent, &added_label, "already_applied");
        return skipped("already_applied");
    }

    let target_hint = RecoveryTargetHint {
        role: ArtifactRole::Setup,
        path: MANIFEST_RELATIVE_PATH.to_string(),
        reason: format!("missing Cargo dependency: {added_label}"),
    };
    let edit = ValidatedVerifierRepairEdit::new(
        MANIFEST_RELATIVE_PATH.to_string(),
        canonical_path,
        &original_contents,
        completion.contents,
        fingerprint,
    );
    if let Err(err) = super::repair_patch_executor::apply_validated_repair_edit(&edit) {
        log_cargo_dependency_skipped(agent, &added_label, "apply_failed");
        log_llm_event(
            "agent.verifier_cargo_dependency_repair.apply_failed",
            serde_json::json!({
                "session_id": agent.session_store.session_id(),
                "path": MANIFEST_RELATIVE_PATH,
                "added": added_label,
                "error": mask_secrets(&err),
            }),
        );
        return skipped("apply_failed");
    }
    super::verifier_orchestration::record_controller_verifier_repair_edit(
        agent,
        &edit.relative_path,
        &edit.fingerprint,
        &target_hint,
    );
    log_llm_event(
        "agent.verifier_cargo_dependency_repair.applied",
        serde_json::json!({
            "session_id": agent.session_store.session_id(),
            "path": edit.relative_path,
            "added": added_label,
            "preimage_hash": edit.preimage_hash,
            "postimage_hash": edit.postimage_hash,
        }),
    );
    CargoDependencyRepairOutcome::Applied {
        relative_path: edit.relative_path,
    }
}

fn skipped(reason: &'static str) -> CargoDependencyRepairOutcome {
    CargoDependencyRepairOutcome::Skipped { reason }
}

/// Reconstruct the verifier diagnostic text from the repair job (error kind +
/// bounded output excerpt). Mirrors
/// `mechanical_compile_repair::mechanical_repair_diagnostic_for_job`.
fn cargo_dependency_diagnostic_for_job(job: &super::repair_job::RepairJob) -> String {
    let error_kind = job
        .error_kind
        .as_deref()
        .filter(|value| !value.trim().is_empty())
        .unwrap_or(job.failure_signature.as_str());
    if job.output_excerpt.trim().is_empty() {
        error_kind.to_string()
    } else {
        format!("{error_kind}\n{}", job.output_excerpt)
    }
}

fn read_manifest(work_root: &Path) -> Option<(std::path::PathBuf, String)> {
    let path = work_root.join(MANIFEST_RELATIVE_PATH);
    let canonical = std::fs::canonicalize(&path).ok()?;
    if !canonical.is_file() {
        return None;
    }
    let metadata = std::fs::metadata(&canonical).ok()?;
    if metadata.len() > VERIFIER_REPAIR_PASS_MAX_FILE_BYTES {
        return None;
    }
    let contents = std::fs::read_to_string(&canonical).ok()?;
    Some((canonical, contents))
}

fn log_cargo_dependency_skipped(agent: &Agent, added: &str, reason: &'static str) {
    log_llm_event(
        "agent.verifier_cargo_dependency_repair.skipped",
        serde_json::json!({
            "session_id": agent.session_store.session_id(),
            "path": MANIFEST_RELATIVE_PATH,
            "added": added,
            "reason": reason,
        }),
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::loop_run::commands::test_agent_with_config;
    use crate::config::Config;
    use crate::session::store::ConversationMessage;

    #[test]
    fn detects_undeclared_serde_from_e0432() {
        let diagnostic =
            "error[E0432]: unresolved import `serde`\n use of undeclared crate or module `serde`";
        assert_eq!(undeclared_known_crates(diagnostic), vec!["serde"]);
    }

    #[test]
    fn detects_undeclared_serde_json_from_e0433() {
        let diagnostic =
            "error[E0433]: failed to resolve: use of undeclared crate or module `serde_json`";
        assert_eq!(undeclared_known_crates(diagnostic), vec!["serde_json"]);
    }

    #[test]
    fn detects_both_in_canonical_order() {
        let diagnostic =
            "use of undeclared crate or module `serde_json`\nunresolved import `serde`";
        assert_eq!(
            undeclared_known_crates(diagnostic),
            vec!["serde", "serde_json"]
        );
    }

    #[test]
    fn serde_backtick_does_not_match_inside_serde_json() {
        // `serde_json` mentioned but not `serde` alone: only serde_json matches.
        let diagnostic = "error[E0432]: unresolved import `serde_json`";
        assert_eq!(undeclared_known_crates(diagnostic), vec!["serde_json"]);
    }

    #[test]
    fn unrelated_serde_mention_without_unresolved_signal_is_ignored() {
        let diagnostic = "note: the trait `serde::Serialize` is implemented for `Foo`";
        assert!(undeclared_known_crates(diagnostic).is_empty());
    }

    #[test]
    fn non_allowlisted_crate_is_ignored() {
        let diagnostic =
            "error[E0432]: unresolved import `tokio`\n use of undeclared crate or module `tokio`";
        assert!(undeclared_known_crates(diagnostic).is_empty());
    }

    #[test]
    fn inserts_after_existing_bare_dependencies_header() {
        let manifest =
            "[package]\nname = \"app\"\nversion = \"0.1.0\"\n\n[dependencies]\nanyhow = \"1\"\n";
        let completion = complete_cargo_dependencies(manifest, &["serde"]).expect("adds serde");
        assert_eq!(completion.added, vec!["serde"]);
        let expected = "[package]\nname = \"app\"\nversion = \"0.1.0\"\n\n[dependencies]\nserde = { version = \"1\", features = [\"derive\"] }\nanyhow = \"1\"\n";
        assert_eq!(completion.contents, expected);
    }

    #[test]
    fn appends_dependencies_table_when_absent() {
        let manifest = "[package]\nname = \"app\"\nversion = \"0.1.0\"\n";
        let completion =
            complete_cargo_dependencies(manifest, &["serde", "serde_json"]).expect("adds both");
        assert_eq!(completion.added, vec!["serde", "serde_json"]);
        let expected = "[package]\nname = \"app\"\nversion = \"0.1.0\"\n\n[dependencies]\nserde = { version = \"1\", features = [\"derive\"] }\nserde_json = \"1\"\n";
        assert_eq!(completion.contents, expected);
    }

    #[test]
    fn already_declared_dependency_is_a_noop() {
        let manifest = "[dependencies]\nserde = { version = \"1\" }\n";
        assert!(complete_cargo_dependencies(manifest, &["serde"]).is_none());
    }

    #[test]
    fn declared_as_subtable_is_a_noop() {
        let manifest = "[dependencies]\nanyhow = \"1\"\n\n[dependencies.serde]\nversion = \"1\"\nfeatures = [\"derive\"]\n";
        assert!(complete_cargo_dependencies(manifest, &["serde"]).is_none());
    }

    #[test]
    fn only_missing_crate_is_added_when_one_already_present() {
        let manifest = "[dependencies]\nserde = { version = \"1\" }\n";
        let completion = complete_cargo_dependencies(manifest, &["serde", "serde_json"])
            .expect("serde_json still missing");
        assert_eq!(completion.added, vec!["serde_json"]);
        assert!(completion.contents.contains("serde_json = \"1\""));
        // The existing serde line is preserved exactly once.
        assert_eq!(completion.contents.matches("serde = { version").count(), 1);
    }

    #[test]
    fn subtable_only_without_bare_header_is_skipped_as_ambiguous() {
        let manifest = "[package]\nname = \"app\"\n\n[dependencies.anyhow]\nversion = \"1\"\n";
        // serde is genuinely missing, but there is no bare `[dependencies]`
        // header to insert under, so the operator defers to the LLM.
        assert!(complete_cargo_dependencies(manifest, &["serde"]).is_none());
    }

    #[test]
    fn commented_out_dependency_does_not_count_as_declared() {
        let manifest = "[dependencies]\n# serde = \"1\"\nanyhow = \"1\"\n";
        let completion = complete_cargo_dependencies(manifest, &["serde"]).expect("adds serde");
        assert_eq!(completion.added, vec!["serde"]);
    }

    #[test]
    fn manifest_without_trailing_newline_is_handled() {
        let manifest = "[package]\nname = \"app\"\n\n[dependencies]\nanyhow = \"1\"";
        let completion = complete_cargo_dependencies(manifest, &["serde"]).expect("adds serde");
        assert!(
            completion
                .contents
                .contains("serde = { version = \"1\", features = [\"derive\"] }")
        );
        // anyhow is preserved.
        assert!(completion.contents.contains("anyhow = \"1\""));
    }

    fn rust_repair_agent_with_manifest(manifest: &str) -> (Agent, tempfile::TempDir) {
        let (mut agent, temp) = test_agent_with_config(Config::default());
        agent.session.messages.push(ConversationMessage::user(
            "Create a Rust JSON parser using serde and verify it with cargo test.".to_string(),
        ));
        std::fs::write(agent.work_root.join("Cargo.toml"), manifest).unwrap();
        std::fs::create_dir_all(agent.work_root.join("src")).unwrap();
        std::fs::write(
            agent.work_root.join("src/lib.rs"),
            "use serde::Serialize;\n#[derive(Serialize)]\npub struct Row;\n",
        )
        .unwrap();
        (agent, temp)
    }

    fn job_with_serde_failure() -> super::super::repair_job::RepairJob {
        let mut job = super::super::repair_job::RepairJob::new_for_test();
        job.command = "cargo test".to_string();
        job.output_excerpt = "error[E0432]: unresolved import `serde`\n --> src/lib.rs:1:5\n  |\n1 | use serde::Serialize;\n  |     ^^^^^ use of undeclared crate or module `serde`".to_string();
        job.failure_signature = "src/lib.rs E0432 serde".to_string();
        job.error_kind = Some("error[E0432]".to_string());
        job
    }

    #[test]
    fn applying_operator_writes_manifest_and_reruns_verifier_next() {
        let manifest = "[package]\nname = \"app\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\n[dependencies]\nanyhow = \"1\"\n";
        let (mut agent, _temp) = rust_repair_agent_with_manifest(manifest);
        agent.repair_job = Some(job_with_serde_failure());

        let outcome = try_apply_cargo_dependency_repair(&mut agent);

        assert_eq!(
            outcome,
            CargoDependencyRepairOutcome::Applied {
                relative_path: "Cargo.toml".to_string()
            }
        );
        let written = std::fs::read_to_string(agent.work_root.join("Cargo.toml")).unwrap();
        assert!(written.contains("serde = { version = \"1\", features = [\"derive\"] }"));
        assert!(written.contains("anyhow = \"1\""));
        assert_eq!(
            agent.repair_job.as_ref().unwrap().next_action(),
            super::super::repair_job::RepairNextAction::RerunVerifier
        );
    }

    #[test]
    fn operator_skips_when_dependency_already_declared() {
        let manifest = "[package]\nname = \"app\"\nversion = \"0.1.0\"\n\n[dependencies]\nserde = { version = \"1\", features = [\"derive\"] }\n";
        let (mut agent, _temp) = rust_repair_agent_with_manifest(manifest);
        agent.repair_job = Some(job_with_serde_failure());

        let outcome = try_apply_cargo_dependency_repair(&mut agent);

        assert_eq!(
            outcome,
            CargoDependencyRepairOutcome::Skipped {
                reason: "already_declared_or_ambiguous"
            }
        );
    }

    #[test]
    fn operator_skips_when_diagnostic_has_no_missing_dependency() {
        let manifest = "[package]\nname = \"app\"\n\n[dependencies]\nanyhow = \"1\"\n";
        let (mut agent, _temp) = rust_repair_agent_with_manifest(manifest);
        let mut job = super::super::repair_job::RepairJob::new_for_test();
        job.command = "cargo test".to_string();
        job.output_excerpt = "error[E0308]: mismatched types".to_string();
        job.failure_signature = "src/lib.rs E0308".to_string();
        job.error_kind = Some("error[E0308]".to_string());
        agent.repair_job = Some(job);

        let outcome = try_apply_cargo_dependency_repair(&mut agent);

        assert_eq!(
            outcome,
            CargoDependencyRepairOutcome::Skipped {
                reason: "no_missing_dependency"
            }
        );
    }

    #[test]
    fn applying_operator_is_idempotent_once_dependency_present() {
        let manifest = "[package]\nname = \"app\"\n\n[dependencies]\nanyhow = \"1\"\n";
        let (mut agent, _temp) = rust_repair_agent_with_manifest(manifest);
        agent.repair_job = Some(job_with_serde_failure());

        let first = try_apply_cargo_dependency_repair(&mut agent);
        assert!(matches!(
            first,
            CargoDependencyRepairOutcome::Applied { .. }
        ));

        // The manifest now declares serde, so a second pass with the same
        // failure has nothing to add and the operator stays a no-op — the loop
        // cannot churn on this operator.
        let second = try_apply_cargo_dependency_repair(&mut agent);
        assert_eq!(
            second,
            CargoDependencyRepairOutcome::Skipped {
                reason: "already_declared_or_ambiguous"
            }
        );
    }

    #[test]
    fn operator_does_not_replay_the_same_edit_after_manifest_revert() {
        // Belt-and-suspenders: even if the manifest is reverted (e.g. a
        // checkpoint rollback) while the applied-intent ledger persists, the
        // same fingerprint must not be re-applied.
        let manifest = "[package]\nname = \"app\"\n\n[dependencies]\nanyhow = \"1\"\n";
        let (mut agent, _temp) = rust_repair_agent_with_manifest(manifest);
        agent.repair_job = Some(job_with_serde_failure());

        let first = try_apply_cargo_dependency_repair(&mut agent);
        assert!(matches!(
            first,
            CargoDependencyRepairOutcome::Applied { .. }
        ));

        // Revert the manifest to its original (serde-missing) state, keeping
        // the applied-intent ledger recorded on the repair job.
        std::fs::write(agent.work_root.join("Cargo.toml"), manifest).unwrap();
        let second = try_apply_cargo_dependency_repair(&mut agent);
        assert_eq!(
            second,
            CargoDependencyRepairOutcome::Skipped {
                reason: "already_applied"
            }
        );
    }
}
