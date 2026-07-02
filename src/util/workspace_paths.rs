//! Workspace path policy shared by controller evidence collectors and tools.
//!
//! `WorkspacePolicy` is the single source for controller-owned metadata and
//! runtime files. These paths must not count as user artifacts, repo edits,
//! verifier repair candidates, completion evidence, or ordinary model
//! discovery candidates. Explicit log-analysis tasks can opt into read-only
//! access to the protected metadata set.

use std::path::{Component, Path};

const IGNORED_WORKSPACE_DIRS: &[&str] = &[
    ".git",
    ".anvil",
    ".anvil-state",
    ".next",
    ".pytest_cache",
    "__pycache__",
    "node_modules",
    "target",
    ".venv",
    "venv",
    "dist",
    "build",
];

const CONTROL_METADATA_FILES: &[&str] = &[
    "prompt.md",
    "prompt.txt",
    "cmd.txt",
    "command.txt",
    "anvil.md",
    "session.json",
    "meta.json",
    "metadata.json",
    "llm-io.jsonl",
    "log.jsonl",
    "runtime.log",
    "run.log",
    "anvil.out",
    "anvil.err",
    "anvil.log",
    "sidecar.out",
    "sidecar.err",
    "sidecar.log",
    "eval.out",
    "eval.err",
    "eval.log",
    "postcheck.out",
    "postcheck.err",
];

