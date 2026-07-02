//! Runtime capability packets for contract-bound generation and repair.
//!
//! The packet is observed controller-side context. It is intentionally
//! stack/task-level rather than benchmark-level: generation and repair can avoid
//! impossible dependencies without adding per-task repair branches. It is never
//! completion authority.

use std::path::Path;
use std::process::Command;

use super::evidence_runner::{EvidenceRunner, EvidenceRunnerKind, evidence_runner_for_task_kind};
use super::task_contract::{ArtifactRole, ObjectiveDeliverableKind, TaskKind};
use super::verifier::capability_for;
use super::worker_contract::{RuntimeProfile, TaskExecutionContract};
use crate::session::store::ConversationMessage;

const MAX_VERSION_BYTES: usize = 80;
const KNOWN_MANIFEST_PATHS: &[&str] = &[
    "Cargo.toml",
    "pyproject.toml",
    "requirements.txt",
    "package.json",
    "tsconfig.json",
    "go.mod",
    "pom.xml",
    "Gemfile",
    "composer.json",
];

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct RuntimeCapabilityContext {
    observed_manifests: Vec<String>,
}

impl RuntimeCapabilityContext {
    pub(super) fn from_work_root(work_root: &Path) -> Self {
        let observed_manifests = KNOWN_MANIFEST_PATHS
            .iter()
            .filter(|path| work_root.join(path).is_file())
            .map(|path| (*path).to_string())
            .collect();
        Self { observed_manifests }
    }

