//! Issue #991 (parent #988, Issue C): deterministic repair operators for Rust
//! *binding* mismatches detected at verifier-failure time.
//!
//! v0.6.4 Rust failures are not only missing dependencies (owned by
//! [`super::cargo_dependency_repair`]). A recurring class is a *binding*
//! mismatch between the deliverable and the evidence runner: an integration
//! test imports the library crate under a name the `[lib]` target does not
//! publish, or it resolves a binary with `env!("CARGO_BIN_EXE_<name>")` for a
//! bin target that does not exist. Dropping these into the LLM free-edit path
//! converges poorly (`repair_exhausted`).
//!
//! This module adds a small registry of single-failure-class operators that run
//! in the deterministic repair slot
//! (`repair_job_dispatch::handle_repair_job_patch_provider_step`) *before* the
//! LLM verifier-repair pass, mirroring [`super::cargo_dependency_repair`] and
//! [`super::mechanical_compile_repair`]. Each operator applies a deterministic
//! edit through the standard validated-repair-edit machinery, so the existing
//! EvidenceRunner reruns on the next verifier pass.
//!
//! Operators (each fires only when its binding check is unambiguous; ambiguous
//! cases return `Skipped` so the controller defers to ContractConflictJob / the
//! LLM minimal-edit pass):
//! - `rust_fix_lib_name_for_integration_test` — set `[lib] name` so an
//!   integration test's `use <lib>::…` resolves, gated by symbol *provenance*
//!   (the imported symbols are `pub` in `src/lib.rs`) and a no-conflict scan.
//! - `rust_fix_cargo_bin_exe_test_env` — rename a test's
//!   `CARGO_BIN_EXE_<name>` reference to the single real bin target.
//!
//! Until #988 Issue A's `EvidenceBindingPlan` lands, the local binding check is
//! the [`RustBindingMismatch`] enum, keyed on the verifier failure diagnostic +
//! workspace inspection (the same keying as `cargo_dependency_repair`).
//!
//! The pure core (diagnostic parsing, manifest/test scanning, edit builders)
//! takes raw text and is fully unit-tested; all `Agent` / filesystem access
//! lives in [`try_apply_rust_binding_repair`].
//!
//! `pub(super)` limited / no facade re-export (DR3-001).

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use super::Agent;
use super::repair_driver::VERIFIER_REPAIR_PASS_MAX_FILE_BYTES;
use super::repair_patch_validation::{
    RepairIntentEdit, ValidatedVerifierRepairEdit, detect_repair_candidate_weakening_patterns,
    repair_intent_edits_fingerprint, validate_repair_candidate_changed,
    validate_repair_candidate_weakening_patterns, validate_repair_intent_not_replayed,
};
use super::task_contract::{ArtifactRole, RecoveryTargetHint};
use crate::logging::log_llm_event;
use crate::safety::path_guard::resolve_user_path;
use crate::session::feedback::mask_secrets;

/// The Cargo manifest the lib-name operator edits. Relative to the workspace.
const MANIFEST_RELATIVE_PATH: &str = "Cargo.toml";
/// The lib crate root the lib-name operator requires to exist (provenance).
const LIB_RS_RELATIVE_PATH: &str = "src/lib.rs";

/// Operator labels (SSOT for log payloads + `Applied.operator`).
const OPERATOR_LIB_NAME: &str = "rust_fix_lib_name_for_integration_test";
const OPERATOR_CARGO_BIN_EXE_ENV: &str = "rust_fix_cargo_bin_exe_test_env";

/// Serde-family crates owned by [`super::cargo_dependency_repair`]; an
/// `use of undeclared crate or module` naming one of these is a missing
/// dependency, never a lib-name mismatch.
const CARGO_DEPENDENCY_OWNED_CRATES: &[&str] = &["serde", "serde_json"];

/// External-consumer surfaces that reference the lib crate by name (separate
/// compilation units). Scanned for symbol provenance + rename-conflict guards.
const EXTERNAL_CONSUMER_DIRS: &[&str] = &["tests", "examples", "benches", "src/bin"];

/// Bound on the workspace scan (file count + per-file bytes) so the operator
/// stays cheap and DoS-safe on adversarial trees.
const MAX_SCAN_FILES: usize = 256;
const MAX_SCAN_FILE_BYTES: u64 = VERIFIER_REPAIR_PASS_MAX_FILE_BYTES;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum RustBindingRepairOutcome {
    Applied {
        operator: &'static str,
        relative_path: String,
    },
    Skipped {
        reason: &'static str,
    },
}

/// Local, operator-scoped binding-check result. The lightweight analogue of
/// #988 Issue A's `EvidenceBindingPlan` until that lands.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum RustBindingMismatch {
    /// An integration test imports the lib crate as `imported`, but the lib's
    /// effective name differs. Fix: set `[lib] name = imported`.
    LibName { imported: String },
    /// A test references `CARGO_BIN_EXE_<referenced>` but the single real bin
    /// target is `actual_bin`. Fix: rename the env reference in `test_path`.
    CargoBinExeEnv {
        referenced: String,
        actual_bin: String,
        test_path: String,
    },
}

/// A validated, ready-to-apply deterministic binding edit.
struct PreparedBindingEdit {
    operator: &'static str,
    relative_path: String,
    canonical_path: PathBuf,
    original_contents: String,
    updated_contents: String,
    fingerprint: String,
    target_role: ArtifactRole,
    /// Human-facing target-hint reason (masked before logging).
    reason: String,
    /// Short edit description for the operator log (masked before logging).
    detail: String,
}

// ---------------------------------------------------------------------------
// Entry point
// ---------------------------------------------------------------------------

/// Run the deterministic Rust binding-repair registry for the active repair
/// job. Reads the failure diagnostic from `agent.repair_job`, prepares the
/// first applicable operator's edit, applies it through the standard
/// validated-repair-edit machinery, records it, and returns `Applied` so the
/// caller reruns the verifier. Any non-applicable / ambiguous condition returns
/// `Skipped` (the caller falls through to the LLM repair pass).
pub(super) fn try_apply_rust_binding_repair(agent: &mut Agent) -> RustBindingRepairOutcome {
    let Some(context) = agent.repair_job.clone() else {
        return skipped("missing_repair_job");
    };
    let diagnostic = binding_diagnostic_for_job(&context);

    let Some(mismatch) = detect_binding_mismatch(agent, &diagnostic) else {
        return skipped("no_binding_mismatch");
    };
    let Some(prepared) = build_binding_edit(&agent.work_root, &context, &mismatch) else {
        return skipped("not_applicable");
    };

    // Replay guard: even after a checkpoint rollback reverts the file, the
    // applied-intent ledger must prevent re-applying the same fingerprint.
    if validate_repair_intent_not_replayed(&context.applied_repair_intents, &prepared.fingerprint)
        .is_err()
    {
        log_binding_repair_skipped(
            agent,
            prepared.operator,
            &prepared.relative_path,
            "already_applied",
        );
        return skipped("already_applied");
    }

    apply_prepared(agent, prepared)
}

