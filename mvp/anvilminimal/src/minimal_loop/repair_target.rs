use crate::minimal_loop::evidence::{SatisfactionChannel, evidence_satisfaction_channel};
use crate::planner::verify::VerificationReport;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RepairTarget {
    DependencySetup,
    PackageConfig,
    FrameworkConfig,
    MissingEntrypoint,
    EmptyApp,
    CapabilityMissing,
    RequiredEvidenceMissing,
    Implementation,
    TestOrEvidence,
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RepairFollowThrough {
    NoChange,
    TargetMatched,
    TargetNotFollowed,
    UnrelatedChange,
}

impl RepairFollowThrough {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::NoChange => "no_change",
            Self::TargetMatched => "target_matched",
            Self::TargetNotFollowed => "target_not_followed",
            Self::UnrelatedChange => "unrelated_change",
        }
    }

    /// Telemetry label for follow-through outcomes.
    ///
    /// This classifier is heuristic and must not gate repair-loop termination;
    /// termination belongs to deterministic verification and repair-progress
    /// verdicts.
    pub fn failure_kind(self) -> Option<&'static str> {
        match self {
            Self::NoChange => Some("verify_repair_no_change"),
            Self::TargetNotFollowed => Some("repair_target_not_followed"),
            Self::UnrelatedChange => Some("repair_unrelated_change"),
            Self::TargetMatched => None,
        }
    }

    pub fn followed(self) -> bool {
        matches!(self, Self::TargetMatched)
    }
}

impl RepairTarget {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::DependencySetup => "dependency_setup",
            Self::PackageConfig => "package_config",
            Self::FrameworkConfig => "framework_config",
            Self::MissingEntrypoint => "missing_entrypoint",
            Self::EmptyApp => "empty_app",
            Self::CapabilityMissing => "capability_missing",
            Self::RequiredEvidenceMissing => "required_evidence_missing",
            Self::Implementation => "implementation",
            Self::TestOrEvidence => "test_or_evidence",
            Self::Unknown => "unknown",
        }
    }

    pub fn guidance(self) -> &'static str {
        match self {
            Self::DependencySetup => {
                "Install or restore project dependencies before retrying build verification."
            }
            Self::PackageConfig => {
                "Fix dependency versions, scripts, and package metadata before editing app code."
            }
            Self::FrameworkConfig => {
                "Fix framework configuration, routing boundaries, or generated type declarations."
            }
            Self::MissingEntrypoint => {
                "Create or restore the executable entrypoint required by the selected profile."
            }
            Self::EmptyApp => {
                "Replace metadata-only or static shell output with real application behavior."
            }
            Self::CapabilityMissing => {
                "Implement the missing user-facing capability required by the goal or contract."
            }
            Self::RequiredEvidenceMissing => {
                "Add deterministic source, test, or verification evidence for the requested behavior."
            }
            Self::Implementation => {
                "Fix the implementation files that should satisfy the requested behavior."
            }
            Self::TestOrEvidence => {
                "Add or strengthen deterministic evidence that verifies the requested behavior."
            }
            Self::Unknown => {
                "Inspect the verification reason and edit the smallest relevant file set."
            }
        }
    }

    pub fn allowed_action(self) -> &'static str {
        match self {
            Self::DependencySetup => "edit_setup_manifest_or_installable_dependency_artifact",
            Self::PackageConfig => "edit_package_manifest_or_lockfile",
            Self::FrameworkConfig => "edit_framework_configuration_or_route_boundary",
            Self::MissingEntrypoint => "create_missing_entrypoint_artifact",
            Self::EmptyApp | Self::CapabilityMissing | Self::Implementation => {
                "edit_task_implementation_artifact"
            }
            Self::RequiredEvidenceMissing | Self::TestOrEvidence => {
                "edit_or_create_verification_evidence"
            }
            Self::Unknown => "edit_smallest_relevant_workspace_artifact",
        }
    }
}

