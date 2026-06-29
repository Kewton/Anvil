use crate::planner::verify::VerificationReport;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RepairTarget {
    DependencySetup,
    PackageConfig,
    FrameworkConfig,
    Implementation,
    TestOrEvidence,
    Unknown,
}

impl RepairTarget {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::DependencySetup => "dependency_setup",
            Self::PackageConfig => "package_config",
            Self::FrameworkConfig => "framework_config",
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
}

pub fn classify_repair_target(report: &VerificationReport) -> RepairTarget {
    if !report.dependency_missing.is_empty() {
        return RepairTarget::DependencySetup;
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
                "css",
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
                "missing_required_evidence",
                "weak_verification_evidence",
                "required_capabilities",
                "non_zero_test",
                "test_artifact",
                "bound_verify_command",
            ],
        )
    }) {
        return RepairTarget::TestOrEvidence;
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
            lower == "package-lock.json"
                || lower.ends_with("/package-lock.json")
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

pub fn repair_target_followed(target: RepairTarget, changed_paths: &[String]) -> bool {
    changed_paths.is_empty()
        || changed_paths
            .iter()
            .any(|path| repair_target_matches_changed_path(target, path))
}

fn contains_any(value: &str, needles: &[&str]) -> bool {
    let lower = value.to_ascii_lowercase();
    needles.iter().any(|needle| lower.contains(needle))
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
    fn evidence_failures_target_tests_or_evidence() {
        let mut report = VerificationReport::pass();
        report.push_profile_failure("missing_required_evidence:non_zero_test");
        assert_eq!(
            classify_repair_target(&report),
            RepairTarget::TestOrEvidence
        );
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
    fn repair_target_followed_accepts_framework_config_paths() {
        assert!(repair_target_followed(
            RepairTarget::FrameworkConfig,
            &["src/app/global.d.ts".to_string()]
        ));
    }
}
