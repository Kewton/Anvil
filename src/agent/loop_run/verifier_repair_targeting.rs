use std::collections::HashSet;
use std::path::Path;

use crate::agent::orchestration::RepoVerification;
use crate::safety::path_guard::resolve_user_path;
use crate::util::workspace_paths::is_ignored_workspace_display_path;

use super::repair_framework_findings::{
    missing_python_module_name_from_output, output_or_command_looks_like_pytest,
    workspace_implementation_imports_python_module,
};
use super::repair_target_admission::{RepairTargetAdmissionContext, admit_repair_target_hint};

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct VerifierRepairTargetCandidate {
    pub(super) hint: super::task_contract::RecoveryTargetHint,
    pub(super) line: Option<usize>,
    score: usize,
    ordinal: usize,
}

pub(super) fn changed_files_for_verifier(
    accumulated: &[RepoVerification],
    current: &RepoVerification,
) -> Vec<String> {
    let mut files = HashSet::new();
    for verif in accumulated.iter().chain(std::iter::once(current)) {
        for file in &verif.all_changed_files {
            if is_ignored_workspace_display_path(file) {
                continue;
            }
            files.insert(file.clone());
        }
    }
    let mut files: Vec<String> = files.into_iter().collect();
    files.sort();
    files
}

pub(super) fn extract_path_like_tokens(text: &str) -> impl Iterator<Item = &str> {
    text.split(|c: char| c.is_whitespace() || c == ':' || c == '"' || c == '\'')
        .map(|t| t.trim_matches(|c: char| matches!(c, '(' | ')' | ',' | ';')))
        .filter(|t| {
            !t.is_empty()
                && t.contains('/')
                && (t.contains(".rs")
                    || t.contains(".py")
                    || t.contains(".ts")
                    || t.contains(".tsx")
                    || t.contains(".js")
                    || t.contains(".jsx")
                    || t.contains(".go")
                    || t.contains(".java")
                    || t.contains(".toml")
                    || t.contains(".json"))
        })
}

pub(super) fn verifier_diagnostic_path_input_is_safe(raw_path: &str) -> bool {
    let path = raw_path.trim();
    if path.is_empty()
        || path.contains('\0')
        || Path::new(path).is_absolute()
        || is_ignored_workspace_display_path(path)
    {
        return false;
    }
    !Path::new(path)
        .components()
        .any(|component| matches!(component, std::path::Component::ParentDir))
}

pub(super) fn diagnostic_missing_setup_path_is_controller_writable(path: &str) -> bool {
    matches!(path, "pyproject.toml")
}

pub(super) fn recovery_target_hint_for_existing_path(
    work_root: &Path,
    raw_path: &str,
    reason: &str,
) -> Option<super::task_contract::RecoveryTargetHint> {
    if !verifier_diagnostic_path_input_is_safe(raw_path) {
        return None;
    }
    let resolved = resolve_user_path(work_root, raw_path).ok()?;
    if !resolved.is_file() {
        return None;
    }
    let root = std::fs::canonicalize(work_root).unwrap_or_else(|_| work_root.to_path_buf());
    let canonical = std::fs::canonicalize(&resolved).ok()?;
    if !canonical.is_file() {
        return None;
    }
    let relative = canonical.strip_prefix(root).ok()?;
    let path = relative.to_string_lossy().replace('\\', "/");
    if is_ignored_workspace_display_path(&path) {
        return None;
    }
    let category = super::completion_evidence::classify_repo_edit_path(Path::new(&path));
    let role = super::task_contract::role_from_repo_edit(category)?;
    Some(super::task_contract::RecoveryTargetHint {
        role,
        path,
        reason: reason.to_string(),
    })
}

pub(super) fn recovery_target_hint_for_diagnostic_path(
    work_root: &Path,
    raw_path: &str,
    reason: &str,
    failure_kind: super::VerifierDiagnosticFailureKind,
    admission: &RepairTargetAdmissionContext<'_>,
) -> Option<super::task_contract::RecoveryTargetHint> {
    if let Some(hint) = recovery_target_hint_for_existing_path(work_root, raw_path, reason) {
        if hint.role == super::task_contract::ArtifactRole::Setup
            && !failure_kind.allows_setup_target()
        {
            return None;
        }
        // Issue #647 (§5.1 stage 2): Owned admission gate at the function exit.
        // Path 1 of the 6 source categories: diagnostic LLM-proposed paths.
        return admit_repair_target_hint(hint, admission);
    }
    recovery_target_hint_for_missing_setup_path(
        work_root,
        raw_path,
        reason,
        failure_kind,
        admission,
    )
}

