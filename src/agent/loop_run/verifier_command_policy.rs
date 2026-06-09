//! Verifier command admission policy.
//!
//! This module owns small deterministic command-shape allowlists. Semantic
//! interpretation stays in ProjectProfile / ObjectiveContract; this file only
//! decides whether an already-proposed command shape is safe enough to admit as
//! a verifier hint.

pub(super) fn admitted_profile_preferred_runner(runner: Option<&str>) -> Option<&'static str> {
    match runner?.trim().to_ascii_lowercase().as_str() {
        "cargo test" => Some("cargo test"),
        "npm test" => Some("npm test"),
        "pytest" => Some("pytest"),
        "python -m unittest discover -s tests" => Some("python -m unittest discover -s tests"),
        "python3 -m unittest discover -s tests" => Some("python3 -m unittest discover -s tests"),
        _ => None,
    }
}

pub(super) fn canonical_project_unit_evidence_command(
    command: Option<&str>,
) -> Option<&'static str> {
    match admitted_profile_preferred_runner(command) {
        Some("python -m unittest discover -s tests")
        | Some("python3 -m unittest discover -s tests") => {
            Some("python3 -m unittest discover -s tests")
        }
        _ => None,
    }
}

pub(super) fn is_evidence_command_hint_allowed(command: &str) -> bool {
    if super::completion_evidence::contains_evidence_poisoning_shell_control(command) {
        return false;
    }
    let lower = command.trim().to_ascii_lowercase();
    [
        "cargo test",
        "cargo build",
        "cargo check",
        "cargo clippy",
        "npm test",
        "npm run test",
        "npm run build",
        "python -m pytest",
        "python3 -m pytest",
        "python -m unittest discover -s tests",
        "python3 -m unittest discover -s tests",
        "pytest",
    ]
    .iter()
    .any(|prefix| lower == *prefix || lower.starts_with(&format!("{prefix} ")))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn profile_runner_admission_accepts_known_verifier_shapes() {
        assert_eq!(
            admitted_profile_preferred_runner(Some("python -m unittest discover -s tests")),
            Some("python -m unittest discover -s tests")
        );
        assert_eq!(
            admitted_profile_preferred_runner(Some("  cargo test  ")),
            Some("cargo test")
        );
    }

    #[test]
    fn profile_runner_admission_rejects_non_verifier_or_shell_shapes() {
        assert_eq!(admitted_profile_preferred_runner(Some("echo ok")), None);
        assert_eq!(
            admitted_profile_preferred_runner(Some("python -m unittest discover -s tests || true")),
            None
        );
    }

    #[test]
    fn project_unit_evidence_command_canonicalizes_unittest_runner() {
        assert_eq!(
            canonical_project_unit_evidence_command(Some("python -m unittest discover -s tests")),
            Some("python3 -m unittest discover -s tests")
        );
    }

    #[test]
    fn evidence_command_hint_allowlist_accepts_project_verifier_prefixes() {
        assert!(is_evidence_command_hint_allowed(
            "cargo test --manifest-path Cargo.toml"
        ));
        assert!(is_evidence_command_hint_allowed(
            "python3 -m unittest discover -s tests"
        ));
        assert!(!is_evidence_command_hint_allowed("echo ok"));
        assert!(!is_evidence_command_hint_allowed("cargo test || true"));
    }
}
