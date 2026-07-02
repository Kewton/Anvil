//! Setup-artifact candidate validation for controller-applied repair.
//!
//! This module validates already-proposed file contents. It does not infer task
//! intent and does not parse verifier output. The goal is to catch destructive
//! or structurally invalid setup artifacts before the full evidence runner is
//! asked to rerun.

use std::path::Path;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum SetupArtifactValidationError {
    EmptyTarget,
    InvalidCargoManifest(String),
    InvalidPackageJson(String),
}

impl SetupArtifactValidationError {
    pub(super) fn message(&self, relative_path: &str) -> String {
        match self {
            Self::EmptyTarget => {
                format!("repair candidate must not empty target file {relative_path}")
            }
            Self::InvalidCargoManifest(detail) => {
                format!("repair candidate Cargo.toml is invalid: {detail}")
            }
            Self::InvalidPackageJson(detail) => {
                format!("repair candidate package.json is invalid: {detail}")
            }
        }
    }
}

pub(super) fn validate_setup_artifact_candidate(
    relative_path: &str,
    candidate_contents: &str,
) -> Result<(), SetupArtifactValidationError> {
    if candidate_contents.trim().is_empty() {
        return Err(SetupArtifactValidationError::EmptyTarget);
    }

    match Path::new(relative_path)
        .file_name()
        .and_then(|name| name.to_str())
    {
        Some("Cargo.toml") => validate_cargo_manifest(candidate_contents),
        Some("package.json") => validate_package_json(candidate_contents),
        _ => Ok(()),
    }
}

fn validate_cargo_manifest(source: &str) -> Result<(), SetupArtifactValidationError> {
    super::cargo_manifest_summary::cargo_manifest_readiness(source)
        .map(|_| ())
        .map_err(|err| {
            SetupArtifactValidationError::InvalidCargoManifest(err.message().to_string())
        })
}

fn validate_package_json(source: &str) -> Result<(), SetupArtifactValidationError> {
    super::package_manifest_summary::package_manifest_readiness(source)
        .map(|_| ())
        .map_err(|err| SetupArtifactValidationError::InvalidPackageJson(err.message().to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_empty_candidate_for_any_target() {
        let err = validate_setup_artifact_candidate("README.md", " \n").unwrap_err();
        assert_eq!(
            err.message("README.md"),
            "repair candidate must not empty target file README.md"
        );
    }

    #[test]
    fn accepts_minimal_package_cargo_manifest() {
        assert!(
            validate_setup_artifact_candidate(
                "Cargo.toml",
                "[package]\nname = \"demo\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
            )
            .is_ok()
        );
    }

    #[test]
    fn accepts_workspace_only_cargo_manifest() {
        assert!(validate_setup_artifact_candidate("Cargo.toml", "[workspace]\n").is_ok());
    }

    #[test]
    fn rejects_virtual_manifest_with_package_target_sections() {
        let err = validate_setup_artifact_candidate(
            "Cargo.toml",
            "[[bench]]\nname = \"benches\"\nharness = false\n",
        )
        .unwrap_err();
        assert_eq!(
            err.message("Cargo.toml"),
            "repair candidate Cargo.toml is invalid: target sections require a [package] section"
        );
    }

    #[test]
    fn rejects_package_manifest_without_name() {
        let err =
            validate_setup_artifact_candidate("Cargo.toml", "[package]\nversion = \"0.1.0\"\n")
                .unwrap_err();
        assert_eq!(
            err.message("Cargo.toml"),
            "repair candidate Cargo.toml is invalid: [package] does not declare a non-empty name"
        );
    }

    #[test]
    fn rejects_malformed_package_name() {
        let err = validate_setup_artifact_candidate("Cargo.toml", "[package]\nname = demo\n")
            .unwrap_err();
        assert_eq!(
            err.message("Cargo.toml"),
            "repair candidate Cargo.toml is invalid: [package] name is malformed"
        );
    }

    #[test]
    fn accepts_package_json_object() {
        assert!(
            validate_setup_artifact_candidate(
                "package.json",
                r#"{"scripts":{"test":"node test.js"}}"#
            )
            .is_ok()
        );
    }

    #[test]
    fn rejects_malformed_package_json() {
        let err = validate_setup_artifact_candidate("package.json", r#"{"scripts":"#).unwrap_err();
        assert_eq!(
            err.message("package.json"),
            "repair candidate package.json is invalid: not valid JSON"
        );
    }

    #[test]
    fn rejects_non_object_package_json() {
        let err = validate_setup_artifact_candidate("package.json", "[]").unwrap_err();
        assert_eq!(
            err.message("package.json"),
            "repair candidate package.json is invalid: top-level value must be an object"
        );
    }

    #[test]
    fn rejects_non_object_package_json_scripts() {
        let err =
            validate_setup_artifact_candidate("package.json", r#"{"scripts":"oops"}"#).unwrap_err();
        assert_eq!(
            err.message("package.json"),
            "repair candidate package.json is invalid: scripts must be an object when declared"
        );
    }
}