pub(super) fn recovery_target_hint_for_missing_setup_path(
    work_root: &Path,
    raw_path: &str,
    reason: &str,
    failure_kind: super::VerifierDiagnosticFailureKind,
    admission: &RepairTargetAdmissionContext<'_>,
) -> Option<super::task_contract::RecoveryTargetHint> {
    if !failure_kind.allows_setup_target() {
        return None;
    }
    let path = raw_path.trim().replace('\\', "/");
    if !verifier_diagnostic_path_input_is_safe(&path)
        || !admission.scope.contains(&path)
        || !diagnostic_missing_setup_path_is_controller_writable(&path)
    {
        return None;
    }
    let resolved = resolve_user_path(work_root, &path).ok()?;
    if resolved.exists() {
        return None;
    }
    if !super::artifact_ownership::nearest_existing_ancestor_within_work_root(work_root, &resolved)
    {
        return None;
    }
    Some(super::task_contract::RecoveryTargetHint {
        role: super::task_contract::ArtifactRole::Setup,
        path,
        reason: reason.to_string(),
    })
}

pub(super) fn verifier_diagnostic_missing_setup_candidates(
    work_root: &Path,
    context: &super::repair_job::RepairJob,
    active_request: &str,
) -> Vec<super::task_contract::RecoveryTargetHint> {
    if !python_verifier_output_missing_external_dependency(work_root, context) {
        return Vec::new();
    }
    let scope = super::task_workspace_scope::TaskWorkspaceScope::detect(work_root, active_request);
    let no_prior_edit = |_: &str| false;
    let admission = RepairTargetAdmissionContext {
        work_root,
        scope: &scope,
        edited_this_session_for: &no_prior_edit,
        scaffold_changed_for: &no_prior_edit,
    };
    recovery_target_hint_for_missing_setup_path(
        work_root,
        "pyproject.toml",
        "Python verifier cannot import a third-party dependency and no setup manifest is available",
        super::VerifierDiagnosticFailureKind::DependencyMissing,
        &admission,
    )
    .into_iter()
    .collect()
}

pub(super) fn python_verifier_output_missing_external_dependency(
    work_root: &Path,
    context: &super::repair_job::RepairJob,
) -> bool {
    if !output_or_command_looks_like_pytest(&context.command, &context.output_excerpt) {
        return false;
    }
    python_missing_external_dependency_name(work_root, &context.output_excerpt).is_some()
}

pub(super) fn recovery_target_hint_for_missing_local_module_path(
    work_root: &Path,
    raw_path: &str,
    reason: &str,
    admission: &RepairTargetAdmissionContext<'_>,
) -> Option<super::task_contract::RecoveryTargetHint> {
    let path = raw_path.trim();
    if !verifier_diagnostic_path_input_is_safe(path) {
        return None;
    }
    if Path::new(path)
        .components()
        .any(|component| match component {
            std::path::Component::Normal(name) => {
                super::task_workspace_scope::is_workspace_ignored_dir(&name.to_string_lossy())
            }
            _ => false,
        })
    {
        return None;
    }
    if Path::new(path)
        .extension()
        .and_then(|ext| ext.to_str())
        .is_none_or(|ext| !ext.eq_ignore_ascii_case("py"))
    {
        return None;
    }
    if !admission.scope.contains(path) {
        return None;
    }
    let resolved = resolve_user_path(work_root, path).ok()?;
    if resolved.exists() {
        return recovery_target_hint_for_existing_path(work_root, path, reason);
    }
    if !super::artifact_ownership::nearest_existing_ancestor_within_work_root(work_root, &resolved)
    {
        return None;
    }
    Some(super::task_contract::RecoveryTargetHint {
        role: super::task_contract::ArtifactRole::Implementation,
        path: path.to_string(),
        reason: reason.to_string(),
    })
}

pub(super) fn verifier_repair_missing_local_module_provider(
    context: &super::repair_job::RepairJob,
    derived_failure_type: super::VerifierFailureType,
    admission: &RepairTargetAdmissionContext<'_>,
) -> Option<super::task_contract::RecoveryTargetHint> {
    if derived_failure_type != super::VerifierFailureType::ImportOrDependency {
        return None;
    }
    let module = missing_python_module_name_from_output(&context.output_excerpt)?;
    if !workspace_implementation_imports_python_module(admission.work_root, &module) {
        return None;
    }
    let path = missing_python_module_workspace_path(admission.work_root, &module)?;
    recovery_target_hint_for_missing_local_module_path(
        admission.work_root,
        &path,
        "verifier output names a missing local module provider",
        admission,
    )
}