pub fn classify_repair_target(report: &VerificationReport) -> RepairTarget {
    if !report.compile_errors.is_empty() {
        return RepairTarget::Implementation;
    }
    if !report.dependency_missing.is_empty() {
        return RepairTarget::DependencySetup;
    }
    if report.profile_failures.iter().any(|reason| {
        contains_any(
            reason,
            &[
                "probe_dependency_missing",
                "probe_infrastructure_failed",
                "app interaction untested",
            ],
        )
    }) {
        return RepairTarget::Unknown;
    }
    if report.profile_failures.iter().any(|reason| {
        contains_any(
            reason,
            &[
                "package.json",
                "dependency",
                "dependencies",
                "devdependencies",
                "scripts.build",
                "scripts.dev",
                "react",
                "typescript",
                "next 15",
                "build-safe",
            ],
        )
    }) {
        return RepairTarget::PackageConfig;
    }
    if report.profile_failures.iter().any(|reason| {
        contains_any(
            reason,
            &[
                "tsconfig",
                "tailwind",
                "tailwind_dev_pipeline_failure",
                "css",
                "http_500",
                "layout",
                "use client",
                "global.d.ts",
                "module resolution",
                "app router",
                "alias",
                "declare module",
            ],
        )
    }) {
        return RepairTarget::FrameworkConfig;
    }
    if report.profile_failures.iter().any(|reason| {
        contains_any(
            reason,
            &[
                "entrypoint missing",
                "next entrypoint missing",
                "missing entrypoint",
                "src/app/page",
                "pages/index",
            ],
        )
    }) || report
        .missing_paths
        .iter()
        .any(|path| contains_any(path, &["src/app/page", "app/page", "pages/index"]))
    {
        return RepairTarget::MissingEntrypoint;
    }
    if report.profile_failures.iter().any(|reason| {
        contains_any(
            reason,
            &[
                "empty app",
                "metadata-only",
                "static shell",
                "static_title_only",
                "build-only",
            ],
        )
    }) {
        return RepairTarget::EmptyApp;
    }
    if report
        .profile_failures
        .iter()
        .any(|reason| missing_required_evidence_includes_source_scan(reason))
    {
        return RepairTarget::CapabilityMissing;
    }
    if report.profile_failures.iter().any(|reason| {
        contains_any(
            reason,
            &[
                "interactive_ui_source_evidence",
                "non_static_screen_evidence",
                "browser_interaction_failed",
                "missing_required_capabilities",
                "plan_output_missing_required_capabilities",
                "capability missing",
            ],
        )
    }) {
        return RepairTarget::CapabilityMissing;
    }
    if report.profile_failures.iter().any(|reason| {
        contains_any(
            reason,
            &[
                "missing_required_evidence",
                "weak_verification_evidence",
                "inconclusive_acceptance",
                "required evidence missing",
                "release gate partial",
                "browser_readiness_missing",
                "browser_readiness_evidence_missing",
                "browser_readiness_or_interaction_evidence_required",
            ],
        )
    }) {
        return RepairTarget::RequiredEvidenceMissing;
    }
    if report.profile_failures.iter().any(|reason| {
        contains_any(
            reason,
            &[
                "required_capabilities",
                "non_zero_test",
                "test_artifact",
                "bound_verify_command",
                "release gate failed",
                "browser_readiness_failed",
            ],
        )
    }) {
        return RepairTarget::TestOrEvidence;
    }
    if report.command_failures.iter().any(|failure| {
        contains_any(
            &format!("{} {}", failure.command, failure.reason),
            &["src/app/page", "app/page", "pages/index", "src/pages/index"],
        )
    }) {
        return RepairTarget::MissingEntrypoint;
    }
    if report.command_failures.iter().any(|failure| {
        contains_any(
            &format!("{} {}", failure.command, failure.reason),
            &[
                "no tests ran",
                "ran 0 tests",
                "test_discovery_failure",
                "test_framework_mismatch",
                "non_zero_test",
            ],
        )
    }) {
        return RepairTarget::TestOrEvidence;
    }
    if report.command_failures.iter().any(|failure| {
        contains_any(
            &format!("{} {}", failure.command, failure.reason),
            &[
                "npm run build",
                "next build",
                "cargo build",
                "compile",
                "syntax",
                "type error",
                "tsc",
            ],
        )
    }) {
        return RepairTarget::Implementation;
    }
    if report.missing_paths.iter().any(|path| {
        contains_any(
            path,
            &[
                "package.json",
                "tsconfig",
                "next.config",
                "tailwind",
                "global.d.ts",
                "layout.tsx",
            ],
        )
    }) {
        return RepairTarget::FrameworkConfig;
    }
    if !report.missing_paths.is_empty()
        || !report.command_failures.is_empty()
        || !report.profile_failures.is_empty()
    {
        return RepairTarget::Implementation;
    }
    RepairTarget::Unknown
}