/// Classify the verifier failure into an unambiguous, deterministically
/// repairable Rust binding mismatch (the local analogue of #988 Issue A's
/// `EvidenceBindingPlan`). Operators are tried in registry order; the first
/// applicable, unambiguous match wins. `None` means defer to the LLM /
/// ContractConflictJob.
fn detect_binding_mismatch(agent: &Agent, diagnostic: &str) -> Option<RustBindingMismatch> {
    detect_lib_name_mismatch(agent, diagnostic)
        .or_else(|| detect_cargo_bin_exe_mismatch(agent, diagnostic))
}

/// Build the deterministic edit for a classified binding mismatch.
fn build_binding_edit(
    work_root: &Path,
    context: &super::repair_job::RepairJob,
    mismatch: &RustBindingMismatch,
) -> Option<PreparedBindingEdit> {
    match mismatch {
        RustBindingMismatch::LibName { imported } => {
            build_lib_name_edit(work_root, context, imported)
        }
        RustBindingMismatch::CargoBinExeEnv {
            referenced,
            actual_bin,
            test_path,
        } => build_cargo_bin_exe_edit(work_root, context, referenced, actual_bin, test_path),
    }
}

fn apply_prepared(agent: &mut Agent, prepared: PreparedBindingEdit) -> RustBindingRepairOutcome {
    let PreparedBindingEdit {
        operator,
        relative_path,
        canonical_path,
        original_contents,
        updated_contents,
        fingerprint,
        target_role,
        reason,
        detail,
    } = prepared;

    let target_hint = RecoveryTargetHint {
        role: target_role,
        path: relative_path.clone(),
        reason,
    };
    let edit = ValidatedVerifierRepairEdit::new(
        relative_path,
        canonical_path,
        &original_contents,
        updated_contents,
        fingerprint,
    );
    if let Err(err) = super::repair_patch_executor::apply_validated_repair_edit(&edit) {
        log_binding_repair_skipped(agent, operator, &edit.relative_path, "apply_failed");
        log_llm_event(
            "agent.verifier_rust_binding_repair.apply_failed",
            serde_json::json!({
                "session_id": agent.session_store.session_id(),
                "operator": operator,
                "path": mask_secrets(&edit.relative_path),
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
        "agent.verifier_rust_binding_repair.applied",
        serde_json::json!({
            "session_id": agent.session_store.session_id(),
            "operator": operator,
            "path": mask_secrets(&edit.relative_path),
            "detail": mask_secrets(&detail),
            "preimage_hash": edit.preimage_hash,
            "postimage_hash": edit.postimage_hash,
        }),
    );
    RustBindingRepairOutcome::Applied {
        operator,
        relative_path: edit.relative_path,
    }
}

fn skipped(reason: &'static str) -> RustBindingRepairOutcome {
    RustBindingRepairOutcome::Skipped { reason }
}

fn log_binding_repair_skipped(agent: &Agent, operator: &str, path: &str, reason: &'static str) {
    log_llm_event(
        "agent.verifier_rust_binding_repair.skipped",
        serde_json::json!({
            "session_id": agent.session_store.session_id(),
            "operator": operator,
            "path": mask_secrets(path),
            "reason": reason,
        }),
    );
}

/// Reconstruct the verifier diagnostic text from the repair job (error kind +
/// bounded output excerpt). Mirrors
/// `cargo_dependency_repair::cargo_dependency_diagnostic_for_job`.
fn binding_diagnostic_for_job(job: &super::repair_job::RepairJob) -> String {
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

// ---------------------------------------------------------------------------
// Operator 1 — rust_fix_lib_name_for_integration_test
// ---------------------------------------------------------------------------

fn detect_lib_name_mismatch(agent: &Agent, diagnostic: &str) -> Option<RustBindingMismatch> {
    // Exactly one undeclared crate name, or the binding direction is ambiguous.
    let undeclared = undeclared_crate_names(diagnostic);
    let [imported] = undeclared.as_slice() else {
        return None;
    };
    let imported = imported.clone();
    // `is_crate_identifier` is already enforced by `undeclared_crate_names`; the
    // serde-family allowlist belongs to `cargo_dependency_repair`.
    if CARGO_DEPENDENCY_OWNED_CRATES.contains(&imported.as_str()) {
        return None;
    }

    let (_canonical_path, manifest) =
        read_workspace_file(&agent.work_root, MANIFEST_RELATIVE_PATH)?;
    // A declared dependency named `imported` would be a missing-dependency
    // problem, not a lib-name mismatch.
    if declared_dependency_keys(&manifest).contains(&imported) {
        return None;
    }
    let current_lib_name = effective_lib_name(&manifest)?;
    if current_lib_name == imported {
        return None;
    }

    // Provenance: the lib must exist and publish at least one of the symbols the
    // tests import as `imported::<symbol>`. This proves `imported` denotes the
    // *local* lib (rejecting e.g. an absent external crate like `tokio`).
    let (_lib_path, lib_rs) = read_workspace_file(&agent.work_root, LIB_RS_RELATIVE_PATH)?;
    let pub_items = pub_item_names(&lib_rs);
    if pub_items.is_empty() {
        return None;
    }
    let imported_symbols = workspace_crate_import_symbols(&agent.work_root, &imported);
    if !imported_symbols.iter().any(|sym| pub_items.contains(sym)) {
        return None;
    }

    // Conflict guard: if any external-consumer file still references the current
    // lib name as a crate, renaming would break it — ambiguous, defer.
    if workspace_references_crate(&agent.work_root, &current_lib_name) {
        return None;
    }

    Some(RustBindingMismatch::LibName { imported })
}

fn build_lib_name_edit(
    work_root: &Path,
    context: &super::repair_job::RepairJob,
    imported: &str,
) -> Option<PreparedBindingEdit> {
    let (canonical_path, manifest) = read_workspace_file(work_root, MANIFEST_RELATIVE_PATH)?;
    let current_lib_name = effective_lib_name(&manifest)?;
    let updated = build_lib_name_manifest_edit(&manifest, imported)?;
    validate_repair_candidate_changed(&manifest, &updated).ok()?;

    let detail = format!("[lib] name {current_lib_name} -> {imported}");
    let edit_payload = [RepairIntentEdit {
        old_string: "<lib-name>",
        new_string: imported,
        replace_all: false,
    }];
    let fingerprint = repair_intent_edits_fingerprint(
        &context.failure_signature,
        MANIFEST_RELATIVE_PATH,
        &edit_payload,
    );
    Some(PreparedBindingEdit {
        operator: OPERATOR_LIB_NAME,
        relative_path: MANIFEST_RELATIVE_PATH.to_string(),
        canonical_path,
        original_contents: manifest,
        updated_contents: updated,
        fingerprint,
        target_role: ArtifactRole::Setup,
        reason: format!("align [lib] name with the integration test's import `{imported}`"),
        detail,
    })
}

/// Extract crate names reported as undeclared/unresolved at the *crate root*
/// (`use of undeclared crate or module \`X\``), in first-seen order, deduped.
/// Only the crate-root phrasing is matched, so `unresolved import \`foo::bar\``
/// (a missing item inside a resolved crate) is intentionally ignored.
pub(super) fn undeclared_crate_names(diagnostic: &str) -> Vec<String> {
    const SIGNAL: &str = "use of undeclared crate or module `";
    let mut names: Vec<String> = Vec::new();
    let mut rest = diagnostic;
    while let Some(pos) = rest.find(SIGNAL) {
        let after = &rest[pos + SIGNAL.len()..];
        if let Some(end) = after.find('`') {
            let name = &after[..end];
            if is_crate_identifier(name) && !names.iter().any(|n| n == name) {
                names.push(name.to_string());
            }
            rest = &after[end..];
        } else {
            break;
        }
    }
    names
}

/// Deterministically set `[lib] name = new_name` while preserving the manifest
/// shape:
/// - an existing `name = …` line under a bare `[lib]` table is rewritten,
/// - an existing `[lib]` table without a `name` gains one immediately after the
///   header,
/// - otherwise a new `[lib]` table is appended.
///
/// Returns `None` only when the manifest declares `[lib]` exclusively through a
/// `[lib.<sub>]` form with no bare header (ambiguous to edit safely).
pub(super) fn build_lib_name_manifest_edit(manifest: &str, new_name: &str) -> Option<String> {
    let scan = scan_lib_table(manifest);
    if scan.subtable_only {
        return None;
    }
    let trailing_newline = manifest.ends_with('\n');
    let replacement = format!("name = \"{new_name}\"");

    if let Some(name_line) = scan.name_line {
        let out = rebuild_lines(manifest, trailing_newline, |index, line| {
            if index == name_line {
                let indent: String = line.chars().take_while(|c| c.is_whitespace()).collect();
                vec![format!("{indent}{replacement}")]
            } else {
                vec![line.to_string()]
            }
        });
        return Some(out);
    }
    if let Some(header_line) = scan.header_line {
        let out = rebuild_lines(manifest, trailing_newline, |index, line| {
            if index == header_line {
                vec![line.to_string(), replacement.clone()]
            } else {
                vec![line.to_string()]
            }
        });
        return Some(out);
    }
    // No `[lib]` table at all: append one.
    let mut out = String::with_capacity(manifest.len() + replacement.len() + 16);
    out.push_str(manifest);
    if !manifest.is_empty() && !manifest.ends_with('\n') {
        out.push('\n');
    }
    if !manifest.is_empty() {
        out.push('\n');
    }
    out.push_str("[lib]\n");
    out.push_str(&replacement);
    out.push('\n');
    Some(out)
}

struct LibTableScan {
    /// Line index of a bare `[lib]` header, if present.
    header_line: Option<usize>,
    /// Line index of a `name = …` line within the bare `[lib]` table.
    name_line: Option<usize>,
    /// True if `[lib]` appears only as a `[lib.<sub>]` sub-table.
    subtable_only: bool,
}

fn scan_lib_table(manifest: &str) -> LibTableScan {
    let mut header_line = None;
    let mut name_line = None;
    let mut has_bare = false;
    let mut has_subtable = false;
    let mut in_bare_lib = false;
    for (index, raw) in manifest.lines().enumerate() {
        let line = strip_comment(raw).trim();
        if line.is_empty() {
            continue;
        }
        if let Some(header) = parse_table_header(line) {
            in_bare_lib = false;
            if header == "lib" {
                has_bare = true;
                if header_line.is_none() {
                    header_line = Some(index);
                }
                in_bare_lib = true;
            } else if header.strip_prefix("lib.").is_some() {
                has_subtable = true;
            }
            continue;
        }
        if in_bare_lib
            && name_line.is_none()
            && parse_assignment_key(line).as_deref() == Some("name")
        {
            name_line = Some(index);
        }
    }
    LibTableScan {
        header_line,
        name_line,
        subtable_only: has_subtable && !has_bare,
    }
}

// ---------------------------------------------------------------------------
// Operator 2 — rust_fix_cargo_bin_exe_test_env
// ---------------------------------------------------------------------------

fn detect_cargo_bin_exe_mismatch(agent: &Agent, diagnostic: &str) -> Option<RustBindingMismatch> {
    // Exactly one undefined CARGO_BIN_EXE name, else ambiguous.
    let referenced = match undefined_cargo_bin_exe_names(diagnostic).as_slice() {
        [single] => single.clone(),
        _ => return None,
    };

    let (_manifest_path, manifest) = read_workspace_file(&agent.work_root, MANIFEST_RELATIVE_PATH)?;
    let bins = bin_target_names(&manifest, &agent.work_root);
    // Exactly one real bin target to point the test at; `referenced` must not
    // already be one (otherwise the env var would be defined).
    let [actual_bin] = bins.as_slice() else {
        return None;
    };
    let actual_bin = actual_bin.clone();
    if actual_bin == referenced {
        return None;
    }

    // Locate the test file that errored from the diagnostic span, falling back
    // to a bounded scan; require exactly one file carrying the token.
    let token = format!("CARGO_BIN_EXE_{referenced}");
    let (test_rel, _canonical, _contents) =
        locate_cargo_bin_exe_test(&agent.work_root, diagnostic, &token)?;

    Some(RustBindingMismatch::CargoBinExeEnv {
        referenced,
        actual_bin,
        test_path: test_rel,
    })
}

fn build_cargo_bin_exe_edit(
    work_root: &Path,
    context: &super::repair_job::RepairJob,
    referenced: &str,
    actual_bin: &str,
    test_path: &str,
) -> Option<PreparedBindingEdit> {
    let (canonical_path, original) = read_workspace_file(work_root, test_path)?;
    let updated = rename_cargo_bin_exe(&original, referenced, actual_bin)?;
    validate_repair_candidate_changed(&original, &updated).ok()?;

    // Test-file edit: reject anything that reads as assertion weakening (a pure
    // env-token rename produces no weakening patterns).
    let weakening = detect_repair_candidate_weakening_patterns(test_path, &original, &updated);
    if !weakening.patterns.is_empty()
        && validate_repair_candidate_weakening_patterns(
            weakening.patterns,
            weakening.rejection_kind,
        )
        .is_err()
    {
        return None;
    }

    let token = format!("CARGO_BIN_EXE_{referenced}");
    let edit_payload = [RepairIntentEdit {
        old_string: &token,
        new_string: "<cargo-bin-exe>",
        replace_all: true,
    }];
    let fingerprint =
        repair_intent_edits_fingerprint(&context.failure_signature, test_path, &edit_payload);
    Some(PreparedBindingEdit {
        operator: OPERATOR_CARGO_BIN_EXE_ENV,
        relative_path: test_path.to_string(),
        canonical_path,
        original_contents: original,
        updated_contents: updated,
        fingerprint,
        target_role: ArtifactRole::Test,
        reason: format!("point CARGO_BIN_EXE reference at the built binary `{actual_bin}`"),
        detail: format!("CARGO_BIN_EXE_{referenced} -> CARGO_BIN_EXE_{actual_bin}"),
    })
}

/// Extract bin-target names referenced via an undefined `CARGO_BIN_EXE_<name>`
/// compile-time environment variable, in first-seen order, deduped.
pub(super) fn undefined_cargo_bin_exe_names(diagnostic: &str) -> Vec<String> {
    const PREFIX: &str = "CARGO_BIN_EXE_";
    let lower = diagnostic.to_ascii_lowercase();
    if !lower.contains("environment variable") || !lower.contains("not defined") {
        return Vec::new();
    }
    let mut names: Vec<String> = Vec::new();
    let mut rest = diagnostic;
    while let Some(pos) = rest.find(PREFIX) {
        let after = &rest[pos + PREFIX.len()..];
        let end = after
            .find(|c: char| !(c.is_ascii_alphanumeric() || c == '_' || c == '-'))
            .unwrap_or(after.len());
        let name = &after[..end];
        if is_bin_target_name(name) && !names.iter().any(|n| n == name) {
            names.push(name.to_string());
        }
        if end == 0 {
            // Avoid an infinite loop on a degenerate match.
            rest = &after[PREFIX.len().min(after.len())..];
        } else {
            rest = &after[end..];
        }
    }
    names
}

/// Rename every `CARGO_BIN_EXE_<from>` token to `CARGO_BIN_EXE_<to>`, respecting
/// the trailing token boundary so `CARGO_BIN_EXE_app` does not match inside
/// `CARGO_BIN_EXE_app2`. Returns `None` if no occurrence was rewritten.
pub(super) fn rename_cargo_bin_exe(src: &str, from: &str, to: &str) -> Option<String> {
    let needle = format!("CARGO_BIN_EXE_{from}");
    let mut out = String::with_capacity(src.len());
    let mut cursor = 0usize;
    let mut replaced = false;
    while let Some(rel) = src[cursor..].find(&needle) {
        let start = cursor + rel;
        let after = start + needle.len();
        let boundary_ok = src[after..]
            .chars()
            .next()
            .map(|c| !(c.is_ascii_alphanumeric() || c == '_' || c == '-'))
            .unwrap_or(true);
        out.push_str(&src[cursor..start]);
        if boundary_ok {
            out.push_str("CARGO_BIN_EXE_");
            out.push_str(to);
            replaced = true;
        } else {
            out.push_str(&needle);
        }
        cursor = after;
    }
    out.push_str(&src[cursor..]);
    replaced.then_some(out)
}

fn locate_cargo_bin_exe_test(
    work_root: &Path,
    diagnostic: &str,
    token: &str,
) -> Option<(String, PathBuf, String)> {
    // Prefer the `-->` span the compiler pointed at.
    for span in diagnostic_span_paths(diagnostic) {
        if !span.ends_with(".rs") {
            continue;
        }
        if let Some((canonical, contents)) = read_workspace_file(work_root, &span)
            && contents.contains(token)
        {
            return Some((span, canonical, contents));
        }
    }
    // Fall back to a bounded scan; require exactly one carrier to stay
    // deterministic.
    let mut matches: Vec<(String, PathBuf, String)> = scan_external_consumer_files(work_root)
        .into_iter()
        .filter(|(_, _, contents)| contents.contains(token))
        .collect();
    if matches.len() == 1 {
        return matches.pop();
    }
    None
}

/// Parse compiler `-->` span lines into their workspace-relative paths.
pub(super) fn diagnostic_span_paths(diagnostic: &str) -> Vec<String> {
    let mut paths: Vec<String> = Vec::new();
    for line in diagnostic.lines() {
        let Some(pos) = line.find("-->") else {
            continue;
        };
        let rest = line[pos + 3..].trim();
        // Strip the `:line:col` suffix; a bare path has no colon.
        let path = rest.split(':').next().unwrap_or(rest).trim();
        if !path.is_empty() && !paths.iter().any(|p| p == path) {
            paths.push(path.to_string());
        }
    }
    paths
}

// ---------------------------------------------------------------------------
// Manifest parsing (line-based, restricted to the shapes the operators act on)
// ---------------------------------------------------------------------------

/// The effective lib crate name: an explicit `[lib] name`, else the package
/// name with `-` normalized to `_` (Cargo's default lib name).
pub(super) fn effective_lib_name(manifest: &str) -> Option<String> {
    if let Some(name) = bare_table_string_value(manifest, "lib", "name") {
        return Some(name);
    }
    bare_table_string_value(manifest, "package", "name").map(|name| name.replace('-', "_"))
}

/// Real bin-target names: explicit `[[bin]] name`s, the default bin (the
/// package name when `src/main.rs` exists), and `src/bin/*.rs` stems. Sorted +
/// deduped for determinism.
pub(super) fn bin_target_names(manifest: &str, work_root: &Path) -> Vec<String> {
    let mut names: BTreeSet<String> = BTreeSet::new();
    names.extend(explicit_bin_names(manifest));
    if work_root.join("src/main.rs").is_file()
        && let Some(pkg) = bare_table_string_value(manifest, "package", "name")
    {
        names.insert(pkg);
    }
    if let Ok(entries) = std::fs::read_dir(work_root.join("src/bin")) {
        for entry in entries.flatten().take(MAX_SCAN_FILES) {
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) == Some("rs")
                && let Some(stem) = path.file_stem().and_then(|s| s.to_str())
                && is_bin_target_name(stem)
            {
                names.insert(stem.to_string());
            }
        }
    }
    names.into_iter().collect()
}

/// Names declared by `[[bin]]` array-of-tables entries.
fn explicit_bin_names(manifest: &str) -> Vec<String> {
    let mut names = Vec::new();
    let mut in_bin = false;
    for raw in manifest.lines() {
        let line = strip_comment(raw).trim();
        if line.is_empty() {
            continue;
        }
        if let Some(array_header) = parse_array_table_header(line) {
            in_bin = array_header == "bin";
            continue;
        }
        if parse_table_header(line).is_some() {
            in_bin = false;
            continue;
        }
        if in_bin
            && parse_assignment_key(line).as_deref() == Some("name")
            && let Some(value) = parse_assignment_string_value(line)
            && !names.contains(&value)
        {
            names.push(value);
        }
    }
    names
}

/// Crate keys declared under a bare `[dependencies]` table (including
/// `[dependencies.<name>]` sub-tables).
fn declared_dependency_keys(manifest: &str) -> Vec<String> {
    let mut keys = Vec::new();
    let mut in_bare = false;
    for raw in manifest.lines() {
        let line = strip_comment(raw).trim();
        if line.is_empty() {
            continue;
        }
        if let Some(header) = parse_table_header(line) {
            in_bare = header == "dependencies";
            if let Some(sub) = header.strip_prefix("dependencies.")
                && let Some(name) = sub.split('.').next()
            {
                let key = unquote(name);
                if !key.is_empty() && !keys.contains(&key) {
                    keys.push(key);
                }
            }
            continue;
        }
        if in_bare
            && let Some(key) = parse_assignment_key(line)
            && !keys.contains(&key)
        {
            keys.push(key);
        }
    }
    keys
}

fn bare_table_string_value(manifest: &str, table: &str, key: &str) -> Option<String> {
    let mut in_table = false;
    for raw in manifest.lines() {
        let line = strip_comment(raw).trim();
        if line.is_empty() {
            continue;
        }
        if let Some(header) = parse_table_header(line) {
            in_table = header == table;
            continue;
        }
        if in_table
            && parse_assignment_key(line).as_deref() == Some(key)
            && let Some(value) = parse_assignment_string_value(line)
        {
            return Some(value);
        }
    }
    None
}

fn parse_assignment_key(line: &str) -> Option<String> {
    let (key_part, _) = line.split_once('=')?;
    let key = unquote(key_part.trim());
    (!key.is_empty()).then_some(key)
}

fn parse_assignment_string_value(line: &str) -> Option<String> {
    let (_, value_part) = line.split_once('=')?;
    let value = value_part.trim();
    let inner = value
        .strip_prefix('"')
        .and_then(|rest| rest.strip_suffix('"'))
        .or_else(|| {
            value
                .strip_prefix('\'')
                .and_then(|rest| rest.strip_suffix('\''))
        })?;
    Some(inner.to_string())
}

/// Parse `[table.header]` into its dotted path, `None` for `[[array]]`.
fn parse_table_header(line: &str) -> Option<String> {
    let inner = line.strip_prefix('[')?.strip_suffix(']')?;
    if inner.starts_with('[') || inner.ends_with(']') {
        return None;
    }
    let trimmed = inner.trim();
    (!trimmed.is_empty()).then(|| trimmed.to_string())
}

/// Parse `[[array.header]]` into its dotted path, `None` otherwise.
fn parse_array_table_header(line: &str) -> Option<String> {
    let inner = line.strip_prefix("[[")?.strip_suffix("]]")?;
    let trimmed = inner.trim();
    (!trimmed.is_empty()).then(|| trimmed.to_string())
}

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

fn unquote(value: &str) -> String {
    let trimmed = value.trim();
    trimmed
        .strip_prefix('"')
        .and_then(|rest| rest.strip_suffix('"'))
        .or_else(|| {
            trimmed
                .strip_prefix('\'')
                .and_then(|rest| rest.strip_suffix('\''))
        })
        .unwrap_or(trimmed)
        .to_string()
}

// ---------------------------------------------------------------------------
// Rust source scanning (provenance + conflict guards)
// ---------------------------------------------------------------------------

/// Names of fully-`pub` items declared at the top level of a Rust source file.
/// Only fully public items (`pub`, not `pub(crate)`/`pub(super)`) are importable
/// from a separate crate, so only those count as binding provenance.
pub(super) fn pub_item_names(src: &str) -> BTreeSet<String> {
    const ITEM_KEYWORDS: &[&str] = &[
        "fn", "struct", "enum", "trait", "mod", "type", "union", "const", "static",
    ];
    const QUALIFIERS: &[&str] = &["async", "unsafe", "extern", "default"];
    let mut names = BTreeSet::new();
    for line in src.lines() {
        let trimmed = line.trim_start();
        let Some(rest) = trimmed.strip_prefix("pub ") else {
            continue;
        };
        let tokens = identifier_tokens(rest);
        let mut idx = 0;
        while idx < tokens.len() && QUALIFIERS.contains(&tokens[idx].as_str()) {
            idx += 1;
        }
        let Some(keyword) = tokens.get(idx) else {
            continue;
        };
        if !ITEM_KEYWORDS.contains(&keyword.as_str()) {
            continue;
        }
        // `const fn` / `static`-vs-`const`: a name token after `fn` wins.
        let name = if (keyword == "const" || keyword == "static")
            && tokens.get(idx + 1).map(String::as_str) == Some("fn")
        {
            tokens.get(idx + 2)
        } else {
            tokens.get(idx + 1)
        };
        if let Some(name) = name
            && is_crate_identifier(name)
        {
            names.insert(name.clone());
        }
    }
    names
}

/// First path segments referenced from `crate_name` across the workspace's
/// external-consumer files: `use crate_name::a;`, `use crate_name::{a, b};`,
/// and bare `crate_name::sym` paths. Glob (`use crate_name::*`) yields nothing.
fn workspace_crate_import_symbols(work_root: &Path, crate_name: &str) -> BTreeSet<String> {
    let mut symbols = BTreeSet::new();
    for (_, _, contents) in scan_external_consumer_files(work_root) {
        symbols.extend(crate_reference_symbols(&contents, crate_name));
    }
    symbols
}

/// First path segments referenced from `crate_name` in one source string.
pub(super) fn crate_reference_symbols(src: &str, crate_name: &str) -> BTreeSet<String> {
    let mut symbols = BTreeSet::new();
    let needle = format!("{crate_name}::");
    let bytes = src.as_bytes();
    let mut cursor = 0usize;
    while let Some(rel) = src[cursor..].find(&needle) {
        let start = cursor + rel;
        let after = start + needle.len();
        cursor = after;
        // Require a word boundary before the crate name so `mycrate::` does not
        // match the tail of `notmycrate::`.
        if start > 0 {
            let prev = bytes[start - 1];
            if prev == b'_' || prev.is_ascii_alphanumeric() || prev == b':' {
                continue;
            }
        }
        match src[after..].chars().next() {
            Some('{') => {
                if let Some(close) = src[after + 1..].find('}') {
                    let group = &src[after + 1..after + 1 + close];
                    for item in group.split(',') {
                        if let Some(sym) = leading_identifier(item.trim()) {
                            symbols.insert(sym);
                        }
                    }
                }
            }
            Some(_) => {
                if let Some(sym) = leading_identifier(&src[after..]) {
                    symbols.insert(sym);
                }
            }
            None => {}
        }
    }
    symbols
}

/// True if any external-consumer file references `crate_name` as a crate
/// (`crate_name::…`, `use crate_name;`, or `extern crate crate_name`).
fn workspace_references_crate(work_root: &Path, crate_name: &str) -> bool {
    scan_external_consumer_files(work_root)
        .iter()
        .any(|(_, _, contents)| source_references_crate(contents, crate_name))
}

/// True if `src` references `crate_name` as a crate root.
pub(super) fn source_references_crate(src: &str, crate_name: &str) -> bool {
    if !crate_reference_symbols(src, crate_name).is_empty() {
        return true;
    }
    let extern_decl = format!("extern crate {crate_name}");
    for line in src.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with(&extern_decl) {
            return true;
        }
        if let Some(rest) = trimmed.strip_prefix("use ") {
            let head = rest.trim_end_matches([';', ' ']).trim();
            if head == crate_name {
                return true;
            }
        }
    }
    false
}

fn leading_identifier(text: &str) -> Option<String> {
    let ident: String = text
        .chars()
        .take_while(|c| *c == '_' || c.is_ascii_alphanumeric())
        .collect();
    if ident.is_empty() || ident == "self" || ident == "crate" || ident == "super" {
        return None;
    }
    Some(ident)
}

fn identifier_tokens(text: &str) -> Vec<String> {
    let mut tokens = Vec::new();
    let mut current = String::new();
    for ch in text.chars() {
        if ch == '_' || ch.is_ascii_alphanumeric() {
            current.push(ch);
        } else if !current.is_empty() {
            tokens.push(std::mem::take(&mut current));
        }
    }
    if !current.is_empty() {
        tokens.push(current);
    }
    tokens
}

fn is_crate_identifier(name: &str) -> bool {
    !name.is_empty()
        && name.chars().all(|c| c == '_' || c.is_ascii_alphanumeric())
        && !name.chars().next().is_some_and(|c| c.is_ascii_digit())
}

fn is_bin_target_name(name: &str) -> bool {
    !name.is_empty()
        && name
            .chars()
            .all(|c| c == '_' || c == '-' || c.is_ascii_alphanumeric())
}

// ---------------------------------------------------------------------------
// Filesystem helpers (bounded)
// ---------------------------------------------------------------------------

fn read_workspace_file(work_root: &Path, relative: &str) -> Option<(PathBuf, String)> {
    let resolved = resolve_user_path(work_root, relative).ok()?;
    let canonical = std::fs::canonicalize(&resolved).ok()?;
    if !canonical.is_file() {
        return None;
    }
    let metadata = std::fs::metadata(&canonical).ok()?;
    if metadata.len() > MAX_SCAN_FILE_BYTES {
        return None;
    }
    let contents = std::fs::read_to_string(&canonical).ok()?;
    Some((canonical, contents))
}

/// Bounded collection of `.rs` files under the external-consumer directories,
/// as `(relative_path, canonical_path, contents)`. Capped by file count and
/// per-file bytes; deterministic order.
fn scan_external_consumer_files(work_root: &Path) -> Vec<(String, PathBuf, String)> {
    let mut out: Vec<(String, PathBuf, String)> = Vec::new();
    for dir in EXTERNAL_CONSUMER_DIRS {
        if out.len() >= MAX_SCAN_FILES {
            break;
        }
        collect_rs_files(work_root, &work_root.join(dir), dir, &mut out);
    }
    out
}

fn collect_rs_files(
    work_root: &Path,
    dir: &Path,
    relative_prefix: &str,
    out: &mut Vec<(String, PathBuf, String)>,
) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    let mut sorted: Vec<PathBuf> = entries.flatten().map(|e| e.path()).collect();
    sorted.sort();
    for path in sorted {
        if out.len() >= MAX_SCAN_FILES {
            return;
        }
        let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
            continue;
        };
        let child_rel = format!("{relative_prefix}/{name}");
        if path.is_dir() {
            collect_rs_files(work_root, &path, &child_rel, out);
            continue;
        }
        if path.extension().and_then(|e| e.to_str()) != Some("rs") {
            continue;
        }
        let Ok(metadata) = std::fs::metadata(&path) else {
            continue;
        };
        if metadata.len() > MAX_SCAN_FILE_BYTES {
            continue;
        }
        if let Ok(canonical) = std::fs::canonicalize(&path)
            && let Ok(contents) = std::fs::read_to_string(&canonical)
        {
            // Keep paths workspace-relative for fingerprints / target hints.
            let _ = work_root;
            out.push((child_rel, canonical, contents));
        }
    }
}