pub(super) fn verifier_repair_preferred_local_import_source(
    context: &super::repair_job::RepairJob,
    // Issue #638 (設計判断 #3): caller passes the assessment-derived failure type
    // so this helper is not gated on `context.failure_type` (which is `Unknown`
    // after the parser scope reduction in Task 1.2). Production callers MUST pass
    // `verifier_failure_type_for_diagnostic_kind(failure_kind, context.failure_type)`.
    derived_failure_type: super::VerifierFailureType,
    admission: &RepairTargetAdmissionContext<'_>,
) -> Option<super::task_contract::RecoveryTargetHint> {
    if derived_failure_type != super::VerifierFailureType::ImportOrDependency {
        return None;
    }
    let lower = context.output_excerpt.to_ascii_lowercase();
    let local_import_mismatch = lower.contains("cannot import name")
        || lower.contains("unresolved import")
        || lower.contains("has no exported member")
        || lower.contains("attempted import error")
        || lower.contains("is not exported from");
    if !local_import_mismatch {
        return None;
    }
    let hint = context.target_hint.as_ref()?;
    let promoted = if hint.role == super::task_contract::ArtifactRole::Implementation {
        Some(super::task_contract::RecoveryTargetHint {
            reason: "local import contract mismatch names this provider/source file".to_string(),
            ..hint.clone()
        })
    } else {
        None
    }?;
    // Issue #647 (§5.1 stage 2): Owned admission gate. Path 4 of the
    // 6 source categories: local-import-contract-derived hints.
    admit_repair_target_hint(promoted, admission)
}

pub(super) fn verifier_repair_stale_assertion_test_target(
    context: &super::repair_job::RepairJob,
    selected_path: Option<&str>,
    // Issue #638 (設計判断 #3): caller passes the assessment-derived failure type
    // so this helper is not gated on `context.failure_type` (which is `Unknown`
    // after the parser scope reduction in Task 1.2). Production callers MUST pass
    // `verifier_failure_type_for_diagnostic_kind(failure_kind, context.failure_type)`.
    derived_failure_type: super::VerifierFailureType,
    admission: &RepairTargetAdmissionContext<'_>,
) -> Option<super::task_contract::RecoveryTargetHint> {
    if derived_failure_type != super::VerifierFailureType::AssertionFailure {
        return None;
    }
    let previous_non_test_repair_was_unresolved = matches!(
        context.rerun_outcome,
        Some(
            super::VerifierRepairRerunOutcome::SameFailureRemaining
                | super::VerifierRepairRerunOutcome::Worsened
                | super::VerifierRepairRerunOutcome::Improved
        )
    );
    if !previous_non_test_repair_was_unresolved {
        return None;
    }
    let previous_target = context.repair_target_hint.as_ref()?;
    if previous_target.role == super::task_contract::ArtifactRole::Test {
        return None;
    }
    if selected_path.is_some_and(|path| path != previous_target.path) {
        return None;
    }
    let failure_target = context.target_hint.as_ref()?;
    if failure_target.role != super::task_contract::ArtifactRole::Test
        || failure_target.path == previous_target.path
    {
        return None;
    }
    let promoted = super::task_contract::RecoveryTargetHint {
        reason: "same assertion failure remained after a non-test repair; inspect generated test setup or expectations".to_string(),
        ..failure_target.clone()
    };
    // Issue #647 (§5.1 stage 2): Owned admission gate. Path 5 of the
    // 6 source categories: stale-assertion test re-target.
    admit_repair_target_hint(promoted, admission)
}

pub(super) fn python_missing_external_dependency_name(
    work_root: &Path,
    output: &str,
) -> Option<String> {
    for line in output.lines() {
        let lower = line.to_ascii_lowercase();
        let Some(marker_index) = lower.find("no module named") else {
            continue;
        };
        let candidate = line[marker_index + "no module named".len()..].trim();
        let candidate = candidate
            .trim_matches(|ch: char| {
                ch == '\''
                    || ch == '"'
                    || ch == ':'
                    || ch == '.'
                    || ch == ','
                    || ch == '`'
                    || ch == ' '
            })
            .split_whitespace()
            .next()
            .unwrap_or_default()
            .trim_matches(|ch: char| {
                ch == '\'' || ch == '"' || ch == ':' || ch == '.' || ch == ','
            });
        if !python_dependency_module_name_is_safe(candidate) {
            continue;
        }
        let top_level = candidate.split('.').next().unwrap_or_default();
        if top_level.is_empty()
            || work_root.join(format!("{top_level}.py")).is_file()
            || work_root.join(top_level).is_dir()
        {
            continue;
        }
        return Some(candidate.to_string());
    }
    None
}