const GENERATED_METADATA_ROOT_DIRS: &[&str] = &["logs", "eval", "benchmark", "benchmarks"];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WorkspacePathClass {
    UserDeliverable,
    ProtectedInput,
    GeneratedMetadata,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WorkspacePathAdmission {
    Accepted,
    Rejected {
        class: WorkspacePathClass,
        reason: WorkspacePolicyRejectReason,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WorkspacePolicyRejectReason {
    ProtectedInput,
    GeneratedMetadata,
}

impl WorkspacePolicyRejectReason {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::ProtectedInput => "protected_input",
            Self::GeneratedMetadata => "generated_metadata",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProtectedMetadataAccess {
    Deny,
    Allow,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WorkspacePolicy {
    protected_metadata_access: ProtectedMetadataAccess,
}

impl WorkspacePolicy {
    pub fn deny_protected_metadata() -> Self {
        Self {
            protected_metadata_access: ProtectedMetadataAccess::Deny,
        }
    }

    pub fn allow_protected_metadata_reads() -> Self {
        Self {
            protected_metadata_access: ProtectedMetadataAccess::Allow,
        }
    }

    pub fn for_task_request(request: &str) -> Self {
        if request_allows_protected_metadata_reads(request) {
            Self::allow_protected_metadata_reads()
        } else {
            Self::deny_protected_metadata()
        }
    }

    pub fn protected_metadata_access(self) -> ProtectedMetadataAccess {
        self.protected_metadata_access
    }

    pub fn is_workspace_ignored_dir_name(self, name: &str) -> bool {
        IGNORED_WORKSPACE_DIRS
            .iter()
            .any(|candidate| candidate.eq_ignore_ascii_case(name))
    }

    pub fn is_control_metadata_file_name(self, name: &str) -> bool {
        CONTROL_METADATA_FILES
            .iter()
            .any(|candidate| candidate.eq_ignore_ascii_case(name))
            || is_postcheck_file_name(name)
    }

    pub fn classify_relative_path(self, relative: &Path) -> WorkspacePathClass {
        if relative.components().any(|component| match component {
            Component::Normal(name) => name
                .to_str()
                .is_some_and(|name| self.is_workspace_ignored_dir_name(name)),
            _ => false,
        }) {
            return WorkspacePathClass::GeneratedMetadata;
        }
        if first_component_name(relative).is_some_and(is_generated_metadata_root_dir_name) {
            return WorkspacePathClass::GeneratedMetadata;
        }
        let Some(name) = top_level_file_name(relative) else {
            return WorkspacePathClass::UserDeliverable;
        };
        let lower = name.to_ascii_lowercase();
        if matches!(
            lower.as_str(),
            "prompt.md" | "prompt.txt" | "cmd.txt" | "command.txt"
        ) {
            return WorkspacePathClass::ProtectedInput;
        }
        if self.is_control_metadata_file_name(name) {
            return WorkspacePathClass::GeneratedMetadata;
        }
        WorkspacePathClass::UserDeliverable
    }

    pub fn is_user_deliverable_relative_path(self, relative: &Path) -> bool {
        self.classify_relative_path(relative) == WorkspacePathClass::UserDeliverable
    }

    pub fn is_protected_input_relative_path(self, relative: &Path) -> bool {
        self.classify_relative_path(relative) == WorkspacePathClass::ProtectedInput
    }

    pub fn is_protected_metadata_relative_path(self, relative: &Path) -> bool {
        self.classify_relative_path(relative) != WorkspacePathClass::UserDeliverable
    }

    pub fn artifact_admission_decision_relative_path(
        self,
        relative: &Path,
    ) -> WorkspacePathAdmission {
        let class = self.classify_relative_path(relative);
        match class {
            WorkspacePathClass::UserDeliverable => WorkspacePathAdmission::Accepted,
            WorkspacePathClass::ProtectedInput => WorkspacePathAdmission::Rejected {
                class,
                reason: WorkspacePolicyRejectReason::ProtectedInput,
            },
            WorkspacePathClass::GeneratedMetadata => WorkspacePathAdmission::Rejected {
                class,
                reason: WorkspacePolicyRejectReason::GeneratedMetadata,
            },
        }
    }

    pub fn artifact_admission_decision_display_path(
        self,
        display_path: &str,
    ) -> WorkspacePathAdmission {
        let path = display_path
            .strip_suffix(" (deleted)")
            .unwrap_or(display_path);
        self.artifact_admission_decision_relative_path(Path::new(path))
    }

    pub fn admits_artifact_relative_path(self, relative: &Path) -> bool {
        self.artifact_admission_decision_relative_path(relative) == WorkspacePathAdmission::Accepted
    }

    pub fn admits_artifact_display_path(self, display_path: &str) -> bool {
        self.artifact_admission_decision_display_path(display_path)
            == WorkspacePathAdmission::Accepted
    }

    pub fn allows_model_read_relative_path(self, relative: &Path) -> bool {
        self.protected_metadata_access == ProtectedMetadataAccess::Allow
            || !self.is_protected_metadata_relative_path(relative)
    }

    pub fn is_ignored_display_path(self, display_path: &str) -> bool {
        let path = display_path
            .strip_suffix(" (deleted)")
            .unwrap_or(display_path);
        self.is_protected_metadata_relative_path(Path::new(path))
    }
}

impl Default for WorkspacePolicy {
    fn default() -> Self {
        Self::deny_protected_metadata()
    }
}

pub fn is_workspace_ignored_dir_name(name: &str) -> bool {
    WorkspacePolicy::default().is_workspace_ignored_dir_name(name)
}

pub fn is_control_metadata_file_name(name: &str) -> bool {
    WorkspacePolicy::default().is_control_metadata_file_name(name)
}

pub fn is_ignored_workspace_relative_path(relative: &Path) -> bool {
    WorkspacePolicy::default().is_protected_metadata_relative_path(relative)
}

pub fn is_ignored_workspace_display_path(display_path: &str) -> bool {
    WorkspacePolicy::default().is_ignored_display_path(display_path)
}

pub fn artifact_admission_decision_relative_path(relative: &Path) -> WorkspacePathAdmission {
    WorkspacePolicy::default().artifact_admission_decision_relative_path(relative)
}

pub fn artifact_admission_decision_display_path(display_path: &str) -> WorkspacePathAdmission {
    WorkspacePolicy::default().artifact_admission_decision_display_path(display_path)
}

pub fn is_workspace_artifact_admitted_relative_path(relative: &Path) -> bool {
    WorkspacePolicy::default().admits_artifact_relative_path(relative)
}

pub fn is_workspace_artifact_admitted_display_path(display_path: &str) -> bool {
    WorkspacePolicy::default().admits_artifact_display_path(display_path)
}

fn is_postcheck_file_name(name: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    lower == "postcheck" || lower.starts_with("postcheck.")
}

fn is_generated_metadata_root_dir_name(name: &str) -> bool {
    GENERATED_METADATA_ROOT_DIRS
        .iter()
        .any(|candidate| candidate.eq_ignore_ascii_case(name))
}

fn request_allows_protected_metadata_reads(request: &str) -> bool {
    let lower = request.to_ascii_lowercase();
    let mentions_metadata = [
        "log",
        "logs",
        "anvil.out",
        "anvil.err",
        "llm-io",
        "session.json",
        "metadata",
        ".anvil",
        "runtime",
        "ログ",
        "メタデータ",
    ]
    .iter()
    .any(|needle| lower.contains(needle));
    let asks_to_inspect = [
        "analyze",
        "analyse",
        "inspect",
        "review",
        "read",
        "debug",
        "investigate",
        "check",
        "調査",
        "分析",
        "確認",
        "読",
        "見",
    ]
    .iter()
    .any(|needle| lower.contains(needle));
    mentions_metadata && asks_to_inspect
}

fn first_component_name(relative: &Path) -> Option<&str> {
    match relative.components().next()? {
        Component::Normal(name) => name.to_str(),
        _ => None,
    }
}

fn top_level_file_name(relative: &Path) -> Option<&str> {
    let mut components = relative.components();
    let first = match components.next()? {
        Component::Normal(name) => name.to_str()?,
        _ => return None,
    };
    components.next().is_none().then_some(first)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ignored_workspace_relative_path_matches_components() {
        assert!(is_ignored_workspace_relative_path(Path::new(
            ".anvil-state/verifier-python/site/_pytest/__init__.py"
        )));
        assert!(is_ignored_workspace_relative_path(Path::new(
            "app/__pycache__/main.cpython-39.pyc"
        )));
        assert!(is_ignored_workspace_relative_path(Path::new("anvil.out")));
        assert!(is_ignored_workspace_relative_path(Path::new("eval.out")));
        assert!(is_ignored_workspace_relative_path(Path::new("runtime.log")));
        assert!(is_ignored_workspace_relative_path(Path::new("sidecar.log")));
        assert!(is_ignored_workspace_relative_path(Path::new(
            "postcheck.err"
        )));
        assert!(is_ignored_workspace_relative_path(Path::new(
            "postcheck.junit.xml"
        )));
        assert!(is_ignored_workspace_relative_path(Path::new("prompt.md")));
        assert!(is_ignored_workspace_relative_path(Path::new(
            ".anvil/session.json"
        )));
        assert!(is_ignored_workspace_relative_path(Path::new(
            "logs/llm-io.jsonl"
        )));
        assert!(!is_ignored_workspace_relative_path(Path::new(
            "docs/anvil-state-notes.md"
        )));
        assert!(!is_ignored_workspace_relative_path(Path::new(
            "docs/anvil.out"
        )));
    }

    #[test]
    fn ignored_workspace_display_path_handles_deleted_suffix() {
        assert!(is_ignored_workspace_display_path(
            ".anvil-state/verifier-python/site/foo.py (deleted)"
        ));
        assert!(!is_ignored_workspace_display_path("README.md (deleted)"));
    }

    #[test]
    fn artifact_admission_decision_explains_policy_rejection() {
        assert_eq!(
            artifact_admission_decision_relative_path(Path::new("src/main.rs")),
            WorkspacePathAdmission::Accepted
        );
        assert_eq!(
            artifact_admission_decision_relative_path(Path::new("prompt.md")),
            WorkspacePathAdmission::Rejected {
                class: WorkspacePathClass::ProtectedInput,
                reason: WorkspacePolicyRejectReason::ProtectedInput,
            }
        );
        assert_eq!(
            artifact_admission_decision_relative_path(Path::new("anvil.out")),
            WorkspacePathAdmission::Rejected {
                class: WorkspacePathClass::GeneratedMetadata,
                reason: WorkspacePolicyRejectReason::GeneratedMetadata,
            }
        );
        assert_eq!(
            artifact_admission_decision_display_path("anvil.out (deleted)"),
            WorkspacePathAdmission::Rejected {
                class: WorkspacePathClass::GeneratedMetadata,
                reason: WorkspacePolicyRejectReason::GeneratedMetadata,
            }
        );
    }

    #[test]
    fn workspace_policy_can_allow_explicit_metadata_reads() {
        let normal = WorkspacePolicy::deny_protected_metadata();
        let log_analysis = WorkspacePolicy::allow_protected_metadata_reads();

        assert!(!normal.allows_model_read_relative_path(Path::new("prompt.md")));
        assert!(!normal.allows_model_read_relative_path(Path::new(".anvil/session.json")));
        assert!(normal.allows_model_read_relative_path(Path::new("src/main.rs")));

        assert!(log_analysis.allows_model_read_relative_path(Path::new("prompt.md")));
        assert!(log_analysis.allows_model_read_relative_path(Path::new(".anvil/session.json")));
        assert!(log_analysis.allows_model_read_relative_path(Path::new("src/main.rs")));
    }

    #[test]
    fn task_request_only_allows_metadata_reads_for_explicit_log_analysis() {
        assert_eq!(
            WorkspacePolicy::for_task_request("analyze anvil.out and explain the failure")
                .protected_metadata_access(),
            ProtectedMetadataAccess::Allow
        );
        assert_eq!(
            WorkspacePolicy::for_task_request("build a CLI from prompt.md")
                .protected_metadata_access(),
            ProtectedMetadataAccess::Deny
        );
    }
}