    #[cfg(test)]
    fn empty() -> Self {
        Self {
            observed_manifests: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct RuntimeRunnerCapability {
    profile: RuntimeProfile,
    runner: &'static str,
    available: bool,
    observed_version: Option<String>,
    unavailable_capabilities: Vec<&'static str>,
}

impl RuntimeRunnerCapability {
    fn unavailable_labels(&self) -> Vec<&'static str> {
        self.unavailable_capabilities.clone()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ManifestPresence {
    NotApplicable,
    Absent,
    DeclaredMissing,
    Observed,
    DeclaredAndObserved,
}

impl ManifestPresence {
    fn label(self) -> &'static str {
        match self {
            Self::NotApplicable => "not_applicable",
            Self::Absent => "absent",
            Self::DeclaredMissing => "declared_missing",
            Self::Observed => "observed",
            Self::DeclaredAndObserved => "declared_and_observed",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct RuntimeCapabilityPacket {
    task_kind: TaskKind,
    process_exec_allowed: bool,
    detected_language: RuntimeProfile,
    project_shape: &'static str,
    declared_manifests: Vec<String>,
    observed_manifests: Vec<String>,
    manifest_presence: ManifestPresence,
    verifier_runner_kind: Option<EvidenceRunnerKind>,
    dependency_constraints: Vec<String>,
    runner: Option<RuntimeRunnerCapability>,
}

#[cfg(test)]
pub(super) fn runtime_capability_message_for_execution(
    execution: &TaskExecutionContract,
) -> ConversationMessage {
    let context = RuntimeCapabilityContext::empty();
    runtime_capability_message_for_execution_with_context(execution, &context)
}

pub(super) fn runtime_capability_message_for_execution_with_context(
    execution: &TaskExecutionContract,
    context: &RuntimeCapabilityContext,
) -> ConversationMessage {
    let packet = RuntimeCapabilityPacket::from_execution(execution, context);
    ConversationMessage::system(packet.policy_message())
}

pub(super) fn runtime_capability_payload_for_execution(
    execution: &TaskExecutionContract,
    context: &RuntimeCapabilityContext,
) -> serde_json::Value {
    RuntimeCapabilityPacket::from_execution(execution, context).payload()
}

impl RuntimeCapabilityPacket {
    fn from_execution(
        execution: &TaskExecutionContract,
        context: &RuntimeCapabilityContext,
    ) -> Self {
        let task_kind = execution.objective_kind.to_task_kind();
        let process_exec_allowed = capability_for(task_kind).allows_process_exec();
        let runner = RuntimeRunnerCapability::probe(execution.runtime_profile);
        let declared_manifests = declared_manifest_paths(execution);
        let observed_manifests = context.observed_manifests.clone();
        let manifest_presence =
            manifest_presence(task_kind, &declared_manifests, &observed_manifests);
        let verifier_runner_kind =
            evidence_runner_for_task_kind(task_kind).map(|runner| runner.kind());
        let dependency_constraints =
            dependency_constraints(process_exec_allowed, manifest_presence, runner.as_ref());

        Self {
            task_kind,
            process_exec_allowed,
            detected_language: execution.runtime_profile,
            project_shape: project_shape_for_execution(execution, manifest_presence),
            declared_manifests,
            observed_manifests,
            manifest_presence,
            verifier_runner_kind,
            dependency_constraints,
            runner,
        }
    }

    fn policy_message(&self) -> String {
        let declared_manifests = list_or_none(&self.declared_manifests);
        let observed_manifests = list_or_none(&self.observed_manifests);
        let verifier_runner = self
            .verifier_runner_kind
            .map(EvidenceRunnerKind::as_str)
            .unwrap_or("none");
        let dependency_constraints = if self.dependency_constraints.is_empty() {
            "none".to_string()
        } else {
            self.dependency_constraints.join(",")
        };
        let (runner, runner_available, observed_version, unavailable_capabilities) =
            match &self.runner {
                Some(runner) => {
                    let unavailable = runner.unavailable_labels();
                    let unavailable = if unavailable.is_empty() {
                        "none".to_string()
                    } else {
                        unavailable.join(",")
                    };
                    (
                        runner.runner,
                        runner.available.to_string(),
                        runner
                            .observed_version
                            .as_deref()
                            .unwrap_or("unknown")
                            .to_string(),
                        unavailable,
                    )
                }
                None => (
                    "none",
                    "n/a".to_string(),
                    "n/a".to_string(),
                    "none".to_string(),
                ),
            };
        format!(
            "[Runtime Capability Packet] task_kind={}; process_exec_allowed={}; detected_language={}; project_shape={}; manifest_presence={}; declared_manifests={}; observed_manifests={}; verifier_runner={}; dependency_constraints={}; runner={}; runner_available={}; observed_version={}; unavailable_capabilities={}. Treat this as controller-observed context, not completion authority. Do not import, install, execute, or verify against unavailable runtime capabilities unless the ObjectiveContract explicitly requires setup for them; choose an implementation compatible with the observed runner and declared deliverables.",
            self.task_kind.as_str(),
            self.process_exec_allowed,
            self.detected_language.label(),
            self.project_shape,
            self.manifest_presence.label(),
            declared_manifests,
            observed_manifests,
            verifier_runner,
            dependency_constraints,
            runner,
            runner_available,
            observed_version,
            unavailable_capabilities
        )
    }

    fn payload(&self) -> serde_json::Value {
        let runner_payload = self.runner.as_ref().map(|runner| {
            serde_json::json!({
                "profile": runner.profile.label(),
                "runner": runner.runner,
                "available": runner.available,
                "observed_version": runner.observed_version,
                "unavailable_capabilities": runner.unavailable_capabilities,
            })
        });
        serde_json::json!({
            "task_kind": self.task_kind.as_str(),
            "process_exec_allowed": self.process_exec_allowed,
            "detected_language": self.detected_language.label(),
            "project_shape": self.project_shape,
            "manifest_presence": self.manifest_presence.label(),
            "declared_manifests": self.declared_manifests,
            "observed_manifests": self.observed_manifests,
            "verifier_runner": self.verifier_runner_kind.map(EvidenceRunnerKind::as_str),
            "dependency_constraints": self.dependency_constraints,
            "runner": runner_payload,
            "authority": "context_only",
        })
    }
}

impl RuntimeRunnerCapability {
    fn probe(profile: RuntimeProfile) -> Option<Self> {
        match profile {
            RuntimeProfile::Python => Some(probe_python3()),
            RuntimeProfile::Rust => Some(probe_version_command(profile, "cargo", &["--version"])),
            RuntimeProfile::Node | RuntimeProfile::TypeScript => {
                Some(probe_version_command(profile, "node", &["--version"]))
            }
            RuntimeProfile::Unspecified => None,
        }
    }
}

fn probe_python3() -> RuntimeRunnerCapability {
    let mut packet = probe_version_command(RuntimeProfile::Python, "python3", &["--version"]);
    if !packet.available {
        return packet;
    }
    let version = packet
        .observed_version
        .as_deref()
        .and_then(parse_python_version);
    packet.unavailable_capabilities = python_unavailable_capabilities(version);
    packet
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct CapabilityRequirement {
    label: &'static str,
    minimum_version: RuntimeVersion,
}

const PYTHON_CAPABILITY_REQUIREMENTS: &[CapabilityRequirement] = &[CapabilityRequirement {
    label: "python_stdlib.tomllib(min_python=3.11)",
    minimum_version: RuntimeVersion {
        major: 3,
        minor: 11,
    },
}];

fn python_unavailable_capabilities(version: Option<RuntimeVersion>) -> Vec<&'static str> {
    PYTHON_CAPABILITY_REQUIREMENTS
        .iter()
        .filter(|requirement| {
            !version.is_some_and(|version| version.at_least_requirement(requirement))
        })
        .map(|requirement| requirement.label)
        .collect()
}

fn probe_version_command(
    profile: RuntimeProfile,
    runner: &'static str,
    args: &[&str],
) -> RuntimeRunnerCapability {
    match Command::new(runner).args(args).output() {
        Ok(output) => RuntimeRunnerCapability {
            profile,
            runner,
            available: output.status.success(),
            observed_version: first_non_empty_version_line(&output.stdout, &output.stderr),
            unavailable_capabilities: Vec::new(),
        },
        Err(_) => RuntimeRunnerCapability {
            profile,
            runner,
            available: false,
            observed_version: None,
            unavailable_capabilities: Vec::new(),
        },
    }
}

fn declared_manifest_paths(execution: &TaskExecutionContract) -> Vec<String> {
    let mut out = Vec::new();
    for deliverable in &execution.deliverables {
        let Some(path) = &deliverable.path else {
            continue;
        };
        let path = path.to_string_lossy().replace('\\', "/");
        if deliverable.role == ArtifactRole::Setup || is_known_manifest_path(&path) {
            push_unique(&mut out, path);
        }
    }
    out
}

fn manifest_presence(
    task_kind: TaskKind,
    declared_manifests: &[String],
    observed_manifests: &[String],
) -> ManifestPresence {
    match (
        declared_manifests.is_empty(),
        observed_manifests.is_empty(),
        task_kind,
    ) {
        (false, false, _) => ManifestPresence::DeclaredAndObserved,
        (false, true, _) => ManifestPresence::DeclaredMissing,
        (true, false, _) => ManifestPresence::Observed,
        (true, true, TaskKind::Coding) => ManifestPresence::Absent,
        (true, true, _) => ManifestPresence::NotApplicable,
    }
}

fn project_shape_for_execution(
    execution: &TaskExecutionContract,
    manifest_presence: ManifestPresence,
) -> &'static str {
    if execution.constraints.read_only {
        return "answer_only";
    }
    if matches!(
        manifest_presence,
        ManifestPresence::Observed | ManifestPresence::DeclaredAndObserved
    ) {
        return "manifested_project";
    }
    match execution.objective_contract().deliverable_kind {
        ObjectiveDeliverableKind::SourceFiles => "source_artifact",
        ObjectiveDeliverableKind::DocumentSections => "document_artifact",
        ObjectiveDeliverableKind::OutputFile => "data_artifact",
        ObjectiveDeliverableKind::ResearchNotes => "research_artifact",
        ObjectiveDeliverableKind::CommandObservation => "command_observation",
        ObjectiveDeliverableKind::ProseArtifact => "prose_artifact",
        ObjectiveDeliverableKind::VisualObservation => "visual_observation",
        ObjectiveDeliverableKind::Answer => "answer_only",
    }
}

fn dependency_constraints(
    process_exec_allowed: bool,
    manifest_presence: ManifestPresence,
    runner: Option<&RuntimeRunnerCapability>,
) -> Vec<String> {
    let mut out = Vec::new();
    if !process_exec_allowed {
        out.push("process_exec_disallowed_for_task_kind".to_string());
    }
    if manifest_presence == ManifestPresence::DeclaredMissing {
        out.push("declared_manifest_not_observed".to_string());
    }
    if let Some(runner) = runner {
        if !runner.available {
            out.push(format!("runner_unavailable:{}", runner.runner));
        }
        for capability in &runner.unavailable_capabilities {
            out.push((*capability).to_string());
        }
    }
    out
}

fn is_known_manifest_path(path: &str) -> bool {
    KNOWN_MANIFEST_PATHS.contains(&path)
}

fn push_unique(out: &mut Vec<String>, value: String) {
    if !out.contains(&value) {
        out.push(value);
    }
}

fn list_or_none(values: &[String]) -> String {
    if values.is_empty() {
        "none".to_string()
    } else {
        values.join(",")
    }
}

fn first_non_empty_version_line(stdout: &[u8], stderr: &[u8]) -> Option<String> {
    let combined = [stdout, stderr].concat();
    String::from_utf8_lossy(&combined)
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .map(cap_version_line)
}

fn cap_version_line(line: &str) -> String {
    let mut capped = line.chars().take(MAX_VERSION_BYTES).collect::<String>();
    if capped.len() < line.len() {
        capped.push_str("...");
    }
    capped
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct RuntimeVersion {
    major: u32,
    minor: u32,
}

impl RuntimeVersion {
    fn at_least(self, major: u32, minor: u32) -> bool {
        self.major > major || (self.major == major && self.minor >= minor)
    }

    fn at_least_requirement(self, requirement: &CapabilityRequirement) -> bool {
        self.at_least(
            requirement.minimum_version.major,
            requirement.minimum_version.minor,
        )
    }
}

fn parse_python_version(raw: &str) -> Option<RuntimeVersion> {
    let version = raw
        .split_whitespace()
        .find(|token| token.chars().next().is_some_and(|ch| ch.is_ascii_digit()))?;
    let mut parts = version.split('.');
    Some(RuntimeVersion {
        major: parts.next()?.parse().ok()?,
        minor: parts.next()?.parse().ok()?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn python_version_parser_accepts_stdout_shape() {
        assert_eq!(
            parse_python_version("Python 3.9.6"),
            Some(RuntimeVersion { major: 3, minor: 9 })
        );
        assert_eq!(
            parse_python_version("Python 3.11.1"),
            Some(RuntimeVersion {
                major: 3,
                minor: 11
            })
        );
    }

    #[test]
    fn python_packet_marks_tomllib_unavailable_before_3_11() {
        let runner = RuntimeRunnerCapability {
            profile: RuntimeProfile::Python,
            runner: "python3",
            available: true,
            observed_version: Some("Python 3.9.6".to_string()),
            unavailable_capabilities: vec!["python_stdlib.tomllib(min_python=3.11)"],
        };
        let packet = RuntimeCapabilityPacket {
            task_kind: TaskKind::Coding,
            process_exec_allowed: true,
            detected_language: RuntimeProfile::Python,
            project_shape: "source_artifact",
            declared_manifests: Vec::new(),
            observed_manifests: Vec::new(),
            manifest_presence: ManifestPresence::Absent,
            verifier_runner_kind: Some(EvidenceRunnerKind::CodingBuildTest),
            dependency_constraints: vec!["python_stdlib.tomllib(min_python=3.11)".to_string()],
            runner: Some(runner),
        };

        let message = packet.policy_message();
        assert!(message.contains("detected_language=python"));
        assert!(message.contains("python_stdlib.tomllib(min_python=3.11)"));
        assert!(message.contains("controller-observed context"));
    }

    #[test]
    fn python_capability_table_marks_requirements_by_version() {
        assert_eq!(
            python_unavailable_capabilities(Some(RuntimeVersion { major: 3, minor: 9 })),
            vec!["python_stdlib.tomllib(min_python=3.11)"]
        );
        assert!(
            python_unavailable_capabilities(Some(RuntimeVersion {
                major: 3,
                minor: 11
            }))
            .is_empty()
        );
    }

    #[test]
    fn non_coding_runtime_packet_keeps_process_exec_disallowed() {
        let contract =
            super::super::task_contract::TaskContract::from_request("Write README.md only.");
        let execution = TaskExecutionContract::from_task_contract(&contract);

        let message = runtime_capability_message_for_execution(&execution);
        assert!(message.content.contains("task_kind=docs"));
        assert!(message.content.contains("process_exec_allowed=false"));
        assert!(
            message
                .content
                .contains("verifier_runner=docs_content_check")
        );
        assert!(message.content.contains("runner=none"));
    }

    #[test]
    fn data_packet_projects_schema_runner_without_process_exec() {
        let contract = super::super::task_contract::TaskContract::from_request(
            "Generate output.csv with columns Category and Total from the input CSV.",
        );
        let execution = TaskExecutionContract::from_task_contract(&contract);
        let packet =
            RuntimeCapabilityPacket::from_execution(&execution, &RuntimeCapabilityContext::empty());

        assert_eq!(packet.task_kind, TaskKind::Data);
        assert!(!packet.process_exec_allowed);
        assert_eq!(packet.project_shape, "data_artifact");
        assert_eq!(
            packet.verifier_runner_kind,
            Some(EvidenceRunnerKind::DataSchemaCheck)
        );
        assert!(
            packet
                .dependency_constraints
                .contains(&"process_exec_disallowed_for_task_kind".to_string())
        );
    }

    #[test]
    fn observed_manifest_is_context_not_authority() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("Cargo.toml"),
            "[package]\nname=\"demo\"\nversion=\"0.1.0\"\nedition=\"2021\"\n",
        )
        .unwrap();
        let contract = super::super::task_contract::TaskContract::from_request(
            "Create a Rust CLI with tests.",
        );
        let execution = TaskExecutionContract::from_task_contract(&contract)
            .with_runtime_profile(RuntimeProfile::Rust);
        let context = RuntimeCapabilityContext::from_work_root(dir.path());
        let packet = RuntimeCapabilityPacket::from_execution(&execution, &context);

        assert!(
            packet
                .observed_manifests
                .contains(&"Cargo.toml".to_string())
        );
        assert!(matches!(
            packet.manifest_presence,
            ManifestPresence::Observed | ManifestPresence::DeclaredAndObserved
        ));
        assert_eq!(packet.project_shape, "manifested_project");
        assert_eq!(packet.payload()["authority"], "context_only");
    }
}