fn python_dependency_module_name_is_safe(module: &str) -> bool {
    !module.is_empty()
        && !module.starts_with('.')
        && module.split('.').all(|part| {
            let mut chars = part.chars();
            let Some(first) = chars.next() else {
                return false;
            };
            (first == '_' || first.is_ascii_alphabetic())
                && chars.all(|ch| ch == '_' || ch.is_ascii_alphanumeric())
        })
}

pub(super) fn missing_python_module_workspace_path(
    work_root: &Path,
    module: &str,
) -> Option<String> {
    let mut parts = module.split('.');
    let top = parts.next()?;
    if !work_root.join(top).is_dir() {
        return None;
    }
    if !work_root.join(top).join("__init__.py").is_file() {
        return None;
    }
    let relative = format!("{}.py", module.replace('.', "/"));
    let parent = Path::new(&relative).parent()?;
    if !work_root.join(parent).is_dir() {
        return None;
    }
    Some(relative)
}

#[cfg(test)]
pub(super) fn verifier_repair_target_hint_from_output(
    work_root: &Path,
    output: &str,
    changed_files: &[String],
) -> Option<super::task_contract::RecoveryTargetHint> {
    verifier_repair_target_candidate_from_output(work_root, output, changed_files)
        .map(|candidate| candidate.hint)
}

pub(super) fn verifier_repair_target_candidate_from_output(
    work_root: &Path,
    output: &str,
    changed_files: &[String],
) -> Option<VerifierRepairTargetCandidate> {
    let mut candidates = Vec::<VerifierRepairTargetCandidate>::new();
    let mut ordinal = 0usize;

    for line in output.lines() {
        if verifier_output_line_is_non_fatal_warning(line) {
            continue;
        }
        for path in extract_path_like_tokens(line) {
            if let Some(candidate) =
                verifier_repair_candidate_from_path(work_root, path, line, true, ordinal)
            {
                insert_verifier_repair_candidate(&mut candidates, candidate);
                ordinal = ordinal.saturating_add(1);
            }
        }
    }

    for path in changed_files {
        if let Some(candidate) =
            verifier_repair_candidate_from_path(work_root, path, "", false, ordinal)
        {
            insert_verifier_repair_candidate(&mut candidates, candidate);
            ordinal = ordinal.saturating_add(1);
        }
    }

    candidates.into_iter().max_by(|a, b| {
        a.score
            .cmp(&b.score)
            .then_with(|| b.ordinal.cmp(&a.ordinal))
    })
}

pub(super) fn verifier_repair_changed_file_hints(
    work_root: &Path,
    changed_files: &[String],
) -> Vec<super::task_contract::RecoveryTargetHint> {
    let mut seen = HashSet::new();
    let mut hints = Vec::new();
    for (ordinal, path) in changed_files.iter().enumerate() {
        let Some(candidate) =
            verifier_repair_candidate_from_path(work_root, path, "", false, ordinal)
        else {
            continue;
        };
        if seen.insert(candidate.hint.path.clone()) {
            hints.push(super::task_contract::RecoveryTargetHint {
                reason: "changed workspace file is a possible verifier repair target".to_string(),
                ..candidate.hint
            });
        }
    }
    hints
}

