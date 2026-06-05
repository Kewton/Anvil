//! Issue #961: task-specialized worker contracts and bounded context packs.
//!
//! This is an additive foundation. It deliberately does not dispatch workers
//! yet; later MissingEvidence / DiagnosticRepair / non-coding capability
//! tracks can consume the same contract shape without adding provider
//! abstraction or broad prompt/context formats.

#![allow(dead_code)] // Foundation seam; focused tests pin the shape before broad callers are wired.

use std::path::{Path, PathBuf};

use super::task_contract::{
    ArtifactRole, ObjectiveDeliverableKind, ObjectiveEvidenceKind, RecoveryTargetHint,
    TaskContract, TaskKind,
};

pub(super) const MAX_CONTEXT_PACK_ENTRIES: usize = 8;
pub(super) const MAX_CONTEXT_PACK_ENTRY_BYTES: usize = 2048;
const MAX_CONTEXT_PACK_LABEL_BYTES: usize = 128;
const TRUNCATION_MARKER: &str = "...";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum WorkerKind {
    Implement,
    TestAuthor,
    Evidence,
    DiagnosticRepair,
    Docs,
    Data,
    Research,
    Ops,
    Authoring,
}

impl WorkerKind {
    pub(super) fn label(self) -> &'static str {
        match self {
            WorkerKind::Implement => "implement",
            WorkerKind::TestAuthor => "test_author",
            WorkerKind::Evidence => "evidence",
            WorkerKind::DiagnosticRepair => "diagnostic_repair",
            WorkerKind::Docs => "docs",
            WorkerKind::Data => "data",
            WorkerKind::Research => "research",
            WorkerKind::Ops => "ops",
            WorkerKind::Authoring => "authoring",
        }
    }

    pub(super) fn primary_for_task_kind(task_kind: TaskKind) -> Self {
        match task_kind {
            TaskKind::Coding => WorkerKind::Implement,
            TaskKind::Docs => WorkerKind::Docs,
            TaskKind::Data => WorkerKind::Data,
            TaskKind::Research => WorkerKind::Research,
            TaskKind::Ops => WorkerKind::Ops,
            TaskKind::Authoring => WorkerKind::Authoring,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum ContextPackKind {
    Contract,
    Target,
    Evidence,
    Diagnostic,
    Repair,
}

impl ContextPackKind {
    pub(super) fn label(self) -> &'static str {
        match self {
            ContextPackKind::Contract => "contract",
            ContextPackKind::Target => "target",
            ContextPackKind::Evidence => "evidence",
            ContextPackKind::Diagnostic => "diagnostic",
            ContextPackKind::Repair => "repair",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum CapabilityAllowedTool {
    Read,
    Write,
    Edit,
    Bash,
}

impl CapabilityAllowedTool {
    pub(super) fn label(self) -> &'static str {
        match self {
            CapabilityAllowedTool::Read => "read",
            CapabilityAllowedTool::Write => "write",
            CapabilityAllowedTool::Edit => "edit",
            CapabilityAllowedTool::Bash => "bash",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum CapabilityContextPolicy {
    ContractOnly,
    TargetedArtifact,
    EvidenceBounded,
    DiagnosticBounded,
    ProcedureBounded,
}

impl CapabilityContextPolicy {
    pub(super) fn label(self) -> &'static str {
        match self {
            CapabilityContextPolicy::ContractOnly => "contract_only",
            CapabilityContextPolicy::TargetedArtifact => "targeted_artifact",
            CapabilityContextPolicy::EvidenceBounded => "evidence_bounded",
            CapabilityContextPolicy::DiagnosticBounded => "diagnostic_bounded",
            CapabilityContextPolicy::ProcedureBounded => "procedure_bounded",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum CapabilityRepairStrategy {
    CompleteMissingDeliverable,
    CreateMissingEvidence,
    RepairFailedEvidence,
    ResolveToolFailure,
}

impl CapabilityRepairStrategy {
    pub(super) fn label(self) -> &'static str {
        match self {
            CapabilityRepairStrategy::CompleteMissingDeliverable => "complete_missing_deliverable",
            CapabilityRepairStrategy::CreateMissingEvidence => "create_missing_evidence",
            CapabilityRepairStrategy::RepairFailedEvidence => "repair_failed_evidence",
            CapabilityRepairStrategy::ResolveToolFailure => "resolve_tool_failure",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum CapabilityCompletionPredicate {
    TestRunPassed,
    RequiredSectionsPresent,
    SchemaCheckPassed,
    SourceEvidencePresent,
    CommandObservationRecorded,
    ContentAccepted,
}

impl CapabilityCompletionPredicate {
    pub(super) fn label(self) -> &'static str {
        match self {
            CapabilityCompletionPredicate::TestRunPassed => "test_run_passed",
            CapabilityCompletionPredicate::RequiredSectionsPresent => "required_sections_present",
            CapabilityCompletionPredicate::SchemaCheckPassed => "schema_check_passed",
            CapabilityCompletionPredicate::SourceEvidencePresent => "source_evidence_present",
            CapabilityCompletionPredicate::CommandObservationRecorded => {
                "command_observation_recorded"
            }
            CapabilityCompletionPredicate::ContentAccepted => "content_accepted",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum CapabilityLifecycleStageKind {
    Deliverable,
    Evidence,
    Repair,
    ToolFailure,
}

impl CapabilityLifecycleStageKind {
    pub(super) fn label(self) -> &'static str {
        match self {
            CapabilityLifecycleStageKind::Deliverable => "deliverable",
            CapabilityLifecycleStageKind::Evidence => "evidence",
            CapabilityLifecycleStageKind::Repair => "repair",
            CapabilityLifecycleStageKind::ToolFailure => "tool_failure",
        }
    }

    pub(super) fn recovery_job_label(self) -> &'static str {
        match self {
            CapabilityLifecycleStageKind::Deliverable => "MissingDeliverableJob",
            CapabilityLifecycleStageKind::Evidence => "MissingEvidenceJob",
            CapabilityLifecycleStageKind::Repair => "EvidenceFailedJob",
            CapabilityLifecycleStageKind::ToolFailure => "ToolFailureJob",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct CapabilitySpec {
    pub(super) task_kind: TaskKind,
    pub(super) deliverable_kind: ObjectiveDeliverableKind,
    pub(super) evidence_kind: ObjectiveEvidenceKind,
    pub(super) required_artifacts: Vec<ArtifactRole>,
    pub(super) allowed_tools: Vec<CapabilityAllowedTool>,
    pub(super) context_policy: CapabilityContextPolicy,
    pub(super) worker_sequence: Vec<WorkerKind>,
    pub(super) repair_strategies: Vec<CapabilityRepairStrategy>,
    pub(super) completion_predicate: CapabilityCompletionPredicate,
    pub(super) eval_labels: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct CapabilityDefaults {
    task_kind: TaskKind,
    deliverable_kind: ObjectiveDeliverableKind,
    evidence_kind: ObjectiveEvidenceKind,
    required_artifacts: Vec<ArtifactRole>,
    allowed_tools: Vec<CapabilityAllowedTool>,
    context_policy: CapabilityContextPolicy,
    worker_sequence: Vec<WorkerKind>,
    repair_strategies: Vec<CapabilityRepairStrategy>,
    completion_predicate: CapabilityCompletionPredicate,
}

impl CapabilityDefaults {
    fn into_spec(self) -> CapabilitySpec {
        let eval_labels = vec![
            format!("task_kind={}", self.task_kind.as_str()),
            format!("deliverable_kind={}", self.deliverable_kind.label()),
            format!("evidence_kind={}", self.evidence_kind.label()),
            format!("completion_predicate={}", self.completion_predicate.label()),
        ];
        CapabilitySpec {
            task_kind: self.task_kind,
            deliverable_kind: self.deliverable_kind,
            evidence_kind: self.evidence_kind,
            required_artifacts: self.required_artifacts,
            allowed_tools: self.allowed_tools,
            context_policy: self.context_policy,
            worker_sequence: self.worker_sequence,
            repair_strategies: self.repair_strategies,
            completion_predicate: self.completion_predicate,
            eval_labels,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct CapabilityLifecycleStagePlan {
    pub(super) stage_kind: CapabilityLifecycleStageKind,
    pub(super) worker_contract: WorkerContract,
    pub(super) context_policy: CapabilityContextPolicy,
    pub(super) repair_strategy: CapabilityRepairStrategy,
    pub(super) output_contract: &'static str,
    pub(super) eval_label: String,
    policy_message: String,
}

impl CapabilityLifecycleStagePlan {
    pub(super) fn recovery_job_label(&self) -> &'static str {
        self.stage_kind.recovery_job_label()
    }

    pub(super) fn policy_message(&self) -> &str {
        &self.policy_message
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct WorkerLifecyclePlan {
    pub(super) capability: CapabilitySpec,
    stages: Vec<CapabilityLifecycleStagePlan>,
}

impl WorkerLifecyclePlan {
    pub(super) fn stages(&self) -> &[CapabilityLifecycleStagePlan] {
        &self.stages
    }

    pub(super) fn stage(
        &self,
        stage_kind: CapabilityLifecycleStageKind,
    ) -> Option<&CapabilityLifecycleStagePlan> {
        self.stages
            .iter()
            .find(|stage| stage.stage_kind == stage_kind)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct ContextPackEntry {
    pub(super) kind: ContextPackKind,
    label: String,
    content: String,
    truncated: bool,
}

impl ContextPackEntry {
    pub(super) fn new(
        kind: ContextPackKind,
        label: impl Into<String>,
        content: impl Into<String>,
    ) -> Self {
        let (label, _) = truncate_utf8(label.into(), MAX_CONTEXT_PACK_LABEL_BYTES);
        let (content, truncated) = truncate_utf8(content.into(), MAX_CONTEXT_PACK_ENTRY_BYTES);
        Self {
            kind,
            label,
            content,
            truncated,
        }
    }

    pub(super) fn label(&self) -> &str {
        &self.label
    }

    pub(super) fn content(&self) -> &str {
        &self.content
    }

    pub(super) fn truncated(&self) -> bool {
        self.truncated
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(super) struct ContextPack {
    entries: Vec<ContextPackEntry>,
}

impl ContextPack {
    pub(super) fn new() -> Self {
        Self::default()
    }

    pub(super) fn from_entries(entries: impl IntoIterator<Item = ContextPackEntry>) -> Self {
        let mut pack = Self::new();
        for entry in entries {
            pack.push(entry);
        }
        pack
    }

    pub(super) fn push(&mut self, entry: ContextPackEntry) {
        if self.entries.len() < MAX_CONTEXT_PACK_ENTRIES {
            self.entries.push(entry);
        }
    }

    pub(super) fn entries(&self) -> &[ContextPackEntry] {
        &self.entries
    }

    pub(super) fn entries_for_kind(&self, kind: ContextPackKind) -> Vec<&ContextPackEntry> {
        self.entries
            .iter()
            .filter(|entry| entry.kind == kind)
            .collect()
    }

    pub(super) fn approximate_token_count(&self) -> usize {
        self.entries
            .iter()
            .map(|entry| {
                approximate_token_count(entry.label()) + approximate_token_count(entry.content())
            })
            .sum()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct WorkerContract {
    pub(super) worker_kind: WorkerKind,
    pub(super) task_kind: TaskKind,
    pub(super) deliverable_kind: ObjectiveDeliverableKind,
    pub(super) evidence_kind: ObjectiveEvidenceKind,
    pub(super) context_pack: ContextPack,
}

impl WorkerContract {
    pub(super) fn from_task_contract(contract: &TaskContract, worker_kind: WorkerKind) -> Self {
        let objective = contract.objective_contract();
        let context_pack = ContextPack::from_entries([
            ContextPackEntry::new(
                ContextPackKind::Contract,
                "task_kind",
                objective.task_kind.as_str(),
            ),
            ContextPackEntry::new(
                ContextPackKind::Contract,
                "deliverable_kind",
                objective.deliverable_kind.label(),
            ),
            ContextPackEntry::new(
                ContextPackKind::Evidence,
                "evidence_kind",
                objective.evidence_kind.label(),
            ),
        ]);
        Self {
            worker_kind,
            task_kind: objective.task_kind,
            deliverable_kind: objective.deliverable_kind,
            evidence_kind: objective.evidence_kind,
            context_pack,
        }
    }

    pub(super) fn primary_for_task_contract(contract: &TaskContract) -> Self {
        let objective = contract.objective_contract();
        Self::from_task_contract(
            contract,
            WorkerKind::primary_for_task_kind(objective.task_kind),
        )
    }

    pub(super) fn with_context_pack(mut self, context_pack: ContextPack) -> Self {
        self.context_pack = context_pack;
        self
    }
}

pub(super) fn capability_spec_for_task_kind(task_kind: TaskKind) -> CapabilitySpec {
    capability_defaults_for_task_kind(task_kind).into_spec()
}

pub(super) fn capability_spec_for_task_contract(contract: &TaskContract) -> CapabilitySpec {
    let objective = contract.objective_contract();
    let mut defaults = capability_defaults_for_task_kind(objective.task_kind);
    defaults.deliverable_kind = objective.deliverable_kind;
    defaults.evidence_kind = objective.evidence_kind;
    if !contract.required_artifacts.is_empty() {
        defaults.required_artifacts = contract.required_artifacts.clone();
    }
    defaults.into_spec()
}

pub(super) fn worker_lifecycle_plan_for_task_contract(
    contract: &TaskContract,
) -> WorkerLifecyclePlan {
    let capability = capability_spec_for_task_contract(contract);
    let stages = [
        CapabilityLifecycleStageKind::Deliverable,
        CapabilityLifecycleStageKind::Evidence,
        CapabilityLifecycleStageKind::Repair,
        CapabilityLifecycleStageKind::ToolFailure,
    ]
    .into_iter()
    .map(|stage_kind| lifecycle_stage_plan_for_contract(contract, &capability, stage_kind))
    .collect();
    WorkerLifecyclePlan { capability, stages }
}

fn capability_defaults_for_task_kind(task_kind: TaskKind) -> CapabilityDefaults {
    match task_kind {
        TaskKind::Coding => CapabilityDefaults {
            task_kind,
            deliverable_kind: ObjectiveDeliverableKind::SourceFiles,
            evidence_kind: ObjectiveEvidenceKind::TestRun,
            required_artifacts: vec![ArtifactRole::Implementation, ArtifactRole::Test],
            allowed_tools: vec![
                CapabilityAllowedTool::Read,
                CapabilityAllowedTool::Write,
                CapabilityAllowedTool::Edit,
                CapabilityAllowedTool::Bash,
            ],
            context_policy: CapabilityContextPolicy::TargetedArtifact,
            worker_sequence: vec![
                WorkerKind::Implement,
                WorkerKind::TestAuthor,
                WorkerKind::Evidence,
                WorkerKind::DiagnosticRepair,
            ],
            repair_strategies: generic_repair_strategies(),
            completion_predicate: CapabilityCompletionPredicate::TestRunPassed,
        },
        TaskKind::Docs => CapabilityDefaults {
            task_kind,
            deliverable_kind: ObjectiveDeliverableKind::DocumentSections,
            evidence_kind: ObjectiveEvidenceKind::ContentCheck,
            required_artifacts: vec![ArtifactRole::UsageDocs],
            allowed_tools: vec![
                CapabilityAllowedTool::Read,
                CapabilityAllowedTool::Write,
                CapabilityAllowedTool::Edit,
            ],
            context_policy: CapabilityContextPolicy::TargetedArtifact,
            worker_sequence: vec![
                WorkerKind::Docs,
                WorkerKind::Evidence,
                WorkerKind::DiagnosticRepair,
            ],
            repair_strategies: generic_repair_strategies(),
            completion_predicate: CapabilityCompletionPredicate::RequiredSectionsPresent,
        },
        TaskKind::Data => CapabilityDefaults {
            task_kind,
            deliverable_kind: ObjectiveDeliverableKind::OutputFile,
            evidence_kind: ObjectiveEvidenceKind::SchemaCheck,
            required_artifacts: vec![ArtifactRole::DataOutput],
            allowed_tools: vec![
                CapabilityAllowedTool::Read,
                CapabilityAllowedTool::Write,
                CapabilityAllowedTool::Edit,
                CapabilityAllowedTool::Bash,
            ],
            context_policy: CapabilityContextPolicy::TargetedArtifact,
            worker_sequence: vec![
                WorkerKind::Data,
                WorkerKind::Evidence,
                WorkerKind::DiagnosticRepair,
            ],
            repair_strategies: generic_repair_strategies(),
            completion_predicate: CapabilityCompletionPredicate::SchemaCheckPassed,
        },
        TaskKind::Research => CapabilityDefaults {
            task_kind,
            deliverable_kind: ObjectiveDeliverableKind::ResearchNotes,
            evidence_kind: ObjectiveEvidenceKind::SourceFetchEvidence,
            required_artifacts: vec![ArtifactRole::UsageDocs],
            allowed_tools: vec![
                CapabilityAllowedTool::Read,
                CapabilityAllowedTool::Write,
                CapabilityAllowedTool::Edit,
                CapabilityAllowedTool::Bash,
            ],
            context_policy: CapabilityContextPolicy::EvidenceBounded,
            worker_sequence: vec![
                WorkerKind::Research,
                WorkerKind::Evidence,
                WorkerKind::DiagnosticRepair,
            ],
            repair_strategies: generic_repair_strategies(),
            completion_predicate: CapabilityCompletionPredicate::SourceEvidencePresent,
        },
        TaskKind::Ops => CapabilityDefaults {
            task_kind,
            deliverable_kind: ObjectiveDeliverableKind::CommandObservation,
            evidence_kind: ObjectiveEvidenceKind::SafetyBoundaryEvidence,
            required_artifacts: vec![ArtifactRole::UsageDocs],
            allowed_tools: vec![
                CapabilityAllowedTool::Read,
                CapabilityAllowedTool::Write,
                CapabilityAllowedTool::Edit,
                CapabilityAllowedTool::Bash,
            ],
            context_policy: CapabilityContextPolicy::ProcedureBounded,
            worker_sequence: vec![
                WorkerKind::Ops,
                WorkerKind::Evidence,
                WorkerKind::DiagnosticRepair,
            ],
            repair_strategies: generic_repair_strategies(),
            completion_predicate: CapabilityCompletionPredicate::CommandObservationRecorded,
        },
        TaskKind::Authoring => CapabilityDefaults {
            task_kind,
            deliverable_kind: ObjectiveDeliverableKind::ProseArtifact,
            evidence_kind: ObjectiveEvidenceKind::ContentAcceptance,
            required_artifacts: vec![ArtifactRole::UsageDocs],
            allowed_tools: vec![
                CapabilityAllowedTool::Read,
                CapabilityAllowedTool::Write,
                CapabilityAllowedTool::Edit,
            ],
            context_policy: CapabilityContextPolicy::TargetedArtifact,
            worker_sequence: vec![
                WorkerKind::Authoring,
                WorkerKind::Evidence,
                WorkerKind::DiagnosticRepair,
            ],
            repair_strategies: generic_repair_strategies(),
            completion_predicate: CapabilityCompletionPredicate::ContentAccepted,
        },
    }
}

fn generic_repair_strategies() -> Vec<CapabilityRepairStrategy> {
    vec![
        CapabilityRepairStrategy::CompleteMissingDeliverable,
        CapabilityRepairStrategy::CreateMissingEvidence,
        CapabilityRepairStrategy::RepairFailedEvidence,
        CapabilityRepairStrategy::ResolveToolFailure,
    ]
}

fn lifecycle_stage_plan_for_contract(
    contract: &TaskContract,
    capability: &CapabilitySpec,
    stage_kind: CapabilityLifecycleStageKind,
) -> CapabilityLifecycleStagePlan {
    let worker_kind = worker_kind_for_lifecycle_stage(capability, stage_kind);
    let context_policy = context_policy_for_lifecycle_stage(capability, stage_kind);
    let repair_strategy = repair_strategy_for_lifecycle_stage(stage_kind);
    let worker_contract = worker_contract_for_capability_stage(
        contract,
        capability,
        worker_kind,
        stage_kind,
        context_policy,
        repair_strategy,
    );
    let output_contract = output_contract_for_lifecycle_stage(capability, stage_kind);
    let eval_label = format!(
        "stage={};recovery_job={};worker_kind={};task_kind={}",
        stage_kind.label(),
        stage_kind.recovery_job_label(),
        worker_kind.label(),
        capability.task_kind.as_str()
    );
    let policy_message = policy_message_for_lifecycle_stage(capability, stage_kind, worker_kind);
    CapabilityLifecycleStagePlan {
        stage_kind,
        worker_contract,
        context_policy,
        repair_strategy,
        output_contract,
        eval_label,
        policy_message,
    }
}

fn worker_kind_for_lifecycle_stage(
    capability: &CapabilitySpec,
    stage_kind: CapabilityLifecycleStageKind,
) -> WorkerKind {
    match stage_kind {
        CapabilityLifecycleStageKind::Deliverable => capability.worker_sequence[0],
        CapabilityLifecycleStageKind::Evidence => WorkerKind::Evidence,
        CapabilityLifecycleStageKind::Repair => WorkerKind::DiagnosticRepair,
        CapabilityLifecycleStageKind::ToolFailure => match capability.task_kind {
            TaskKind::Ops => WorkerKind::Ops,
            _ => WorkerKind::DiagnosticRepair,
        },
    }
}

fn context_policy_for_lifecycle_stage(
    capability: &CapabilitySpec,
    stage_kind: CapabilityLifecycleStageKind,
) -> CapabilityContextPolicy {
    match stage_kind {
        CapabilityLifecycleStageKind::Deliverable => capability.context_policy,
        CapabilityLifecycleStageKind::Evidence => CapabilityContextPolicy::EvidenceBounded,
        CapabilityLifecycleStageKind::Repair => CapabilityContextPolicy::DiagnosticBounded,
        CapabilityLifecycleStageKind::ToolFailure => CapabilityContextPolicy::ProcedureBounded,
    }
}

fn repair_strategy_for_lifecycle_stage(
    stage_kind: CapabilityLifecycleStageKind,
) -> CapabilityRepairStrategy {
    match stage_kind {
        CapabilityLifecycleStageKind::Deliverable => {
            CapabilityRepairStrategy::CompleteMissingDeliverable
        }
        CapabilityLifecycleStageKind::Evidence => CapabilityRepairStrategy::CreateMissingEvidence,
        CapabilityLifecycleStageKind::Repair => CapabilityRepairStrategy::RepairFailedEvidence,
        CapabilityLifecycleStageKind::ToolFailure => CapabilityRepairStrategy::ResolveToolFailure,
    }
}

fn worker_contract_for_capability_stage(
    contract: &TaskContract,
    capability: &CapabilitySpec,
    worker_kind: WorkerKind,
    stage_kind: CapabilityLifecycleStageKind,
    context_policy: CapabilityContextPolicy,
    repair_strategy: CapabilityRepairStrategy,
) -> WorkerContract {
    let mut worker_contract = WorkerContract::from_task_contract(contract, worker_kind);
    let mut context_pack = worker_contract.context_pack;
    context_pack.push(ContextPackEntry::new(
        ContextPackKind::Contract,
        "capability_stage",
        stage_kind.label(),
    ));
    context_pack.push(ContextPackEntry::new(
        ContextPackKind::Contract,
        "context_policy",
        context_policy.label(),
    ));
    context_pack.push(ContextPackEntry::new(
        ContextPackKind::Contract,
        "required_artifacts",
        capability
            .required_artifacts
            .iter()
            .map(|role| role.label())
            .collect::<Vec<_>>()
            .join(","),
    ));
    context_pack.push(ContextPackEntry::new(
        ContextPackKind::Contract,
        "allowed_tools",
        capability
            .allowed_tools
            .iter()
            .map(|tool| tool.label())
            .collect::<Vec<_>>()
            .join(","),
    ));
    context_pack.push(ContextPackEntry::new(
        ContextPackKind::Repair,
        "repair_strategy",
        repair_strategy.label(),
    ));
    worker_contract.context_pack = context_pack;
    worker_contract
}

fn output_contract_for_lifecycle_stage(
    capability: &CapabilitySpec,
    stage_kind: CapabilityLifecycleStageKind,
) -> &'static str {
    match stage_kind {
        CapabilityLifecycleStageKind::Deliverable => match capability.task_kind {
            TaskKind::Coding => "source_files_created_or_updated_with_no_unrelated_changes",
            TaskKind::Docs => "required_document_sections_created_or_updated",
            TaskKind::Data => "structured_output_file_created_or_updated",
            TaskKind::Research => "research_report_or_notes_created_with_source_slots",
            TaskKind::Ops => "command_observation_or_runbook_created_with_safety_notes",
            TaskKind::Authoring => "prose_artifact_created_or_updated_for_requested_audience",
        },
        CapabilityLifecycleStageKind::Evidence => match capability.task_kind {
            TaskKind::Coding => "test_run_evidence_bound_to_owned_artifacts",
            TaskKind::Docs => "content_check_evidence_for_required_sections",
            TaskKind::Data => "schema_or_record_count_evidence_for_output_file",
            TaskKind::Research => "source_fetch_or_citation_evidence_for_research_artifact",
            TaskKind::Ops => "command_observation_and_safety_boundary_evidence",
            TaskKind::Authoring => "content_acceptance_evidence_for_prose_artifact",
        },
        CapabilityLifecycleStageKind::Repair => "single_bounded_repair_for_declared_stage_failure",
        CapabilityLifecycleStageKind::ToolFailure => {
            "tool_or_environment_failure_observed_and_recovered_before_completion"
        }
    }
}

fn policy_message_for_lifecycle_stage(
    capability: &CapabilitySpec,
    stage_kind: CapabilityLifecycleStageKind,
    worker_kind: WorkerKind,
) -> String {
    format!(
        "[{worker}] {recovery_job} owns this turn for task_kind={task_kind}. Produce {deliverable_kind}, gather {evidence_kind}, use context_policy={context_policy}, and satisfy completion_predicate={predicate}. Next action: {next_action}.",
        worker = worker_kind.label(),
        recovery_job = stage_kind.recovery_job_label(),
        task_kind = capability.task_kind.as_str(),
        deliverable_kind = capability.deliverable_kind.label(),
        evidence_kind = capability.evidence_kind.label(),
        context_policy = context_policy_for_lifecycle_stage(capability, stage_kind).label(),
        predicate = capability.completion_predicate.label(),
        next_action = next_action_for_lifecycle_stage(capability.task_kind, stage_kind),
    )
}

fn next_action_for_lifecycle_stage(
    task_kind: TaskKind,
    stage_kind: CapabilityLifecycleStageKind,
) -> &'static str {
    match (task_kind, stage_kind) {
        (TaskKind::Coding, CapabilityLifecycleStageKind::Evidence) => {
            "run the local test command tied to owned test artifacts"
        }
        (TaskKind::Coding, CapabilityLifecycleStageKind::Repair) => {
            "apply one bounded code or test repair, then rerun local tests"
        }
        (_, CapabilityLifecycleStageKind::Deliverable) => {
            "create or update only the declared deliverable artifacts"
        }
        (_, CapabilityLifecycleStageKind::Evidence) => {
            "record deterministic local evidence for the declared artifact"
        }
        (_, CapabilityLifecycleStageKind::Repair) => {
            "apply one bounded artifact repair, then rerun the evidence check"
        }
        (_, CapabilityLifecycleStageKind::ToolFailure) => {
            "resolve the tool, permission, or environment problem before claiming completion"
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct TestAuthorWorkerRequest {
    pub(super) worker_contract: WorkerContract,
    target_test_path: PathBuf,
    allowed_write_scope: Vec<PathBuf>,
    evidence_command: String,
    output_contract: &'static str,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum EvidenceFailureKind {
    CompileError,
    ImportNameMismatch,
    SignatureMismatch,
    AssertionMismatch,
    SetupManifestMissing,
    SchemaMismatch,
    ContentSectionMissing,
    SourceEvidenceMissing,
    Unknown,
}

impl EvidenceFailureKind {
    pub(super) fn label(self) -> &'static str {
        match self {
            EvidenceFailureKind::CompileError => "compile_error",
            EvidenceFailureKind::ImportNameMismatch => "import_name_mismatch",
            EvidenceFailureKind::SignatureMismatch => "signature_mismatch",
            EvidenceFailureKind::AssertionMismatch => "assertion_mismatch",
            EvidenceFailureKind::SetupManifestMissing => "setup_manifest_missing",
            EvidenceFailureKind::SchemaMismatch => "schema_mismatch",
            EvidenceFailureKind::ContentSectionMissing => "content_section_missing",
            EvidenceFailureKind::SourceEvidenceMissing => "source_evidence_missing",
            EvidenceFailureKind::Unknown => "unknown",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum DiagnosticRepairTargetRole {
    Implementation,
    TestArtifact,
    Manifest,
    Docs,
    Data,
    ResearchEvidence,
    OpsProcedure,
    Unknown,
}

impl DiagnosticRepairTargetRole {
    pub(super) fn label(self) -> &'static str {
        match self {
            DiagnosticRepairTargetRole::Implementation => "implementation",
            DiagnosticRepairTargetRole::TestArtifact => "test_artifact",
            DiagnosticRepairTargetRole::Manifest => "manifest",
            DiagnosticRepairTargetRole::Docs => "docs",
            DiagnosticRepairTargetRole::Data => "data",
            DiagnosticRepairTargetRole::ResearchEvidence => "research_evidence",
            DiagnosticRepairTargetRole::OpsProcedure => "ops_procedure",
            DiagnosticRepairTargetRole::Unknown => "unknown",
        }
    }

    fn from_artifact_role(role: ArtifactRole, task_kind: TaskKind) -> Self {
        match role {
            ArtifactRole::Implementation => DiagnosticRepairTargetRole::Implementation,
            ArtifactRole::Test => DiagnosticRepairTargetRole::TestArtifact,
            ArtifactRole::Setup => DiagnosticRepairTargetRole::Manifest,
            ArtifactRole::UsageDocs => match task_kind {
                TaskKind::Research => DiagnosticRepairTargetRole::ResearchEvidence,
                TaskKind::Ops => DiagnosticRepairTargetRole::OpsProcedure,
                _ => DiagnosticRepairTargetRole::Docs,
            },
            ArtifactRole::DataOutput => DiagnosticRepairTargetRole::Data,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct DiagnosticRepairWorkerRequest {
    pub(super) worker_contract: WorkerContract,
    failure_kind: EvidenceFailureKind,
    target_role: DiagnosticRepairTargetRole,
    target_path: PathBuf,
    allowed_change_kind: String,
    evidence_command: String,
    diagnostic: String,
    output_contract: &'static str,
}

impl DiagnosticRepairWorkerRequest {
    pub(super) fn failure_kind(&self) -> EvidenceFailureKind {
        self.failure_kind
    }

    pub(super) fn target_role(&self) -> DiagnosticRepairTargetRole {
        self.target_role
    }

    pub(super) fn target_path(&self) -> &Path {
        &self.target_path
    }

    pub(super) fn allowed_change_kind(&self) -> &str {
        &self.allowed_change_kind
    }

    pub(super) fn evidence_command(&self) -> &str {
        &self.evidence_command
    }

    pub(super) fn diagnostic(&self) -> &str {
        &self.diagnostic
    }

    pub(super) fn output_contract(&self) -> &'static str {
        self.output_contract
    }

    pub(super) fn policy_message(&self, next_required_action: &str) -> String {
        let target =
            super::task_contract::mask_and_cap_recovery_field(&self.target_path.to_string_lossy());
        let diagnostic = super::task_contract::mask_and_cap_recovery_field(&self.diagnostic);
        let evidence_command =
            super::task_contract::mask_and_cap_recovery_field(&self.evidence_command);
        format!(
            "[DiagnosticRepairWorker] EvidenceFailedJob owns this turn. failure_kind={failure_kind}; target_role={target_role}; target={target}; allowed_change_kind={allowed_change_kind}; evidence_command={evidence_command}. Diagnostic: {diagnostic}. Next required action: {next_required_action}. Keep the change bounded to that target and failure. Do not switch files, run verification, or finish with prose.",
            failure_kind = self.failure_kind.label(),
            target_role = self.target_role.label(),
            allowed_change_kind = self.allowed_change_kind,
        )
    }
}

impl TestAuthorWorkerRequest {
    pub(super) fn target_test_path(&self) -> &Path {
        &self.target_test_path
    }

    pub(super) fn allowed_write_scope(&self) -> &[PathBuf] {
        &self.allowed_write_scope
    }

    pub(super) fn evidence_command(&self) -> &str {
        &self.evidence_command
    }

    pub(super) fn output_contract(&self) -> &'static str {
        self.output_contract
    }

    pub(super) fn policy_message(&self) -> String {
        let target = self.target_test_path.to_string_lossy();
        format!(
            "[TestAuthorWorker] MissingEvidenceJob owns this turn. Create or update `{target}` only, using exactly one Write or Edit tool call. The required evidence command is `{}`. Output runnable tests; do not answer in prose, do not call Bash, and do not change unrelated files.",
            self.evidence_command
        )
    }
}

pub(super) fn test_author_worker_request_for_missing_evidence(
    contract: &TaskContract,
    target_and_stack: Option<(&str, &str)>,
    implementation_context: Option<&str>,
) -> Option<TestAuthorWorkerRequest> {
    let objective = contract.objective_contract();
    if objective.task_kind != TaskKind::Coding
        || objective.deliverable_kind != ObjectiveDeliverableKind::SourceFiles
        || objective.evidence_kind != ObjectiveEvidenceKind::TestRun
    {
        return None;
    }
    let (target_test_path, stack_label) = target_and_stack?;
    let target_test_path = PathBuf::from(target_test_path);
    let evidence_command =
        test_author_evidence_command_for_stack(stack_label, &target_test_path.to_string_lossy());
    let mut context_pack =
        WorkerContract::from_task_contract(contract, WorkerKind::TestAuthor).context_pack;
    context_pack.push(ContextPackEntry::new(
        ContextPackKind::Target,
        "target_test_path",
        target_test_path.to_string_lossy(),
    ));
    context_pack.push(ContextPackEntry::new(
        ContextPackKind::Target,
        "allowed_write_scope",
        target_test_path.to_string_lossy(),
    ));
    context_pack.push(ContextPackEntry::new(
        ContextPackKind::Evidence,
        "evidence_command",
        evidence_command.as_str(),
    ));
    context_pack.push(ContextPackEntry::new(
        ContextPackKind::Target,
        "implementation_context",
        implementation_context.unwrap_or(
            "implementation excerpt unavailable; infer public behavior from the active request and project files",
        ),
    ));
    Some(TestAuthorWorkerRequest {
        worker_contract: WorkerContract::from_task_contract(contract, WorkerKind::TestAuthor)
            .with_context_pack(context_pack),
        target_test_path: target_test_path.clone(),
        allowed_write_scope: vec![target_test_path],
        evidence_command,
        output_contract: "exactly_one_write_or_edit_tool_call_creating_or_updating_the_owned_test_artifact",
    })
}

pub(super) fn test_author_evidence_command_for_stack(
    stack_label: &str,
    target_test_path: &str,
) -> String {
    match stack_label {
        "rust" => "cargo test".to_string(),
        "python" => format!("pytest {target_test_path}"),
        "javascript" => format!("node --test {target_test_path}"),
        "typescript" => "npm test".to_string(),
        _ => "npm test".to_string(),
    }
}

pub(super) fn diagnostic_repair_worker_request_for_evidence_failed(
    contract: &TaskContract,
    diagnostic: &str,
    target_hint: &RecoveryTargetHint,
    allowed_change_kind: &str,
    evidence_command: Option<&str>,
) -> DiagnosticRepairWorkerRequest {
    let objective = contract.objective_contract();
    let failure_kind = classify_evidence_failure_kind(diagnostic);
    let target_role =
        DiagnosticRepairTargetRole::from_artifact_role(target_hint.role, objective.task_kind);
    let target_path = PathBuf::from(target_hint.path.as_str());
    let evidence_command = evidence_command
        .filter(|command| !command.trim().is_empty())
        .unwrap_or("rerun configured evidence command")
        .to_string();
    let allowed_change_kind = if allowed_change_kind.trim().is_empty() {
        default_allowed_change_kind_for_target_role(target_role)
    } else {
        allowed_change_kind.trim().to_string()
    };
    let mut context_pack =
        WorkerContract::from_task_contract(contract, WorkerKind::DiagnosticRepair).context_pack;
    context_pack.push(ContextPackEntry::new(
        ContextPackKind::Target,
        "target_role",
        target_role.label(),
    ));
    context_pack.push(ContextPackEntry::new(
        ContextPackKind::Target,
        "target_path",
        target_path.to_string_lossy(),
    ));
    context_pack.push(ContextPackEntry::new(
        ContextPackKind::Repair,
        "allowed_change_kind",
        allowed_change_kind.as_str(),
    ));
    context_pack.push(ContextPackEntry::new(
        ContextPackKind::Evidence,
        "evidence_command",
        evidence_command.as_str(),
    ));
    context_pack.push(ContextPackEntry::new(
        ContextPackKind::Diagnostic,
        "diagnostic",
        format!("failure_kind={}; {diagnostic}", failure_kind.label()),
    ));
    DiagnosticRepairWorkerRequest {
        worker_contract: WorkerContract::from_task_contract(contract, WorkerKind::DiagnosticRepair)
            .with_context_pack(context_pack),
        failure_kind,
        target_role,
        target_path,
        allowed_change_kind,
        evidence_command,
        diagnostic: diagnostic.to_string(),
        output_contract: "single_bounded_repair_tool_action_for_the_declared_target_and_failure",
    }
}

pub(super) fn classify_evidence_failure_kind(diagnostic: &str) -> EvidenceFailureKind {
    let normalized = diagnostic.to_ascii_lowercase();
    if contains_any(
        &normalized,
        &[
            "cargo.toml",
            "package.json",
            "pyproject.toml",
            "requirements.txt",
            "module not found",
            "no such file or directory",
            "could not find manifest",
            "could not find `cargo.toml`",
            "missing manifest",
        ],
    ) {
        return EvidenceFailureKind::SetupManifestMissing;
    }
    if contains_any(
        &normalized,
        &[
            "importerror",
            "modulenotfounderror",
            "cannot find module",
            "does not provide an export",
            "has no exported member",
            "unresolved import",
            "unresolved imports",
            "cannot import name",
            "no named exports",
            "not exported",
        ],
    ) {
        return EvidenceFailureKind::ImportNameMismatch;
    }
    if contains_any(
        &normalized,
        &[
            "wrong number of arguments",
            "takes 0 positional arguments",
            "takes 1 argument",
            "expected function",
            "mismatched types",
            "expected signature",
            "signature mismatch",
            "typeerror:",
            "typeerror",
        ],
    ) {
        return EvidenceFailureKind::SignatureMismatch;
    }
    if contains_any(
        &normalized,
        &[
            "assertionerror",
            "assertion failed",
            "assert_eq!",
            "assert_ne!",
            "expected:",
            "actual:",
            "left:",
            "right:",
            "snapshot mismatch",
            "test failed",
        ],
    ) {
        return EvidenceFailureKind::AssertionMismatch;
    }
    if contains_any(
        &normalized,
        &[
            "schema",
            "jsonschema",
            "csv",
            "missing column",
            "unexpected column",
            "invalid field",
            "invalid record",
        ],
    ) {
        return EvidenceFailureKind::SchemaMismatch;
    }
    if contains_any(
        &normalized,
        &[
            "missing heading",
            "required heading",
            "missing section",
            "required section",
            "content check failed",
        ],
    ) {
        return EvidenceFailureKind::ContentSectionMissing;
    }
    if contains_any(
        &normalized,
        &[
            "source fetch",
            "citation",
            "reference not found",
            "missing source",
            "source evidence",
        ],
    ) {
        return EvidenceFailureKind::SourceEvidenceMissing;
    }
    if contains_any(
        &normalized,
        &[
            "error[e",
            "failed to compile",
            "compilation failed",
            "syntaxerror",
            "tsc",
            "cannot find symbol",
            "expected one of",
            "unterminated",
            "parse error",
        ],
    ) {
        return EvidenceFailureKind::CompileError;
    }
    EvidenceFailureKind::Unknown
}

fn default_allowed_change_kind_for_target_role(role: DiagnosticRepairTargetRole) -> String {
    match role {
        DiagnosticRepairTargetRole::Implementation => "implementation".to_string(),
        DiagnosticRepairTargetRole::TestArtifact => "test".to_string(),
        DiagnosticRepairTargetRole::Manifest => "manifest".to_string(),
        DiagnosticRepairTargetRole::Docs => "docs".to_string(),
        DiagnosticRepairTargetRole::Data => "data_schema".to_string(),
        DiagnosticRepairTargetRole::ResearchEvidence => "research_evidence".to_string(),
        DiagnosticRepairTargetRole::OpsProcedure => "ops_procedure".to_string(),
        DiagnosticRepairTargetRole::Unknown => "bounded_target_repair".to_string(),
    }
}

fn contains_any(haystack: &str, needles: &[&str]) -> bool {
    needles.iter().any(|needle| haystack.contains(needle))
}

fn truncate_utf8(value: String, max_bytes: usize) -> (String, bool) {
    if value.len() <= max_bytes {
        return (value, false);
    }
    let mut end = max_bytes;
    while end > 0 && !value.is_char_boundary(end) {
        end -= 1;
    }
    let mut truncated = value[..end].to_string();
    truncated.push_str(TRUNCATION_MARKER);
    (truncated, true)
}

fn approximate_token_count(text: &str) -> usize {
    text.split_whitespace().count().max(text.len().div_ceil(4))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn worker_kind_labels_cover_current_worker_intents() {
        let labels = [
            WorkerKind::Implement.label(),
            WorkerKind::TestAuthor.label(),
            WorkerKind::Evidence.label(),
            WorkerKind::DiagnosticRepair.label(),
            WorkerKind::Docs.label(),
            WorkerKind::Data.label(),
            WorkerKind::Research.label(),
            WorkerKind::Ops.label(),
            WorkerKind::Authoring.label(),
        ];
        assert_eq!(
            labels,
            [
                "implement",
                "test_author",
                "evidence",
                "diagnostic_repair",
                "docs",
                "data",
                "research",
                "ops",
                "authoring"
            ]
        );
    }

    #[test]
    fn worker_contract_carries_objective_vocabulary_for_coding_test_author() {
        let contract = TaskContract::from_request(
            "Create a Rust CLI word counter with Cargo tests and usage docs.",
        );
        let worker = WorkerContract::from_task_contract(&contract, WorkerKind::TestAuthor);

        assert_eq!(worker.worker_kind, WorkerKind::TestAuthor);
        assert_eq!(worker.task_kind, TaskKind::Coding);
        assert_eq!(
            worker.deliverable_kind,
            ObjectiveDeliverableKind::SourceFiles
        );
        assert_eq!(worker.evidence_kind, ObjectiveEvidenceKind::TestRun);
        assert_eq!(
            worker
                .context_pack
                .entries_for_kind(ContextPackKind::Contract)
                .len(),
            2
        );
        assert_eq!(
            worker
                .context_pack
                .entries_for_kind(ContextPackKind::Evidence)
                .len(),
            1
        );
    }

    #[test]
    fn primary_worker_contract_covers_non_coding_task_kinds() {
        let cases = [
            (
                "Write README.md with prerequisites, rollback, validation, and incident sections.",
                WorkerKind::Docs,
                ObjectiveDeliverableKind::DocumentSections,
                ObjectiveEvidenceKind::ContentCheck,
            ),
            (
                "Transform orders.csv into cleaned output.csv with id,total columns.",
                WorkerKind::Data,
                ObjectiveDeliverableKind::OutputFile,
                ObjectiveEvidenceKind::SchemaCheck,
            ),
            (
                "Research local LLM repair loops and draft a report with sources.",
                WorkerKind::Research,
                ObjectiveDeliverableKind::ResearchNotes,
                ObjectiveEvidenceKind::SourceFetchEvidence,
            ),
        ];

        for (request, expected_worker, expected_deliverable, expected_evidence) in cases {
            let contract = TaskContract::from_request(request);
            let worker = WorkerContract::primary_for_task_contract(&contract);
            assert_eq!(worker.worker_kind, expected_worker, "{request}");
            assert_eq!(worker.deliverable_kind, expected_deliverable, "{request}");
            assert_eq!(worker.evidence_kind, expected_evidence, "{request}");
        }
    }

    #[test]
    fn context_pack_bounds_entries_and_filters_by_kind() {
        let long = "x".repeat(MAX_CONTEXT_PACK_ENTRY_BYTES + 32);
        let entries = (0..MAX_CONTEXT_PACK_ENTRIES + 3).map(|idx| {
            let kind = if idx % 2 == 0 {
                ContextPackKind::Target
            } else {
                ContextPackKind::Repair
            };
            ContextPackEntry::new(kind, format!("entry-{idx}"), long.clone())
        });
        let pack = ContextPack::from_entries(entries);

        assert_eq!(pack.entries().len(), MAX_CONTEXT_PACK_ENTRIES);
        assert!(pack.entries()[0].truncated());
        assert!(
            pack.entries()[0].content().len()
                <= MAX_CONTEXT_PACK_ENTRY_BYTES + TRUNCATION_MARKER.len()
        );
        assert_eq!(
            pack.entries_for_kind(ContextPackKind::Target).len(),
            MAX_CONTEXT_PACK_ENTRIES / 2
        );
        assert!(pack.approximate_token_count() > 0);
    }

    #[test]
    fn test_author_worker_request_uses_coding_objective_and_bounded_context() {
        let contract = TaskContract::from_request(
            "Create a Rust library crate and verify it with cargo test.",
        );
        let request = test_author_worker_request_for_missing_evidence(
            &contract,
            Some(("tests/lib.rs", "rust")),
            Some("pub fn slugify(input: &str) -> String"),
        )
        .expect("coding request should create test author request");

        assert_eq!(request.worker_contract.worker_kind, WorkerKind::TestAuthor);
        assert_eq!(request.worker_contract.task_kind, TaskKind::Coding);
        assert_eq!(
            request.worker_contract.deliverable_kind,
            ObjectiveDeliverableKind::SourceFiles
        );
        assert_eq!(
            request.worker_contract.evidence_kind,
            ObjectiveEvidenceKind::TestRun
        );
        assert_eq!(request.target_test_path(), Path::new("tests/lib.rs"));
        assert_eq!(
            request.allowed_write_scope(),
            &[PathBuf::from("tests/lib.rs")]
        );
        assert_eq!(request.evidence_command(), "cargo test");
        assert!(request.policy_message().contains("TestAuthorWorker"));
        assert!(
            request
                .worker_contract
                .context_pack
                .entries_for_kind(ContextPackKind::Target)
                .len()
                >= 3
        );
    }

    #[test]
    fn test_author_worker_request_covers_rust_node_and_python_target_synthesis() {
        let cases = [
            (
                "Create a Rust library and verify behavior with cargo test.",
                "tests/lib.rs",
                "cargo test",
            ),
            (
                "Build a Node JSON formatter CLI and add npm test coverage.",
                "tests/main.test.js",
                "node --test tests/main.test.js",
            ),
            (
                "Create a Python sales CLI and verify it with pytest.",
                "tests/test_main.py",
                "pytest tests/test_main.py",
            ),
        ];

        for (active_request, expected_target, expected_command) in cases {
            let contract = TaskContract::from_request(active_request);
            let target_and_stack =
                super::super::verifier_orchestration::synthesized_missing_test_target_path_for_request(
                    active_request,
                );
            let request =
                test_author_worker_request_for_missing_evidence(&contract, target_and_stack, None)
                    .expect(active_request);

            assert_eq!(request.target_test_path(), Path::new(expected_target));
            assert_eq!(request.evidence_command(), expected_command);
            assert_eq!(
                request.output_contract(),
                "exactly_one_write_or_edit_tool_call_creating_or_updating_the_owned_test_artifact"
            );
        }
    }

    #[test]
    fn test_author_worker_request_keeps_non_coding_tasks_out_of_coding_verifier_vocabulary() {
        let docs =
            TaskContract::from_request("Write README.md with install and rollback sections.");
        let request = test_author_worker_request_for_missing_evidence(
            &docs,
            Some(("tests/main.test.js", "javascript")),
            None,
        );

        assert!(request.is_none());
    }

    #[test]
    fn evidence_failure_classifier_covers_coding_and_non_coding_runner_failures() {
        let cases = [
            (
                "error[E0425]: cannot find function `slugify` in this scope",
                EvidenceFailureKind::CompileError,
            ),
            (
                "ImportError: cannot import name 'slugify' from 'app'",
                EvidenceFailureKind::ImportNameMismatch,
            ),
            (
                "SyntaxError: The requested module './main.js' does not provide an export named 'formatJson'",
                EvidenceFailureKind::ImportNameMismatch,
            ),
            (
                "TypeError: slugify() takes 1 argument but 2 were given",
                EvidenceFailureKind::SignatureMismatch,
            ),
            (
                "AssertionError: expected: clean-slug actual: clean_slug",
                EvidenceFailureKind::AssertionMismatch,
            ),
            (
                "CSV schema check failed: missing column total",
                EvidenceFailureKind::SchemaMismatch,
            ),
            (
                "Content check failed: missing section Rollback",
                EvidenceFailureKind::ContentSectionMissing,
            ),
            (
                "Source evidence missing: citation reference not found",
                EvidenceFailureKind::SourceEvidenceMissing,
            ),
            (
                "could not find `Cargo.toml` in `/tmp/project`",
                EvidenceFailureKind::SetupManifestMissing,
            ),
        ];

        for (diagnostic, expected) in cases {
            assert_eq!(
                classify_evidence_failure_kind(diagnostic),
                expected,
                "{diagnostic}"
            );
        }
    }

    #[test]
    fn diagnostic_repair_worker_request_focuses_rust_compile_failure_on_impl_target() {
        let contract = TaskContract::from_request(
            "Create a Rust slugify library and verify it with cargo test.",
        );
        let target_hint = RecoveryTargetHint {
            role: ArtifactRole::Implementation,
            path: "src/lib.rs".to_string(),
            reason: "compiler points at missing public function".to_string(),
        };
        let request = diagnostic_repair_worker_request_for_evidence_failed(
            &contract,
            "error[E0425]: cannot find function `slugify` in this scope",
            &target_hint,
            "implementation",
            Some("cargo test"),
        );

        assert_eq!(
            request.worker_contract.worker_kind,
            WorkerKind::DiagnosticRepair
        );
        assert_eq!(request.failure_kind(), EvidenceFailureKind::CompileError);
        assert_eq!(
            request.target_role(),
            DiagnosticRepairTargetRole::Implementation
        );
        assert_eq!(request.target_path(), Path::new("src/lib.rs"));
        assert_eq!(request.allowed_change_kind(), "implementation");
        assert_eq!(request.evidence_command(), "cargo test");
        assert_eq!(
            request.output_contract(),
            "single_bounded_repair_tool_action_for_the_declared_target_and_failure"
        );
        assert!(
            request
                .policy_message("exactly one compact Edit on the declared target")
                .contains("DiagnosticRepairWorker")
        );
        let diagnostic_entries = request
            .worker_contract
            .context_pack
            .entries_for_kind(ContextPackKind::Diagnostic);
        assert_eq!(diagnostic_entries.len(), 1);
        assert!(
            diagnostic_entries[0]
                .content()
                .contains("failure_kind=compile_error")
        );
    }

    #[test]
    fn diagnostic_repair_worker_request_distinguishes_test_artifact_self_failure() {
        let contract = TaskContract::from_request(
            "Create a Node JSON formatter CLI and verify it with node --test.",
        );
        let target_hint = RecoveryTargetHint {
            role: ArtifactRole::Test,
            path: "tests/main.test.js".to_string(),
            reason: "test imports the wrong export name".to_string(),
        };
        let request = diagnostic_repair_worker_request_for_evidence_failed(
            &contract,
            "SyntaxError: The requested module '../src/main.js' does not provide an export named 'formatJson'",
            &target_hint,
            "test",
            Some("node --test tests/main.test.js"),
        );

        assert_eq!(
            request.failure_kind(),
            EvidenceFailureKind::ImportNameMismatch
        );
        assert_eq!(
            request.target_role(),
            DiagnosticRepairTargetRole::TestArtifact
        );
        assert_eq!(request.allowed_change_kind(), "test");
        assert_eq!(request.evidence_command(), "node --test tests/main.test.js");
    }

    #[test]
    fn diagnostic_repair_worker_request_projects_non_coding_target_roles() {
        let docs = TaskContract::from_request(
            "Write README.md with prerequisites, rollback, and validation sections.",
        );
        let docs_hint = RecoveryTargetHint {
            role: ArtifactRole::UsageDocs,
            path: "README.md".to_string(),
            reason: "content check found a missing section".to_string(),
        };
        let docs_request = diagnostic_repair_worker_request_for_evidence_failed(
            &docs,
            "Content check failed: missing heading Rollback",
            &docs_hint,
            "",
            Some("content-check README.md"),
        );

        assert_eq!(
            docs_request.failure_kind(),
            EvidenceFailureKind::ContentSectionMissing
        );
        assert_eq!(docs_request.target_role(), DiagnosticRepairTargetRole::Docs);
        assert_eq!(docs_request.allowed_change_kind(), "docs");

        let data = TaskContract::from_request(
            "Transform orders.csv into output.csv with id,total columns.",
        );
        let data_hint = RecoveryTargetHint {
            role: ArtifactRole::DataOutput,
            path: "output.csv".to_string(),
            reason: "schema check found missing total column".to_string(),
        };
        let data_request = diagnostic_repair_worker_request_for_evidence_failed(
            &data,
            "CSV schema check failed: missing column total",
            &data_hint,
            "",
            Some("schema-check output.csv"),
        );

        assert_eq!(
            data_request.failure_kind(),
            EvidenceFailureKind::SchemaMismatch
        );
        assert_eq!(data_request.target_role(), DiagnosticRepairTargetRole::Data);
        assert_eq!(data_request.allowed_change_kind(), "data_schema");
    }

    #[test]
    fn capability_spec_defaults_cover_general_purpose_task_kinds() {
        let cases = [
            (
                TaskKind::Docs,
                ObjectiveDeliverableKind::DocumentSections,
                ObjectiveEvidenceKind::ContentCheck,
                CapabilityCompletionPredicate::RequiredSectionsPresent,
                ArtifactRole::UsageDocs,
            ),
            (
                TaskKind::Data,
                ObjectiveDeliverableKind::OutputFile,
                ObjectiveEvidenceKind::SchemaCheck,
                CapabilityCompletionPredicate::SchemaCheckPassed,
                ArtifactRole::DataOutput,
            ),
            (
                TaskKind::Research,
                ObjectiveDeliverableKind::ResearchNotes,
                ObjectiveEvidenceKind::SourceFetchEvidence,
                CapabilityCompletionPredicate::SourceEvidencePresent,
                ArtifactRole::UsageDocs,
            ),
            (
                TaskKind::Ops,
                ObjectiveDeliverableKind::CommandObservation,
                ObjectiveEvidenceKind::SafetyBoundaryEvidence,
                CapabilityCompletionPredicate::CommandObservationRecorded,
                ArtifactRole::UsageDocs,
            ),
            (
                TaskKind::Authoring,
                ObjectiveDeliverableKind::ProseArtifact,
                ObjectiveEvidenceKind::ContentAcceptance,
                CapabilityCompletionPredicate::ContentAccepted,
                ArtifactRole::UsageDocs,
            ),
        ];

        for (
            task_kind,
            expected_deliverable,
            expected_evidence,
            expected_predicate,
            expected_role,
        ) in cases
        {
            let spec = capability_spec_for_task_kind(task_kind);

            assert_eq!(spec.task_kind, task_kind);
            assert_eq!(spec.deliverable_kind, expected_deliverable);
            assert_eq!(spec.evidence_kind, expected_evidence);
            assert_eq!(spec.completion_predicate, expected_predicate);
            assert!(spec.required_artifacts.contains(&expected_role));
            assert!(spec.worker_sequence.contains(&WorkerKind::Evidence));
            assert!(spec.worker_sequence.contains(&WorkerKind::DiagnosticRepair));
            assert!(
                spec.repair_strategies
                    .contains(&CapabilityRepairStrategy::RepairFailedEvidence)
            );
            assert!(
                spec.eval_labels
                    .iter()
                    .any(|label| label == &format!("task_kind={}", task_kind.as_str()))
            );
        }
    }

    #[test]
    fn non_coding_worker_lifecycle_plans_have_deliverable_evidence_and_repair_stages() {
        let cases = [
            (
                "Write README.md with install, validation, and rollback sections.",
                TaskKind::Docs,
                WorkerKind::Docs,
            ),
            (
                "Transform orders.csv into output.csv with id,total columns.",
                TaskKind::Data,
                WorkerKind::Data,
            ),
            (
                "Investigate local LLM repair loops and produce a report in report.md with sources.",
                TaskKind::Research,
                WorkerKind::Research,
            ),
            (
                "Prepare a deployment runbook with rollback and validation commands.",
                TaskKind::Ops,
                WorkerKind::Ops,
            ),
            (
                "Translate README.ja.md into English and write README.md.",
                TaskKind::Authoring,
                WorkerKind::Authoring,
            ),
        ];

        for (request, expected_kind, expected_primary_worker) in cases {
            let contract = TaskContract::from_request(request);
            assert_eq!(contract.task_kind, expected_kind, "{request}");

            let plan = worker_lifecycle_plan_for_task_contract(&contract);

            assert_eq!(plan.capability.task_kind, expected_kind);
            assert_eq!(plan.stages().len(), 4);
            let deliverable = plan
                .stage(CapabilityLifecycleStageKind::Deliverable)
                .expect("deliverable stage");
            let evidence = plan
                .stage(CapabilityLifecycleStageKind::Evidence)
                .expect("evidence stage");
            let repair = plan
                .stage(CapabilityLifecycleStageKind::Repair)
                .expect("repair stage");

            assert_eq!(
                deliverable.worker_contract.worker_kind,
                expected_primary_worker
            );
            assert_eq!(evidence.worker_contract.worker_kind, WorkerKind::Evidence);
            assert_eq!(
                repair.worker_contract.worker_kind,
                WorkerKind::DiagnosticRepair
            );
            assert_eq!(deliverable.recovery_job_label(), "MissingDeliverableJob");
            assert_eq!(evidence.recovery_job_label(), "MissingEvidenceJob");
            assert_eq!(repair.recovery_job_label(), "EvidenceFailedJob");
            assert_eq!(
                deliverable.worker_contract.deliverable_kind,
                plan.capability.deliverable_kind
            );
            assert_eq!(
                evidence.worker_contract.evidence_kind,
                plan.capability.evidence_kind
            );
            assert!(
                deliverable
                    .worker_contract
                    .context_pack
                    .entries()
                    .iter()
                    .any(|entry| entry.label() == "allowed_tools")
            );
            assert!(
                repair
                    .worker_contract
                    .context_pack
                    .entries_for_kind(ContextPackKind::Repair)
                    .iter()
                    .any(|entry| entry.content().contains("repair_failed_evidence"))
            );
        }
    }

    #[test]
    fn non_coding_lifecycle_represents_expected_evidence_predicates() {
        let docs = worker_lifecycle_plan_for_task_contract(&TaskContract::from_request(
            "Write README.md with setup, usage, and rollback sections.",
        ));
        assert_eq!(
            docs.capability.completion_predicate,
            CapabilityCompletionPredicate::RequiredSectionsPresent
        );
        assert!(
            docs.stage(CapabilityLifecycleStageKind::Evidence)
                .unwrap()
                .output_contract
                .contains("content_check")
        );

        let data = worker_lifecycle_plan_for_task_contract(&TaskContract::from_request(
            "Convert customers.csv to customers.json with id,name fields.",
        ));
        assert_eq!(
            data.capability.completion_predicate,
            CapabilityCompletionPredicate::SchemaCheckPassed
        );
        assert!(
            data.stage(CapabilityLifecycleStageKind::Evidence)
                .unwrap()
                .output_contract
                .contains("schema")
        );

        let research = worker_lifecycle_plan_for_task_contract(&TaskContract::from_request(
            "Investigate local LLM agents and produce a report in report.md with cited sources.",
        ));
        assert_eq!(
            research.capability.completion_predicate,
            CapabilityCompletionPredicate::SourceEvidencePresent
        );
        assert!(
            research
                .stage(CapabilityLifecycleStageKind::Evidence)
                .unwrap()
                .output_contract
                .contains("source_fetch")
        );

        let ops = worker_lifecycle_plan_for_task_contract(&TaskContract::from_request(
            "Prepare a deployment runbook with rollback commands and validation checks.",
        ));
        assert_eq!(
            ops.capability.completion_predicate,
            CapabilityCompletionPredicate::CommandObservationRecorded
        );
        assert!(
            ops.stage(CapabilityLifecycleStageKind::Deliverable)
                .unwrap()
                .output_contract
                .contains("runbook")
        );

        let authoring = worker_lifecycle_plan_for_task_contract(&TaskContract::from_request(
            "Translate README.ja.md into English and write README.md.",
        ));
        assert_eq!(
            authoring.capability.completion_predicate,
            CapabilityCompletionPredicate::ContentAccepted
        );
        assert!(
            authoring
                .stage(CapabilityLifecycleStageKind::Evidence)
                .unwrap()
                .output_contract
                .contains("content_acceptance")
        );
    }

    #[test]
    fn non_coding_policy_messages_avoid_coding_specific_verifier_vocabulary() {
        let requests = [
            "Write README.md with install and rollback sections.",
            "Convert orders.csv into output.csv with id,total columns.",
            "Investigate local-first agent repair loops and produce a report in report.md with sources.",
            "Prepare a deployment runbook with rollback and validation commands.",
            "Translate README.ja.md into English and write README.md.",
        ];

        for request in requests {
            let contract = TaskContract::from_request(request);
            assert_ne!(contract.task_kind, TaskKind::Coding, "{request}");
            let plan = worker_lifecycle_plan_for_task_contract(&contract);
            for stage in plan.stages() {
                let message = stage.policy_message().to_ascii_lowercase();
                assert!(!message.contains("verifier"), "{request}: {message}");
                assert!(!message.contains("cargo"), "{request}: {message}");
                assert!(!message.contains("pytest"), "{request}: {message}");
                assert!(!message.contains("npm test"), "{request}: {message}");
                assert!(!message.contains("node --test"), "{request}: {message}");
            }
        }
    }

    #[test]
    fn lifecycle_stage_context_pack_stays_bounded_and_eval_labeled() {
        let contract = TaskContract::from_request(
            "Transform orders.csv into output.csv with id,total columns.",
        );
        let plan = worker_lifecycle_plan_for_task_contract(&contract);

        for stage in plan.stages() {
            assert!(stage.worker_contract.context_pack.entries().len() <= MAX_CONTEXT_PACK_ENTRIES);
            assert!(stage.eval_label.contains(stage.stage_kind.label()));
            assert!(stage.eval_label.contains(stage.recovery_job_label()));
            assert!(stage.output_contract.len() > 12);
        }
    }
}