pub fn repair_target_matches_changed_path(target: RepairTarget, path: &str) -> bool {
    let lower = path.to_ascii_lowercase();
    match target {
        RepairTarget::DependencySetup => {
            lower == "package.json"
                || lower == "package-lock.json"
                || lower == "pnpm-lock.yaml"
                || lower == "yarn.lock"
                || lower == "cargo.toml"
                || lower == "cargo.lock"
                || lower == "pyproject.toml"
                || lower == "requirements.txt"
                || lower.ends_with("/package.json")
                || lower.ends_with("/package-lock.json")
                || lower.ends_with("/pnpm-lock.yaml")
                || lower.ends_with("/yarn.lock")
                || lower.ends_with("/cargo.toml")
                || lower.ends_with("/cargo.lock")
                || lower.ends_with("/pyproject.toml")
                || lower.ends_with("/requirements.txt")
                || lower.contains("node_modules/.bin/")
        }
        RepairTarget::PackageConfig => {
            lower.ends_with("package.json") || lower.ends_with("package-lock.json")
        }
        RepairTarget::FrameworkConfig => contains_any(
            &lower,
            &[
                "tsconfig",
                "next.config",
                "tailwind",
                "postcss",
                "global.d.ts",
                "globals.css",
                "layout.tsx",
                "layout.jsx",
            ],
        ),
        RepairTarget::MissingEntrypoint => contains_any(
            &lower,
            &[
                "src/app/page",
                "app/page",
                "pages/index",
                "src/pages/index",
                "main.rs",
                "lib.rs",
                "main.py",
                "app.py",
            ],
        ),
        RepairTarget::EmptyApp | RepairTarget::CapabilityMissing => {
            contains_any(
                &lower,
                &[
                    "src/", "app/", "pages/", ".rs", ".py", ".ts", ".tsx", ".js", ".jsx",
                ],
            ) && !lower.ends_with("package.json")
        }
        RepairTarget::RequiredEvidenceMissing => contains_any(
            &lower,
            &[
                "test",
                "spec",
                "__tests__",
                ".snap",
                "evidence",
                "README",
                "readme",
            ],
        ),
        RepairTarget::Implementation => {
            contains_any(
                &lower,
                &[
                    "src/", "app/", "pages/", ".rs", ".py", ".ts", ".tsx", ".js", ".jsx",
                ],
            ) && !lower.ends_with("package.json")
        }
        RepairTarget::TestOrEvidence => contains_any(
            &lower,
            &[
                "test",
                "spec",
                "__tests__",
                ".snap",
                "evidence",
                "README",
                "readme",
            ],
        ),
        RepairTarget::Unknown => true,
    }
}

pub fn classify_repair_follow_through(
    target: RepairTarget,
    changed_paths: &[String],
) -> RepairFollowThrough {
    if changed_paths.is_empty() {
        return RepairFollowThrough::NoChange;
    }
    if changed_paths
        .iter()
        .any(|path| repair_target_matches_changed_path(target, path))
    {
        RepairFollowThrough::TargetMatched
    } else if changed_paths
        .iter()
        .any(|path| repair_change_is_related_to_task_artifact(path))
    {
        RepairFollowThrough::TargetNotFollowed
    } else {
        RepairFollowThrough::UnrelatedChange
    }
}

pub fn repair_target_followed(target: RepairTarget, changed_paths: &[String]) -> bool {
    classify_repair_follow_through(target, changed_paths).followed()
}

fn contains_any(value: &str, needles: &[&str]) -> bool {
    let lower = value.to_ascii_lowercase();
    needles.iter().any(|needle| lower.contains(needle))
}

fn missing_required_evidence_includes_source_scan(reason: &str) -> bool {
    missing_required_evidence_keys(reason)
        .into_iter()
        .any(|key| evidence_satisfaction_channel(key) == SatisfactionChannel::SourceScan)
}

fn missing_required_evidence_keys(reason: &str) -> Vec<&str> {
    let Some((_, rest)) = reason.split_once("missing_required_evidence:") else {
        return Vec::new();
    };
    rest.split(',')
        .map(|key| {
            key.trim()
                .trim_matches(|ch: char| matches!(ch, '.' | ';' | ')' | ']'))
        })
        .filter(|key| !key.is_empty())
        .collect()
}