fn verifier_repair_candidate_from_path(
    work_root: &Path,
    raw_path: &str,
    source_line: &str,
    from_verifier_output: bool,
    ordinal: usize,
) -> Option<VerifierRepairTargetCandidate> {
    let Ok(resolved) = resolve_user_path(work_root, raw_path) else {
        return None;
    };
    if !resolved.is_file() {
        return None;
    }
    let canonical_root = work_root.canonicalize().ok();
    let relative = if let Some(root) = canonical_root.as_ref() {
        resolved.strip_prefix(root).ok()
    } else {
        resolved.strip_prefix(work_root).ok()
    }?;
    let path = relative.to_string_lossy().replace('\\', "/");
    if is_ignored_workspace_display_path(&path) {
        return None;
    }
    let category = super::completion_evidence::classify_repo_edit_path(Path::new(&path));
    let role = super::task_contract::role_from_repo_edit(category)?;
    let line = from_verifier_output
        .then(|| verifier_line_number_for_path(source_line, raw_path))
        .flatten();
    let role_score = match role {
        super::task_contract::ArtifactRole::Implementation => 30,
        super::task_contract::ArtifactRole::Setup => 25,
        super::task_contract::ArtifactRole::Test => 15,
        super::task_contract::ArtifactRole::UsageDocs => 5,
    };
    let import_provider_score = if from_verifier_output
        && role == super::task_contract::ArtifactRole::Implementation
        && verifier_line_names_import_provider(source_line)
    {
        40
    } else {
        0
    };
    let score = usize::from(from_verifier_output) * 50
        + usize::from(line.is_some()) * 30
        + role_score
        + import_provider_score;
    Some(VerifierRepairTargetCandidate {
        hint: super::task_contract::RecoveryTargetHint {
            role,
            path,
            reason: "verifier output or changed files identify this artifact as repair target"
                .to_string(),
        },
        line,
        score,
        ordinal,
    })
}

fn verifier_line_names_import_provider(line: &str) -> bool {
    let lower = line.to_ascii_lowercase();
    lower.contains("cannot import name")
        || lower.contains("unresolved import")
        || lower.contains("has no exported member")
        || lower.contains("attempted import error")
        || lower.contains("is not exported from")
}

fn insert_verifier_repair_candidate(
    candidates: &mut Vec<VerifierRepairTargetCandidate>,
    candidate: VerifierRepairTargetCandidate,
) {
    if let Some(existing) = candidates
        .iter_mut()
        .find(|existing| existing.hint.path == candidate.hint.path)
    {
        if candidate.score > existing.score
            || (candidate.score == existing.score && candidate.ordinal < existing.ordinal)
        {
            *existing = candidate;
        }
        return;
    }
    candidates.push(candidate);
}

fn verifier_output_line_is_non_fatal_warning(line: &str) -> bool {
    let trimmed = line.trim_start();
    let lower = trimmed.to_ascii_lowercase();
    (lower.contains("warning") || lower.contains("warnings summary"))
        && !lower.starts_with("error")
        && !lower.starts_with("failed ")
        && !lower.starts_with("e   ")
        && !lower.starts_with("e ")
        && !lower.starts_with("thread '")
}

fn verifier_line_number_for_path(line: &str, raw_path: &str) -> Option<usize> {
    let idx = line.find(raw_path)?;
    let rest = &line[idx + raw_path.len()..];
    if let Some(number) = rest.strip_prefix(':').and_then(parse_leading_usize) {
        return Some(number);
    }
    rest.find("line ")
        .and_then(|idx| parse_leading_usize(&rest[idx + "line ".len()..]))
}