fn rebuild_lines<F>(manifest: &str, trailing_newline: bool, mut map: F) -> String
where
    F: FnMut(usize, &str) -> Vec<String>,
{
    let mut out = String::with_capacity(manifest.len() + 32);
    for (index, line) in manifest.lines().enumerate() {
        for produced in map(index, line) {
            out.push_str(&produced);
            out.push('\n');
        }
    }
    if !trailing_newline {
        out.pop();
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    // --- diagnostic parsing -------------------------------------------------

    #[test]
    fn extracts_single_undeclared_crate() {
        let diag = "error[E0432]: unresolved import `calculator`\n use of undeclared crate or module `calculator`";
        assert_eq!(undeclared_crate_names(diag), vec!["calculator".to_string()]);
    }

    #[test]
    fn extracts_multiple_distinct_undeclared_crates() {
        let diag =
            "use of undeclared crate or module `calc`\nuse of undeclared crate or module `mathlib`";
        assert_eq!(
            undeclared_crate_names(diag),
            vec!["calc".to_string(), "mathlib".to_string()]
        );
    }

    #[test]
    fn ignores_unresolved_item_within_resolved_crate() {
        // No "use of undeclared crate or module" phrasing -> not a crate-root miss.
        let diag = "error[E0432]: unresolved import `calc::missing`\n no `missing` in the root";
        assert!(undeclared_crate_names(diag).is_empty());
    }

    #[test]
    fn extracts_undefined_cargo_bin_exe_name() {
        let diag = "error: environment variable `CARGO_BIN_EXE_mytool` not defined at compile time\n --> tests/cli.rs:3:25";
        assert_eq!(
            undefined_cargo_bin_exe_names(diag),
            vec!["mytool".to_string()]
        );
    }

    #[test]
    fn cargo_bin_exe_requires_not_defined_signal() {
        let diag = "note: CARGO_BIN_EXE_mytool is documented here";
        assert!(undefined_cargo_bin_exe_names(diag).is_empty());
    }

    #[test]
    fn cargo_bin_exe_name_may_contain_dash() {
        let diag = "error: environment variable `CARGO_BIN_EXE_my-tool` not defined";
        assert_eq!(
            undefined_cargo_bin_exe_names(diag),
            vec!["my-tool".to_string()]
        );
    }

    #[test]
    fn parses_diagnostic_span_paths() {
        let diag = "error[E0432]\n --> tests/integration.rs:1:5\n note\n --> src/lib.rs:2:1";
        assert_eq!(
            diagnostic_span_paths(diag),
            vec!["tests/integration.rs".to_string(), "src/lib.rs".to_string()]
        );
    }

    // --- manifest scanning --------------------------------------------------

    #[test]
    fn effective_lib_name_uses_explicit_lib_name() {
        let manifest = "[package]\nname = \"rust-calc\"\n\n[lib]\nname = \"mathlib\"\n";
        assert_eq!(effective_lib_name(manifest), Some("mathlib".to_string()));
    }

    #[test]
    fn effective_lib_name_defaults_to_normalized_package_name() {
        let manifest = "[package]\nname = \"rust-calc\"\n";
        assert_eq!(effective_lib_name(manifest), Some("rust_calc".to_string()));
    }

    #[test]
    fn explicit_bin_names_are_collected() {
        let manifest = "[package]\nname = \"app\"\n\n[[bin]]\nname = \"cli\"\npath = \"src/cli.rs\"\n\n[[bin]]\nname = \"daemon\"\n";
        assert_eq!(explicit_bin_names(manifest), vec!["cli", "daemon"]);
    }

    #[test]
    fn declared_dependency_keys_include_subtables() {
        let manifest = "[dependencies]\nanyhow = \"1\"\n\n[dependencies.tokio]\nversion = \"1\"\n";
        let keys = declared_dependency_keys(manifest);
        assert!(keys.contains(&"anyhow".to_string()));
        assert!(keys.contains(&"tokio".to_string()));
    }

    // --- lib-name manifest edit --------------------------------------------

    #[test]
    fn rewrites_existing_lib_name_value() {
        let manifest = "[package]\nname = \"rust-calc\"\n\n[lib]\nname = \"rust_calc\"\n";
        let out = build_lib_name_manifest_edit(manifest, "calculator").unwrap();
        assert_eq!(
            out,
            "[package]\nname = \"rust-calc\"\n\n[lib]\nname = \"calculator\"\n"
        );
    }

    #[test]
    fn inserts_name_under_existing_lib_header() {
        let manifest = "[package]\nname = \"app\"\n\n[lib]\npath = \"src/lib.rs\"\n";
        let out = build_lib_name_manifest_edit(manifest, "calculator").unwrap();
        assert_eq!(
            out,
            "[package]\nname = \"app\"\n\n[lib]\nname = \"calculator\"\npath = \"src/lib.rs\"\n"
        );
    }

    #[test]
    fn appends_lib_table_when_absent() {
        let manifest = "[package]\nname = \"app\"\n";
        let out = build_lib_name_manifest_edit(manifest, "calculator").unwrap();
        assert_eq!(
            out,
            "[package]\nname = \"app\"\n\n[lib]\nname = \"calculator\"\n"
        );
    }

    #[test]
    fn lib_subtable_only_is_ambiguous() {
        let manifest = "[package]\nname = \"app\"\n\n[lib.metadata]\nfoo = \"bar\"\n";
        assert!(build_lib_name_manifest_edit(manifest, "calculator").is_none());
    }

    // --- cargo_bin_exe rename ----------------------------------------------

    #[test]
    fn renames_cargo_bin_exe_token() {
        let src = "let exe = env!(\"CARGO_BIN_EXE_mytool\");\n";
        let out = rename_cargo_bin_exe(src, "mytool", "my-tool").unwrap();
        assert_eq!(out, "let exe = env!(\"CARGO_BIN_EXE_my-tool\");\n");
    }

    #[test]
    fn rename_respects_trailing_token_boundary() {
        let src = "CARGO_BIN_EXE_app CARGO_BIN_EXE_app2";
        let out = rename_cargo_bin_exe(src, "app", "cli").unwrap();
        // Only the standalone `..._app` is renamed; `..._app2` is preserved.
        assert_eq!(out, "CARGO_BIN_EXE_cli CARGO_BIN_EXE_app2");
    }

    #[test]
    fn rename_returns_none_without_occurrence() {
        assert!(rename_cargo_bin_exe("no token here", "mytool", "cli").is_none());
    }

    // --- pub-item provenance ------------------------------------------------

    #[test]
    fn extracts_pub_item_names() {
        let src = "pub fn add(a: u32) -> u32 { a }\npub struct Calc;\npub const MAX: u32 = 9;\npub async fn run() {}\npub const fn zero() -> u32 { 0 }\nfn private_helper() {}\npub(crate) fn internal() {}\n";
        let items = pub_item_names(src);
        assert!(items.contains("add"));
        assert!(items.contains("Calc"));
        assert!(items.contains("MAX"));
        assert!(items.contains("run"));
        assert!(items.contains("zero"));
        assert!(!items.contains("private_helper"));
        // `pub(crate)` is not externally importable -> not provenance.
        assert!(!items.contains("internal"));
    }

    #[test]
    fn crate_reference_symbols_handles_use_forms() {
        let src = "use calculator::add;\nuse calculator::{sub, mul};\nfn f() { calculator::div(1, 2); }\n";
        let symbols = crate_reference_symbols(src, "calculator");
        assert!(symbols.contains("add"));
        assert!(symbols.contains("sub"));
        assert!(symbols.contains("mul"));
        assert!(symbols.contains("div"));
    }

    #[test]
    fn crate_reference_symbols_respects_word_boundary() {
        let src = "use notcalculator::add;\n";
        assert!(crate_reference_symbols(src, "calculator").is_empty());
    }

    #[test]
    fn glob_import_yields_no_named_symbol() {
        let src = "use calculator::*;\n";
        assert!(crate_reference_symbols(src, "calculator").is_empty());
    }

    #[test]
    fn source_references_crate_detects_extern_and_whole_use() {
        assert!(source_references_crate("extern crate calc;\n", "calc"));
        assert!(source_references_crate("use calc;\n", "calc"));
        assert!(source_references_crate("use calc::add;\n", "calc"));
        assert!(!source_references_crate("use other::calc;\n", "calc"));
    }

    #[test]
    fn identifier_validators() {
        assert!(is_crate_identifier("calculator"));
        assert!(is_crate_identifier("my_lib"));
        assert!(!is_crate_identifier("9lives"));
        assert!(!is_crate_identifier("has-dash"));
        assert!(is_bin_target_name("my-tool"));
        assert!(!is_bin_target_name(""));
    }

    // --- Agent-backed end-to-end (Ollama-free) ------------------------------

    mod agent_e2e {
        use super::super::*;
        use crate::agent::loop_run::commands::test_agent_with_config;
        use crate::agent::loop_run::repair_job::{RepairJob, RepairNextAction};
        use crate::config::Config;
        use crate::session::store::ConversationMessage;

        fn agent_with(files: &[(&str, &str)]) -> (Agent, tempfile::TempDir) {
            let (mut agent, temp) = test_agent_with_config(Config::default());
            agent.session.messages.push(ConversationMessage::user(
                "Create a Rust library `calculator` with an integration test and verify with cargo test."
                    .to_string(),
            ));
            for (rel, contents) in files {
                let path = agent.work_root.join(rel);
                if let Some(parent) = path.parent() {
                    std::fs::create_dir_all(parent).unwrap();
                }
                std::fs::write(path, contents).unwrap();
            }
            (agent, temp)
        }

        fn job_with(error_kind: &str, output_excerpt: &str, signature: &str) -> RepairJob {
            let mut job = RepairJob::new_for_test();
            job.command = "cargo test".to_string();
            job.output_excerpt = output_excerpt.to_string();
            job.failure_signature = signature.to_string();
            job.error_kind = Some(error_kind.to_string());
            job
        }

        #[test]
        fn lib_name_mismatch_is_repaired_deterministically() {
            let (mut agent, _temp) = agent_with(&[
                (
                    "Cargo.toml",
                    "[package]\nname = \"rust-calc\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
                ),
                (
                    "src/lib.rs",
                    "pub fn add(a: u32, b: u32) -> u32 { a + b }\n",
                ),
                (
                    "tests/integration.rs",
                    "use calculator::add;\n#[test]\nfn adds() { assert_eq!(add(1, 2), 3); }\n",
                ),
            ]);
            agent.repair_job = Some(job_with(
                "error[E0432]",
                "error[E0432]: unresolved import `calculator`\n --> tests/integration.rs:1:5\n  |\n1 | use calculator::add;\n  |     ^^^^^^^^^^ use of undeclared crate or module `calculator`",
                "tests/integration.rs E0432 calculator",
            ));

            let outcome = try_apply_rust_binding_repair(&mut agent);

            assert_eq!(
                outcome,
                RustBindingRepairOutcome::Applied {
                    operator: OPERATOR_LIB_NAME,
                    relative_path: "Cargo.toml".to_string(),
                }
            );
            let manifest = std::fs::read_to_string(agent.work_root.join("Cargo.toml")).unwrap();
            assert!(manifest.contains("[lib]"));
            assert!(manifest.contains("name = \"calculator\""));
            // The verifier reruns next.
            assert_eq!(
                agent.repair_job.as_ref().unwrap().next_action(),
                RepairNextAction::RerunVerifier
            );
        }

        #[test]
        fn lib_name_operator_is_idempotent_once_aligned() {
            let (mut agent, _temp) = agent_with(&[
                (
                    "Cargo.toml",
                    "[package]\nname = \"rust-calc\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
                ),
                (
                    "src/lib.rs",
                    "pub fn add(a: u32, b: u32) -> u32 { a + b }\n",
                ),
                (
                    "tests/integration.rs",
                    "use calculator::add;\n#[test]\nfn adds() { assert_eq!(add(1, 2), 3); }\n",
                ),
            ]);
            agent.repair_job = Some(job_with(
                "error[E0432]",
                "use of undeclared crate or module `calculator`\n --> tests/integration.rs:1:5",
                "tests/integration.rs E0432 calculator",
            ));

            let first = try_apply_rust_binding_repair(&mut agent);
            assert!(matches!(first, RustBindingRepairOutcome::Applied { .. }));

            // The lib is now named `calculator`, so the same failure has no
            // mismatch left; the operator cannot churn.
            let second = try_apply_rust_binding_repair(&mut agent);
            assert_eq!(
                second,
                RustBindingRepairOutcome::Skipped {
                    reason: "no_binding_mismatch"
                }
            );
        }

        #[test]
        fn external_missing_dependency_is_not_a_lib_name_mismatch() {
            // The test imports `tokio`, which the local lib does not provide.
            // Provenance fails -> defer (it is a missing dependency, not a
            // binding mismatch).
            let (mut agent, _temp) = agent_with(&[
                (
                    "Cargo.toml",
                    "[package]\nname = \"app\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
                ),
                (
                    "src/lib.rs",
                    "pub fn add(a: u32, b: u32) -> u32 { a + b }\n",
                ),
                (
                    "tests/integration.rs",
                    "use tokio::spawn;\n#[test]\nfn t() { spawn(async {}); }\n",
                ),
            ]);
            agent.repair_job = Some(job_with(
                "error[E0432]",
                "use of undeclared crate or module `tokio`\n --> tests/integration.rs:1:5",
                "tests/integration.rs E0432 tokio",
            ));

            assert_eq!(
                try_apply_rust_binding_repair(&mut agent),
                RustBindingRepairOutcome::Skipped {
                    reason: "no_binding_mismatch"
                }
            );
        }

        #[test]
        fn lib_name_rename_conflict_defers() {
            // A second integration test still imports the current lib name
            // (`rust_calc`), so renaming would break it -> ambiguous -> defer.
            let (mut agent, _temp) = agent_with(&[
                (
                    "Cargo.toml",
                    "[package]\nname = \"rust-calc\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
                ),
                (
                    "src/lib.rs",
                    "pub fn add(a: u32, b: u32) -> u32 { a + b }\n",
                ),
                ("tests/a.rs", "use calculator::add;\n"),
                ("tests/b.rs", "use rust_calc::add;\n"),
            ]);
            agent.repair_job = Some(job_with(
                "error[E0432]",
                "use of undeclared crate or module `calculator`\n --> tests/a.rs:1:5",
                "tests/a.rs E0432 calculator",
            ));

            assert_eq!(
                try_apply_rust_binding_repair(&mut agent),
                RustBindingRepairOutcome::Skipped {
                    reason: "no_binding_mismatch"
                }
            );
        }

        #[test]
        fn cargo_bin_exe_mismatch_is_repaired_deterministically() {
            let (mut agent, _temp) = agent_with(&[
                (
                    "Cargo.toml",
                    "[package]\nname = \"app\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
                ),
                ("src/main.rs", "fn main() { println!(\"hi\"); }\n"),
                (
                    "tests/cli.rs",
                    "#[test]\nfn runs() {\n    let exe = env!(\"CARGO_BIN_EXE_mytool\");\n    assert!(!exe.is_empty());\n}\n",
                ),
            ]);
            agent.repair_job = Some(job_with(
                "error",
                "error: environment variable `CARGO_BIN_EXE_mytool` not defined at compile time\n --> tests/cli.rs:3:15",
                "tests/cli.rs CARGO_BIN_EXE_mytool",
            ));

            let outcome = try_apply_rust_binding_repair(&mut agent);

            assert_eq!(
                outcome,
                RustBindingRepairOutcome::Applied {
                    operator: OPERATOR_CARGO_BIN_EXE_ENV,
                    relative_path: "tests/cli.rs".to_string(),
                }
            );
            let test = std::fs::read_to_string(agent.work_root.join("tests/cli.rs")).unwrap();
            assert!(test.contains("CARGO_BIN_EXE_app"));
            assert!(!test.contains("CARGO_BIN_EXE_mytool"));
            // The assertion is preserved verbatim.
            assert!(test.contains("assert!(!exe.is_empty());"));
            assert_eq!(
                agent.repair_job.as_ref().unwrap().next_action(),
                RepairNextAction::RerunVerifier
            );
        }

        #[test]
        fn cargo_bin_exe_ambiguous_multiple_bins_defers() {
            let (mut agent, _temp) = agent_with(&[
                (
                    "Cargo.toml",
                    "[package]\nname = \"app\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\n[[bin]]\nname = \"cli\"\npath = \"src/cli.rs\"\n\n[[bin]]\nname = \"daemon\"\npath = \"src/daemon.rs\"\n",
                ),
                ("src/cli.rs", "fn main() {}\n"),
                ("src/daemon.rs", "fn main() {}\n"),
                (
                    "tests/cli.rs",
                    "#[test]\nfn runs() {\n    let exe = env!(\"CARGO_BIN_EXE_mytool\");\n    assert!(!exe.is_empty());\n}\n",
                ),
            ]);
            agent.repair_job = Some(job_with(
                "error",
                "error: environment variable `CARGO_BIN_EXE_mytool` not defined at compile time\n --> tests/cli.rs:3:15",
                "tests/cli.rs CARGO_BIN_EXE_mytool",
            ));

            assert_eq!(
                try_apply_rust_binding_repair(&mut agent),
                RustBindingRepairOutcome::Skipped {
                    reason: "no_binding_mismatch"
                }
            );
        }

        #[test]
        fn unrelated_diagnostic_is_skipped() {
            let (mut agent, _temp) = agent_with(&[
                (
                    "Cargo.toml",
                    "[package]\nname = \"app\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
                ),
                (
                    "src/lib.rs",
                    "pub fn add(a: u32, b: u32) -> u32 { a + b }\n",
                ),
            ]);
            agent.repair_job = Some(job_with(
                "error[E0308]",
                "error[E0308]: mismatched types\n expected `u32`, found `String`",
                "src/lib.rs E0308",
            ));

            assert_eq!(
                try_apply_rust_binding_repair(&mut agent),
                RustBindingRepairOutcome::Skipped {
                    reason: "no_binding_mismatch"
                }
            );
        }
    }
}