fn repair_change_is_related_to_task_artifact(path: &str) -> bool {
    let lower = path.to_ascii_lowercase();
    if lower.starts_with(".anvil/")
        || lower.starts_with(".git/")
        || lower.starts_with("docs/")
        || lower.ends_with(".md")
    {
        return false;
    }
    contains_any(
        &lower,
        &[
            "src/",
            "app/",
            "pages/",
            "tests/",
            "__tests__/",
            "test",
            "spec",
            "package.json",
            "package-lock.json",
            "pnpm-lock.yaml",
            "yarn.lock",
            "cargo.toml",
            "cargo.lock",
            "pyproject.toml",
            "requirements.txt",
            "tsconfig",
            "next.config",
            "tailwind",
            "postcss",
            ".rs",
            ".py",
            ".ts",
            ".tsx",
            ".js",
            ".jsx",
        ],
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::planner::verify::VerificationReport;

    #[test]
    fn dependency_missing_targets_setup() {
        let mut report = VerificationReport::pass();
        report.push_dependency_missing("node_modules/.bin/next missing");
        assert_eq!(
            classify_repair_target(&report),
            RepairTarget::DependencySetup
        );
    }

    #[test]
    fn package_failures_target_package_config() {
        let mut report = VerificationReport::pass();
        report.push_profile_failure("scripts.build must run next build");
        assert_eq!(classify_repair_target(&report), RepairTarget::PackageConfig);
    }

    #[test]
    fn evidence_failures_target_required_evidence() {
        let mut report = VerificationReport::pass();
        report.push_profile_failure("missing_required_evidence:non_zero_test");
        assert_eq!(
            classify_repair_target(&report),
            RepairTarget::RequiredEvidenceMissing
        );
    }

    #[test]
    fn release_gate_missing_browser_evidence_targets_required_evidence() {
        let mut report = VerificationReport::pass();
        report.push_profile_failure(
            "release gate partial: browser_readiness_or_interaction_evidence_required:browser_readiness_evidence_missing",
        );
        assert_eq!(
            classify_repair_target(&report),
            RepairTarget::RequiredEvidenceMissing
        );
    }

    #[test]
    fn tailwind_dev_route_failure_targets_framework_config() {
        let mut report = VerificationReport::pass();
        report.push_profile_failure(
            "release gate failed: browser_readiness_failed:tailwind_dev_pipeline_failure",
        );
        assert_eq!(
            classify_repair_target(&report),
            RepairTarget::FrameworkConfig
        );
    }

    #[test]
    fn browser_http_500_route_failure_targets_framework_config() {
        let mut report = VerificationReport::pass();
        report.push_profile_failure("release gate failed: browser_readiness_failed:http_500");
        assert_eq!(
            classify_repair_target(&report),
            RepairTarget::FrameworkConfig
        );
    }

    #[test]
    fn browser_route_failure_targets_test_or_evidence() {
        let mut report = VerificationReport::pass();
        report.push_profile_failure("release gate failed: browser_readiness_failed:route_render");
        assert_eq!(
            classify_repair_target(&report),
            RepairTarget::TestOrEvidence
        );
    }

    #[test]
    fn browser_interaction_probe_failure_targets_capability_implementation() {
        let mut report = VerificationReport::pass();
        report.push_profile_failure(
            "release gate failed: browser_interaction_failed:start_transition_missing",
        );
        assert_eq!(
            classify_repair_target(&report),
            RepairTarget::CapabilityMissing
        );
        assert_eq!(
            classify_repair_target(&report).allowed_action(),
            "edit_task_implementation_artifact"
        );
    }

    #[test]
    fn browser_interaction_probe_infrastructure_failure_is_not_app_repair_target() {
        let mut report = VerificationReport::pass();
        report.push_profile_failure(
            "release gate failed: probe_dependency_missing:playwright_module_missing; app interaction untested"
                .to_string(),
        );

        assert_eq!(classify_repair_target(&report), RepairTarget::Unknown);
    }

    #[test]
    fn repair_target_followed_accepts_package_config_paths() {
        assert!(repair_target_followed(
            RepairTarget::PackageConfig,
            &["package.json".to_string()]
        ));
        assert!(!repair_target_followed(
            RepairTarget::PackageConfig,
            &["src/app/page.tsx".to_string()]
        ));
    }

    #[test]
    fn repair_follow_through_distinguishes_no_change() {
        assert_eq!(
            classify_repair_follow_through(RepairTarget::MissingEntrypoint, &[]),
            RepairFollowThrough::NoChange
        );
        assert!(!repair_target_followed(
            RepairTarget::MissingEntrypoint,
            &[]
        ));
    }

    #[test]
    fn repair_follow_through_distinguishes_target_not_followed() {
        assert_eq!(
            classify_repair_follow_through(
                RepairTarget::MissingEntrypoint,
                &["src/app/widget.tsx".to_string()]
            ),
            RepairFollowThrough::TargetNotFollowed
        );
    }

    #[test]
    fn repair_follow_through_distinguishes_unrelated_change() {
        assert_eq!(
            classify_repair_follow_through(
                RepairTarget::MissingEntrypoint,
                &["README.md".to_string()]
            ),
            RepairFollowThrough::UnrelatedChange
        );
    }

    #[test]
    fn repair_follow_through_accepts_missing_entrypoint_artifact() {
        assert_eq!(
            classify_repair_follow_through(
                RepairTarget::MissingEntrypoint,
                &["src/app/page.tsx".to_string()]
            ),
            RepairFollowThrough::TargetMatched
        );
    }

    #[test]
    fn dependency_setup_target_accepts_manifest_or_lockfile_changes() {
        assert!(repair_target_followed(
            RepairTarget::DependencySetup,
            &["package.json".to_string()]
        ));
        assert!(repair_target_followed(
            RepairTarget::DependencySetup,
            &["package-lock.json".to_string()]
        ));
    }

    #[test]
    fn classifies_missing_entrypoint() {
        let report = VerificationReport::missing_path("src/app/page.tsx");
        assert_eq!(
            classify_repair_target(&report),
            RepairTarget::MissingEntrypoint
        );
    }

    #[test]
    fn classifies_missing_implementation_obligation_target_as_entrypoint() {
        let mut report = VerificationReport::pass();
        report.push_profile_failure(
            "missing_required_obligation_target:implementation:src/app/page.tsx",
        );
        assert_eq!(
            classify_repair_target(&report),
            RepairTarget::MissingEntrypoint
        );
    }

    #[test]
    fn classifies_capability_missing() {
        let mut report = VerificationReport::pass();
        report.push_profile_failure("missing_required_capabilities:player_control");
        assert_eq!(
            classify_repair_target(&report),
            RepairTarget::CapabilityMissing
        );
    }

    #[test]
    fn classifies_interactive_source_evidence_as_capability_missing() {
        let mut report = VerificationReport::pass();
        report.push_profile_failure("missing_required_evidence:interactive_ui_source_evidence");
        assert_eq!(
            classify_repair_target(&report),
            RepairTarget::CapabilityMissing
        );
    }

    #[test]
    fn source_scanned_missing_gameplay_evidence_targets_capability_implementation() {
        let mut report = VerificationReport::pass();
        report.push_profile_failure(
            "missing_required_evidence:failure_or_collision_evidence,restart_or_recoverable_state_evidence",
        );
        let target = classify_repair_target(&report);
        assert_eq!(target, RepairTarget::CapabilityMissing);
        assert_eq!(
            classify_repair_follow_through(target, &["src/app/page.tsx".to_string()]),
            RepairFollowThrough::TargetMatched
        );
    }

    #[test]
    fn test_artifact_only_missing_evidence_keeps_evidence_target() {
        let mut report = VerificationReport::pass();
        report.push_profile_failure("missing_required_evidence:test_artifact,bound_verify_command");
        let target = classify_repair_target(&report);
        assert_eq!(target, RepairTarget::RequiredEvidenceMissing);
        assert_eq!(
            classify_repair_follow_through(target, &["src/app/page.tsx".to_string()]),
            RepairFollowThrough::TargetNotFollowed
        );
    }

    #[test]
    fn repair_target_followed_accepts_framework_config_paths() {
        assert!(repair_target_followed(
            RepairTarget::FrameworkConfig,
            &["src/app/global.d.ts".to_string()]
        ));
    }
}