fn parse_leading_usize(input: &str) -> Option<usize> {
    let digits = input
        .chars()
        .take_while(|ch| ch.is_ascii_digit())
        .collect::<String>();
    if digits.is_empty() {
        None
    } else {
        digits.parse().ok()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn verifier_diagnostic_path_input_rejects_unsafe_shapes() {
        assert!(!verifier_diagnostic_path_input_is_safe(""));
        assert!(!verifier_diagnostic_path_input_is_safe("../app/main.py"));
        assert!(!verifier_diagnostic_path_input_is_safe("/tmp/app/main.py"));
        assert!(!verifier_diagnostic_path_input_is_safe("app/with\0nul.py"));
        assert!(!verifier_diagnostic_path_input_is_safe(
            ".anvil-state/sessions/job.json"
        ));
        assert!(verifier_diagnostic_path_input_is_safe("app/main.py"));
    }

    #[test]
    fn recovery_target_hint_for_existing_path_classifies_safe_existing_files() {
        let temp = tempdir().unwrap();
        let work_root = temp.path();
        std::fs::create_dir_all(work_root.join("app")).unwrap();
        std::fs::create_dir_all(work_root.join("tests")).unwrap();
        std::fs::write(work_root.join("app/main.py"), "x = 1\n").unwrap();
        std::fs::write(work_root.join("tests/test_main.py"), "def test_x(): pass\n").unwrap();

        let implementation =
            recovery_target_hint_for_existing_path(work_root, "app/main.py", "reason").unwrap();
        assert_eq!(implementation.path, "app/main.py");
        assert_eq!(
            implementation.role,
            super::super::task_contract::ArtifactRole::Implementation
        );

        let test =
            recovery_target_hint_for_existing_path(work_root, "tests/test_main.py", "reason")
                .unwrap();
        assert_eq!(test.path, "tests/test_main.py");
        assert_eq!(test.role, super::super::task_contract::ArtifactRole::Test);
    }

    #[test]
    fn recovery_target_hint_for_existing_path_rejects_unsafe_or_ignored_paths() {
        let temp = tempdir().unwrap();
        let work_root = temp.path();
        std::fs::create_dir_all(work_root.join(".anvil-state")).unwrap();
        std::fs::write(work_root.join(".anvil-state/session.json"), "{}").unwrap();

        assert!(
            recovery_target_hint_for_existing_path(work_root, "../outside.py", "reason").is_none()
        );
        assert!(
            recovery_target_hint_for_existing_path(
                work_root,
                ".anvil-state/session.json",
                "reason"
            )
            .is_none()
        );
        assert!(
            recovery_target_hint_for_existing_path(work_root, "missing.py", "reason").is_none()
        );
    }

    #[test]
    fn recovery_target_hint_for_diagnostic_path_admits_owned_existing_targets() {
        let temp = tempdir().unwrap();
        let work_root = temp.path();
        std::fs::create_dir_all(work_root.join("app")).unwrap();
        std::fs::write(work_root.join("app/main.py"), "x = 1\n").unwrap();
        let scope = super::super::task_workspace_scope::TaskWorkspaceScope::detect(work_root, "");
        let edited = |path: &str| path == "app/main.py";
        let unchanged = |_: &str| false;
        let admission = RepairTargetAdmissionContext {
            work_root,
            scope: &scope,
            edited_this_session_for: &edited,
            scaffold_changed_for: &unchanged,
        };

        let hint = recovery_target_hint_for_diagnostic_path(
            work_root,
            "app/main.py",
            "reason",
            super::super::VerifierDiagnosticFailureKind::RuntimeError,
            &admission,
        )
        .unwrap();

        assert_eq!(hint.path, "app/main.py");
        assert_eq!(
            hint.role,
            super::super::task_contract::ArtifactRole::Implementation
        );
    }

    #[test]
    fn recovery_target_hint_for_diagnostic_path_rejects_unowned_existing_targets() {
        let temp = tempdir().unwrap();
        let work_root = temp.path();
        std::fs::create_dir_all(work_root.join("app")).unwrap();
        std::fs::write(work_root.join("app/main.py"), "x = 1\n").unwrap();
        let scope = super::super::task_workspace_scope::TaskWorkspaceScope::detect(work_root, "");
        let unchanged = |_: &str| false;
        let admission = RepairTargetAdmissionContext {
            work_root,
            scope: &scope,
            edited_this_session_for: &unchanged,
            scaffold_changed_for: &unchanged,
        };

        assert!(
            recovery_target_hint_for_diagnostic_path(
                work_root,
                "app/main.py",
                "reason",
                super::super::VerifierDiagnosticFailureKind::RuntimeError,
                &admission,
            )
            .is_none()
        );
    }

    #[test]
    fn recovery_target_hint_for_missing_setup_path_requires_setup_failure_and_manifest() {
        let temp = tempdir().unwrap();
        let work_root = temp.path();
        let scope = super::super::task_workspace_scope::TaskWorkspaceScope::detect(work_root, "");
        let unchanged = |_: &str| false;
        let admission = RepairTargetAdmissionContext {
            work_root,
            scope: &scope,
            edited_this_session_for: &unchanged,
            scaffold_changed_for: &unchanged,
        };

        let hint = recovery_target_hint_for_missing_setup_path(
            work_root,
            "pyproject.toml",
            "reason",
            super::super::VerifierDiagnosticFailureKind::DependencyMissing,
            &admission,
        )
        .unwrap();

        assert_eq!(hint.path, "pyproject.toml");
        assert_eq!(hint.role, super::super::task_contract::ArtifactRole::Setup);
        assert!(
            recovery_target_hint_for_missing_setup_path(
                work_root,
                "pyproject.toml",
                "reason",
                super::super::VerifierDiagnosticFailureKind::RuntimeError,
                &admission,
            )
            .is_none()
        );
        assert!(
            recovery_target_hint_for_missing_setup_path(
                work_root,
                "requirements.txt",
                "reason",
                super::super::VerifierDiagnosticFailureKind::DependencyMissing,
                &admission,
            )
            .is_none()
        );
    }

    #[test]
    fn verifier_diagnostic_missing_setup_candidates_adds_pyproject_for_pytest_dependency() {
        let temp = tempdir().unwrap();
        let work_root = temp.path();
        let context = super::super::repair_job::RepairJob {
            command: "python3 -m pytest".to_string(),
            output_excerpt: "ModuleNotFoundError: No module named 'fastapi'".to_string(),
            ..super::super::repair_job::RepairJob::new_for_test()
        };

        let hints = verifier_diagnostic_missing_setup_candidates(work_root, &context, "");

        assert_eq!(hints.len(), 1);
        assert_eq!(hints[0].path, "pyproject.toml");
        assert_eq!(
            hints[0].role,
            super::super::task_contract::ArtifactRole::Setup
        );
    }

    #[test]
    fn verifier_diagnostic_missing_setup_candidates_ignores_non_pytest_or_local_modules() {
        let temp = tempdir().unwrap();
        let work_root = temp.path();
        std::fs::write(work_root.join("localpkg.py"), "").unwrap();
        let non_pytest = super::super::repair_job::RepairJob {
            command: "cargo test".to_string(),
            output_excerpt: "ModuleNotFoundError: No module named 'fastapi'".to_string(),
            ..super::super::repair_job::RepairJob::new_for_test()
        };
        let local_module = super::super::repair_job::RepairJob {
            command: "python3 -m pytest".to_string(),
            output_excerpt: "ModuleNotFoundError: No module named 'localpkg'".to_string(),
            ..super::super::repair_job::RepairJob::new_for_test()
        };

        assert!(
            verifier_diagnostic_missing_setup_candidates(work_root, &non_pytest, "").is_empty()
        );
        assert!(
            verifier_diagnostic_missing_setup_candidates(work_root, &local_module, "").is_empty()
        );
    }

    #[test]
    fn verifier_repair_missing_local_module_provider_targets_prospective_impl_file() {
        let temp = tempdir().unwrap();
        let work_root = temp.path();
        std::fs::create_dir_all(work_root.join("app")).unwrap();
        std::fs::write(work_root.join("app/__init__.py"), "").unwrap();
        std::fs::write(
            work_root.join("app/main.py"),
            "from app.database import db\n",
        )
        .unwrap();
        let scope = super::super::task_workspace_scope::TaskWorkspaceScope::detect(work_root, "");
        let admission = RepairTargetAdmissionContext::owned_for_test(work_root, &scope);
        let context = super::super::repair_job::RepairJob {
            output_excerpt: "ModuleNotFoundError: No module named 'app.database'".to_string(),
            ..super::super::repair_job::RepairJob::new_for_test()
        };

        let hint = verifier_repair_missing_local_module_provider(
            &context,
            super::super::VerifierFailureType::ImportOrDependency,
            &admission,
        )
        .unwrap();

        assert_eq!(hint.path, "app/database.py");
        assert_eq!(
            hint.role,
            super::super::task_contract::ArtifactRole::Implementation
        );
    }

    #[test]
    fn verifier_repair_missing_local_module_provider_ignores_test_only_imports() {
        let temp = tempdir().unwrap();
        let work_root = temp.path();
        std::fs::create_dir_all(work_root.join("app")).unwrap();
        std::fs::create_dir_all(work_root.join("tests")).unwrap();
        std::fs::write(work_root.join("app/__init__.py"), "").unwrap();
        std::fs::write(
            work_root.join("app/main.py"),
            "from fastapi import FastAPI\n",
        )
        .unwrap();
        std::fs::write(
            work_root.join("tests/test_main.py"),
            "from app.database import SessionLocal\n",
        )
        .unwrap();
        let scope = super::super::task_workspace_scope::TaskWorkspaceScope::detect(work_root, "");
        let admission = RepairTargetAdmissionContext::owned_for_test(work_root, &scope);
        let context = super::super::repair_job::RepairJob {
            output_excerpt: "ModuleNotFoundError: No module named 'app.database'".to_string(),
            ..super::super::repair_job::RepairJob::new_for_test()
        };

        assert!(
            verifier_repair_missing_local_module_provider(
                &context,
                super::super::VerifierFailureType::ImportOrDependency,
                &admission,
            )
            .is_none()
        );
    }

    #[test]
    fn verifier_repair_preferred_local_import_source_promotes_owned_impl_target() {
        let temp = tempdir().unwrap();
        let work_root = temp.path();
        std::fs::create_dir_all(work_root.join("app")).unwrap();
        std::fs::write(work_root.join("app/main.py"), "from app import missing\n").unwrap();
        let scope = super::super::task_workspace_scope::TaskWorkspaceScope::detect(work_root, "");
        let admission = RepairTargetAdmissionContext::owned_for_test(work_root, &scope);
        let hint = super::super::task_contract::RecoveryTargetHint {
            role: super::super::task_contract::ArtifactRole::Implementation,
            path: "app/main.py".to_string(),
            reason: "changed impl".to_string(),
        };
        let context = super::super::repair_job::RepairJob {
            output_excerpt: "ImportError: cannot import name 'missing' from 'app'".to_string(),
            target_hint: Some(hint),
            ..super::super::repair_job::RepairJob::new_for_test()
        };

        let promoted = verifier_repair_preferred_local_import_source(
            &context,
            super::super::VerifierFailureType::ImportOrDependency,
            &admission,
        )
        .unwrap();

        assert_eq!(promoted.path, "app/main.py");
        assert_eq!(
            promoted.reason,
            "local import contract mismatch names this provider/source file"
        );
    }

    #[test]
    fn verifier_repair_stale_assertion_test_target_promotes_owned_test_after_unresolved_impl_repair()
     {
        let temp = tempdir().unwrap();
        let work_root = temp.path();
        std::fs::create_dir_all(work_root.join("app")).unwrap();
        std::fs::create_dir_all(work_root.join("tests")).unwrap();
        std::fs::write(work_root.join("app/main.py"), "items = []\n").unwrap();
        std::fs::write(
            work_root.join("tests/test_main.py"),
            "def test_items(): pass\n",
        )
        .unwrap();
        let scope = super::super::task_workspace_scope::TaskWorkspaceScope::detect(work_root, "");
        let admission = RepairTargetAdmissionContext::owned_for_test(work_root, &scope);
        let impl_hint = super::super::task_contract::RecoveryTargetHint {
            role: super::super::task_contract::ArtifactRole::Implementation,
            path: "app/main.py".to_string(),
            reason: "previous target".to_string(),
        };
        let test_hint = super::super::task_contract::RecoveryTargetHint {
            role: super::super::task_contract::ArtifactRole::Test,
            path: "tests/test_main.py".to_string(),
            reason: "failure target".to_string(),
        };
        let context = super::super::repair_job::RepairJob {
            repair_target_hint: Some(impl_hint.clone()),
            target_hint: Some(test_hint),
            rerun_outcome: Some(super::super::VerifierRepairRerunOutcome::SameFailureRemaining),
            ..super::super::repair_job::RepairJob::new_for_test()
        };

        let promoted = verifier_repair_stale_assertion_test_target(
            &context,
            Some("app/main.py"),
            super::super::VerifierFailureType::AssertionFailure,
            &admission,
        )
        .unwrap();

        assert_eq!(promoted.path, "tests/test_main.py");
        assert_eq!(impl_hint.path, "app/main.py");
    }

    #[test]
    fn python_missing_external_dependency_ignores_local_modules() {
        let temp = tempdir().unwrap();
        let work_root = temp.path();
        std::fs::write(work_root.join("localpkg.py"), "").unwrap();

        assert_eq!(
            python_missing_external_dependency_name(
                work_root,
                "ModuleNotFoundError: No module named 'requests'"
            )
            .as_deref(),
            Some("requests")
        );
        assert_eq!(
            python_missing_external_dependency_name(
                work_root,
                "ModuleNotFoundError: No module named 'localpkg'"
            ),
            None
        );
        assert_eq!(
            python_missing_external_dependency_name(
                work_root,
                "ModuleNotFoundError: No module named '../unsafe'"
            ),
            None
        );
    }

    #[test]
    fn missing_python_module_workspace_path_requires_package_parent() {
        let temp = tempdir().unwrap();
        let work_root = temp.path();
        std::fs::create_dir_all(work_root.join("app").join("services")).unwrap();
        std::fs::write(work_root.join("app").join("__init__.py"), "").unwrap();

        assert_eq!(
            missing_python_module_workspace_path(work_root, "app.services.worker").as_deref(),
            Some("app/services/worker.py")
        );
        assert_eq!(
            missing_python_module_workspace_path(work_root, "app.missing.worker"),
            None
        );
        assert_eq!(
            missing_python_module_workspace_path(work_root, "other.services.worker"),
            None
        );
    }
}
