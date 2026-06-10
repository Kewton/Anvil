//! Issue #961: task-specialized worker contracts and bounded context packs.
//!
//! This is an additive foundation. It deliberately does not dispatch workers
//! yet; later MissingEvidence / DiagnosticRepair / non-coding capability
//! tracks can consume the same contract shape without adding provider
//! abstraction or broad prompt/context formats.

#![allow(dead_code)] // Foundation seam; focused tests pin the shape before broad callers are wired.

use std::path::{Path, PathBuf};

use super::api_contract_expectation::{
    ApiContractExpectation, api_contract_delta_summary, api_contract_summary,
};
use super::authoring_style::AuthoringStyleDecision;
use super::repair_target_decision::RepairTargetDeltaKind;
use super::scaffold_pipeline::ScaffoldFramework;
use super::task_contract::{
    ArtifactRole, DeliverableFormat, DeliverableKind, DeliverableSchema, ObjectiveContract,
    ObjectiveDeliverableKind, ObjectiveEvidenceKind, ObjectiveKind, RecoveryTargetHint,
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
    if objective.has_required_deliverables() {
        defaults.required_artifacts = objective.required_deliverables.clone();
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
    repair_delta_kind: RepairTargetDeltaKind,
    ledger_facts: String,
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
    repair_delta_kind: RepairTargetDeltaKind,
    failure_kind: EvidenceFailureKind,
    target_role: DiagnosticRepairTargetRole,
    target_path: PathBuf,
    allowed_change_kind: String,
    evidence_command: String,
    ledger_facts: String,
    api_contract_delta: String,
    diagnostic: String,
    output_contract: &'static str,
}

impl DiagnosticRepairWorkerRequest {
    pub(super) fn failure_kind(&self) -> EvidenceFailureKind {
        self.failure_kind
    }

    pub(super) fn repair_delta_kind(&self) -> RepairTargetDeltaKind {
        self.repair_delta_kind
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

    pub(super) fn ledger_facts(&self) -> &str {
        &self.ledger_facts
    }

    pub(super) fn api_contract_delta(&self) -> &str {
        &self.api_contract_delta
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
        let ledger_facts = super::task_contract::mask_and_cap_recovery_field(&self.ledger_facts);
        let api_contract_delta =
            super::task_contract::mask_and_cap_recovery_field(&self.api_contract_delta);
        format!(
            "[DiagnosticRepairWorker] EvidenceFailedJob owns this turn. repair_delta={repair_delta}; ledger_facts={ledger_facts}; api_contract_delta={api_contract_delta}; failure_kind={failure_kind}; target_role={target_role}; target={target}; allowed_change_kind={allowed_change_kind}; evidence_command={evidence_command}. Treat repair_delta, ledger_facts, and api_contract_delta as primary control data; treat Diagnostic as auxiliary failure text. Diagnostic: {diagnostic}. Next required action: {next_required_action}. Keep the change bounded to that target and failure. Do not switch files, run verification, or finish with prose.",
            repair_delta = self.repair_delta_kind.as_str(),
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

    pub(super) fn repair_delta_kind(&self) -> RepairTargetDeltaKind {
        self.repair_delta_kind
    }

    pub(super) fn ledger_facts(&self) -> &str {
        &self.ledger_facts
    }

    pub(super) fn output_contract(&self) -> &'static str {
        self.output_contract
    }

    pub(super) fn policy_message(&self) -> String {
        let target = self.target_test_path.to_string_lossy();
        format!(
            "[TestAuthorWorker] MissingEvidenceJob owns this turn. repair_delta={}; ledger_facts={}. Create or update `{target}` only, using exactly one Write or Edit tool call. The required evidence command is `{}`. Output runnable tests; do not answer in prose, do not call Bash, and do not change unrelated files.",
            self.repair_delta_kind.as_str(),
            self.ledger_facts,
            self.evidence_command,
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
    let repair_delta_kind = RepairTargetDeltaKind::MissingEvidence;
    let ledger_facts = format!(
        "target_path_present=true; evidence_command_present=true; implementation_context_present={}",
        implementation_context.is_some()
    );
    context_pack.push(ContextPackEntry::new(
        ContextPackKind::Repair,
        "repair_delta",
        format!(
            "repair_delta_kind={}; ledger_facts={ledger_facts}",
            repair_delta_kind.as_str()
        ),
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
        repair_delta_kind,
        ledger_facts,
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
    let repair_delta_kind = diagnostic_repair_delta_kind(failure_kind, target_role);
    let api_contract_delta =
        api_contract_delta_summary(&contract.api_contract_expectations, diagnostic)
            .unwrap_or_else(|| "none".to_string());
    let ledger_facts = format!(
        "target_path_present={}; evidence_command_present={}; diagnostic_failure_kind={}",
        !target_hint.path.trim().is_empty(),
        evidence_command.trim() != "rerun configured evidence command",
        failure_kind.label()
    );
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
        format!(
            "allowed_change_kind={}; repair_delta_kind={}; ledger_facts={ledger_facts}",
            allowed_change_kind,
            repair_delta_kind.as_str()
        ),
    ));
    context_pack.push(ContextPackEntry::new(
        ContextPackKind::Repair,
        "api_contract_delta",
        api_contract_delta.as_str(),
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
        repair_delta_kind,
        failure_kind,
        target_role,
        target_path,
        allowed_change_kind,
        evidence_command,
        ledger_facts,
        api_contract_delta,
        diagnostic: diagnostic.to_string(),
        output_contract: "single_bounded_repair_tool_action_for_the_declared_target_and_failure",
    }
}

fn diagnostic_repair_delta_kind(
    failure_kind: EvidenceFailureKind,
    target_role: DiagnosticRepairTargetRole,
) -> RepairTargetDeltaKind {
    match failure_kind {
        EvidenceFailureKind::SetupManifestMissing => RepairTargetDeltaKind::MissingEvidence,
        EvidenceFailureKind::ContentSectionMissing | EvidenceFailureKind::SourceEvidenceMissing => {
            RepairTargetDeltaKind::MissingDeliverable
        }
        EvidenceFailureKind::AssertionMismatch
            if target_role == DiagnosticRepairTargetRole::TestArtifact =>
        {
            RepairTargetDeltaKind::StyleMismatch
        }
        EvidenceFailureKind::CompileError
        | EvidenceFailureKind::ImportNameMismatch
        | EvidenceFailureKind::SignatureMismatch
        | EvidenceFailureKind::AssertionMismatch
        | EvidenceFailureKind::SchemaMismatch
        | EvidenceFailureKind::Unknown => RepairTargetDeltaKind::EvidenceFailed,
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

// ── Issue #1002: TaskExecutionContract + focused worker inputs ─────────────
//
// The classification-layer `ObjectiveContract` says *what* the lifecycle drives
// toward. `TaskExecutionContract` is the *runtime* projection each worker reads:
// it bundles the objective with the runtime profile, declared deliverables, the
// public contract, the evidence requirement, the shared constraints, and the
// repair policy — so a worker's input is the contract plus its target files
// rather than a replay of prior turn logs. All six task kinds ride this one
// lifecycle; runtime-specific differences (stack, scaffold, evidence command)
// are closed into the profile / builder adapters instead of per-kind worker
// types (RustWorker / NodeWorker / …).

/// Runtime/stack profile. Lets one `WorkerKind` set serve every stack rather
/// than spawning per-stack worker types (maintainability requirement).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum RuntimeProfile {
    Rust,
    Python,
    Node,
    TypeScript,
    Unspecified,
}

impl RuntimeProfile {
    pub(super) fn label(self) -> &'static str {
        match self {
            RuntimeProfile::Rust => "rust",
            RuntimeProfile::Python => "python",
            RuntimeProfile::Node => "node",
            RuntimeProfile::TypeScript => "typescript",
            RuntimeProfile::Unspecified => "unspecified",
        }
    }

    /// Map the verifier stack-label vocabulary (`test_author_evidence_command_for_stack`)
    /// into a runtime profile so a caller that already detected a stack can inject it.
    pub(super) fn from_stack_label(stack_label: &str) -> Self {
        match stack_label.trim().to_ascii_lowercase().as_str() {
            "rust" => RuntimeProfile::Rust,
            "python" => RuntimeProfile::Python,
            "javascript" | "node" => RuntimeProfile::Node,
            "typescript" => RuntimeProfile::TypeScript,
            _ => RuntimeProfile::Unspecified,
        }
    }

    fn from_path(path: &Path) -> Option<Self> {
        match path.extension().and_then(|ext| ext.to_str()) {
            Some("rs") => Some(RuntimeProfile::Rust),
            Some("py") => Some(RuntimeProfile::Python),
            Some("ts" | "tsx") => Some(RuntimeProfile::TypeScript),
            Some("js" | "jsx" | "mjs" | "cjs") => Some(RuntimeProfile::Node),
            _ => None,
        }
    }
}

/// Scaffold profile. `None` for tasks that do not provision a project skeleton;
/// `Web(_)` reuses the existing scaffold-framework vocabulary.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum ScaffoldProfile {
    None,
    Web(ScaffoldFramework),
}

impl ScaffoldProfile {
    pub(super) fn label(self) -> &'static str {
        match self {
            ScaffoldProfile::None => "none",
            ScaffoldProfile::Web(framework) => framework.label(),
        }
    }
}

/// One declared deliverable in execution terms: an artifact role plus the
/// concrete target path when the contract pinned one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct ExecutionDeliverable {
    pub(super) role: ArtifactRole,
    pub(super) path: Option<PathBuf>,
    pub(super) kind: Option<DeliverableKind>,
    pub(super) format: Option<DeliverableFormat>,
    pub(super) schema: Option<DeliverableSchema>,
    pub(super) required_sections: Vec<String>,
    pub(super) acceptance_criteria: Vec<String>,
}

const MAX_PUBLIC_CONTRACT_SIGNATURES: usize = 6;

/// The public interface a deliverable must satisfy. Bounded + masked at
/// construction so the worker leads with the contract rather than prior logs.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(super) struct PublicContract {
    goal: Option<String>,
    signatures: Vec<String>,
}

impl PublicContract {
    fn from_parts(goal: Option<String>, signatures: Vec<String>) -> Self {
        let goal = goal
            .map(|value| super::task_contract::mask_and_cap_recovery_field(&value))
            .filter(|value| !value.trim().is_empty());
        let mut contract = Self {
            goal,
            signatures: Vec::new(),
        };
        for signature in signatures {
            contract.push_signature(&signature);
        }
        contract
    }

    pub(super) fn goal(&self) -> Option<&str> {
        self.goal.as_deref()
    }

    pub(super) fn signatures(&self) -> &[String] {
        &self.signatures
    }

    pub(super) fn is_empty(&self) -> bool {
        self.goal.is_none() && self.signatures.is_empty()
    }

    fn summary(&self) -> Option<String> {
        if self.is_empty() {
            return None;
        }
        let mut parts = Vec::new();
        if let Some(goal) = &self.goal {
            parts.push(goal.clone());
        }
        if !self.signatures.is_empty() {
            parts.push(format!("satisfies: {}", self.signatures.join(", ")));
        }
        Some(parts.join("; "))
    }

    fn push_signature(&mut self, signature: &str) {
        if self.signatures.len() >= MAX_PUBLIC_CONTRACT_SIGNATURES {
            return;
        }
        let masked = super::task_contract::mask_and_cap_recovery_field(signature);
        if !masked.trim().is_empty() && !self.signatures.contains(&masked) {
            self.signatures.push(masked);
        }
    }
}

/// Evidence requirement: the evidence kind plus the deterministic local command
/// when the runtime supplied one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct ExecutionEvidence {
    pub(super) kind: ObjectiveEvidenceKind,
    pub(super) required: bool,
    pub(super) command: Option<String>,
}

/// Constraints every worker shares: which tools and files it may touch.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct ExecutionConstraints {
    pub(super) allowed_tools: Vec<CapabilityAllowedTool>,
    pub(super) allowed_files: Vec<PathBuf>,
    pub(super) read_only: bool,
}

/// Repair policy: bounded strategies plus the change kinds a repair worker may
/// use for the declared target roles.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct ExecutionRepairPolicy {
    pub(super) strategies: Vec<CapabilityRepairStrategy>,
    pub(super) allowed_change_kinds: Vec<String>,
}

/// Runtime projection of a `TaskContract`. The eight headline fields are the
/// lifecycle-common shape every worker reads; `deliverable_kind` backs the
/// lossless [`Self::objective_contract`] compat projection.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct TaskExecutionContract {
    pub(super) objective_kind: ObjectiveKind,
    pub(super) runtime_profile: RuntimeProfile,
    pub(super) deliverables: Vec<ExecutionDeliverable>,
    pub(super) scaffold_profile: ScaffoldProfile,
    pub(super) public_contract: PublicContract,
    pub(super) api_contract_expectations: Vec<ApiContractExpectation>,
    pub(super) evidence: ExecutionEvidence,
    pub(super) authoring_style_decision: AuthoringStyleDecision,
    pub(super) constraints: ExecutionConstraints,
    pub(super) repair_policy: ExecutionRepairPolicy,
    deliverable_kind: ObjectiveDeliverableKind,
}

impl TaskExecutionContract {
    /// Build the runtime contract from the classification-layer `TaskContract`.
    /// Reuses `objective_contract()` and `capability_spec_for_task_contract()`
    /// as the single sources of truth so no parallel taxonomy is introduced.
    pub(super) fn from_task_contract(contract: &TaskContract) -> Self {
        let objective = contract.objective_contract();
        let capability = capability_spec_for_task_contract(contract);
        let deliverables = execution_deliverables_for_contract(contract, &capability);
        let runtime_profile = runtime_profile_for_deliverables(&deliverables);
        let public_contract = public_contract_for_task_contract(contract);
        let allowed_files = deliverables
            .iter()
            .filter_map(|deliverable| deliverable.path.clone())
            .collect();
        let read_only = objective.deliverable_kind == ObjectiveDeliverableKind::Answer;
        let allowed_change_kinds =
            allowed_change_kinds_for_roles(&capability.required_artifacts, objective.task_kind);
        Self {
            objective_kind: objective.objective_kind,
            runtime_profile,
            deliverables,
            scaffold_profile: ScaffoldProfile::None,
            public_contract,
            api_contract_expectations: contract.api_contract_expectations.clone(),
            evidence: ExecutionEvidence {
                kind: objective.evidence_kind,
                required: objective.evidence_required,
                command: None,
            },
            authoring_style_decision: contract.authoring_style_decision,
            constraints: ExecutionConstraints {
                allowed_tools: capability.allowed_tools,
                allowed_files,
                read_only,
            },
            repair_policy: ExecutionRepairPolicy {
                strategies: capability.repair_strategies,
                allowed_change_kinds,
            },
            deliverable_kind: objective.deliverable_kind,
        }
    }

    /// Compat projection back to the read-only [`ObjectiveContract`] view, so the
    /// existing generic lifecycle / telemetry consumers keep working unchanged.
    pub(super) fn objective_contract(&self) -> ObjectiveContract {
        ObjectiveContract {
            authority: super::task_contract::ObjectiveAuthority::CurrentUserRequest,
            auxiliary_context: super::task_contract::ObjectiveAuxiliaryContext::SessionContext,
            task_kind: self.objective_kind.to_task_kind(),
            objective_kind: self.objective_kind,
            deliverable_kind: self.deliverable_kind,
            evidence_kind: self.evidence.kind,
            required_deliverables: required_deliverables_from_execution(&self.deliverables),
            evidence_required: self.evidence.required,
            required_evidence_commands: self.evidence.command.iter().cloned().collect::<Vec<_>>(),
        }
    }

    pub(super) fn with_runtime_profile(mut self, runtime_profile: RuntimeProfile) -> Self {
        self.runtime_profile = runtime_profile;
        self
    }

    pub(super) fn with_scaffold_profile(mut self, scaffold_profile: ScaffoldProfile) -> Self {
        self.scaffold_profile = scaffold_profile;
        self
    }

    pub(super) fn with_evidence_command(mut self, command: impl Into<String>) -> Self {
        let command = command.into();
        let trimmed = command.trim();
        if !trimmed.is_empty() {
            self.evidence.command = Some(trimmed.to_string());
        }
        self
    }

    pub(super) fn with_public_signature(mut self, signature: &str) -> Self {
        self.public_contract.push_signature(signature);
        self
    }

    /// Allowed change kind the repair worker may use for a given target role.
    /// Prefers the entry already declared in this contract's repair policy (the
    /// role is a declared deliverable) and otherwise falls back to that role's
    /// canonical change kind, so a repair worker always has a bounded change kind
    /// for whichever file the failure points at.
    pub(super) fn allowed_change_kind_for_role(&self, role: ArtifactRole) -> String {
        let target_role = DiagnosticRepairTargetRole::from_artifact_role(
            role,
            self.objective_kind.to_task_kind(),
        );
        let canonical = default_allowed_change_kind_for_target_role(target_role);
        self.repair_policy
            .allowed_change_kinds
            .iter()
            .find(|candidate| candidate.as_str() == canonical.as_str())
            .cloned()
            .unwrap_or(canonical)
    }

    /// Whether `role` is a declared deliverable in this contract's repair policy.
    pub(super) fn repair_policy_declares_role(&self, role: ArtifactRole) -> bool {
        let target_role = DiagnosticRepairTargetRole::from_artifact_role(
            role,
            self.objective_kind.to_task_kind(),
        );
        let canonical = default_allowed_change_kind_for_target_role(target_role);
        self.repair_policy
            .allowed_change_kinds
            .iter()
            .any(|candidate| candidate.as_str() == canonical.as_str())
    }

    /// Focused deliverable worker input: leads with the target file, the public
    /// contract, the allowed files, and the evidence command. For a coding
    /// objective the primary worker is [`WorkerKind::Implement`] (the
    /// implementation worker); every other kind rides the same builder.
    pub(super) fn deliverable_worker_request(
        &self,
        contract: &TaskContract,
        target_path: Option<&str>,
        public_contract_excerpt: Option<&str>,
    ) -> DeliverableWorkerRequest {
        let worker_kind = WorkerKind::primary_for_task_kind(self.objective_kind.to_task_kind());
        let target_path = target_path
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(PathBuf::from)
            .or_else(|| {
                self.deliverables
                    .iter()
                    .find_map(|deliverable| deliverable.path.clone())
            });
        let mut public_contract = self.public_contract.clone();
        if let Some(excerpt) = public_contract_excerpt {
            public_contract.push_signature(excerpt);
        }
        let allowed_files = if self.constraints.allowed_files.is_empty() {
            target_path.iter().cloned().collect()
        } else {
            self.constraints.allowed_files.clone()
        };
        let evidence_command = self.evidence.command.clone();

        let mut context_pack =
            WorkerContract::from_task_contract(contract, worker_kind).context_pack;
        if let Some(target) = &target_path {
            context_pack.push(ContextPackEntry::new(
                ContextPackKind::Target,
                "target_path",
                target.to_string_lossy(),
            ));
        }
        if !allowed_files.is_empty() {
            context_pack.push(ContextPackEntry::new(
                ContextPackKind::Target,
                "allowed_files",
                allowed_files
                    .iter()
                    .map(|path| path.to_string_lossy().into_owned())
                    .collect::<Vec<_>>()
                    .join(","),
            ));
        }
        if let Some(summary) = public_contract.summary() {
            context_pack.push(ContextPackEntry::new(
                ContextPackKind::Contract,
                "public_contract",
                summary,
            ));
        }
        if let Some(summary) = api_contract_summary(&self.api_contract_expectations) {
            context_pack.push(ContextPackEntry::new(
                ContextPackKind::Contract,
                "api_contract",
                summary,
            ));
        }
        if let Some(command) = &evidence_command {
            context_pack.push(ContextPackEntry::new(
                ContextPackKind::Evidence,
                "evidence_command",
                command.as_str(),
            ));
        }

        DeliverableWorkerRequest {
            worker_contract: WorkerContract::from_task_contract(contract, worker_kind)
                .with_context_pack(context_pack),
            worker_kind,
            target_path,
            allowed_files,
            public_contract,
            evidence_command,
            output_contract: deliverable_worker_output_contract(self.objective_kind),
        }
    }

    /// Repair worker input that draws the allowed change kind from this
    /// execution contract's repair policy and delegates to the bounded
    /// diagnostic-repair builder (failure observation + target file + allowed
    /// change kind).
    pub(super) fn repair_worker_request(
        &self,
        contract: &TaskContract,
        diagnostic: &str,
        target_hint: &RecoveryTargetHint,
        evidence_command: Option<&str>,
    ) -> DiagnosticRepairWorkerRequest {
        let allowed_change_kind = self.allowed_change_kind_for_role(target_hint.role);
        diagnostic_repair_worker_request_for_evidence_failed(
            contract,
            diagnostic,
            target_hint,
            &allowed_change_kind,
            evidence_command,
        )
    }
}

/// Focused worker input for producing the primary deliverable. The context pack
/// carries the contract, the target file, the allowed files, the public
/// contract, and the evidence command — not a replay of prior turn logs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct DeliverableWorkerRequest {
    pub(super) worker_contract: WorkerContract,
    worker_kind: WorkerKind,
    target_path: Option<PathBuf>,
    allowed_files: Vec<PathBuf>,
    public_contract: PublicContract,
    evidence_command: Option<String>,
    output_contract: &'static str,
}

impl DeliverableWorkerRequest {
    pub(super) fn worker_kind(&self) -> WorkerKind {
        self.worker_kind
    }

    pub(super) fn target_path(&self) -> Option<&Path> {
        self.target_path.as_deref()
    }

    pub(super) fn allowed_files(&self) -> &[PathBuf] {
        &self.allowed_files
    }

    pub(super) fn public_contract(&self) -> &PublicContract {
        &self.public_contract
    }

    pub(super) fn evidence_command(&self) -> Option<&str> {
        self.evidence_command.as_deref()
    }

    pub(super) fn output_contract(&self) -> &'static str {
        self.output_contract
    }

    pub(super) fn policy_message(&self) -> String {
        let worker = self.worker_kind.label();
        let target = self
            .target_path
            .as_ref()
            .map(|path| super::task_contract::mask_and_cap_recovery_field(&path.to_string_lossy()))
            .unwrap_or_else(|| "the declared deliverable artifact".to_string());
        let allowed_files = if self.allowed_files.is_empty() {
            "only the declared deliverable target".to_string()
        } else {
            self.allowed_files
                .iter()
                .map(|path| {
                    super::task_contract::mask_and_cap_recovery_field(&path.to_string_lossy())
                })
                .collect::<Vec<_>>()
                .join(", ")
        };
        let evidence = self
            .evidence_command
            .as_deref()
            .map(super::task_contract::mask_and_cap_recovery_field)
            .unwrap_or_else(|| "the configured local evidence check".to_string());
        let public_contract = self
            .public_contract
            .summary()
            .map(|summary| super::task_contract::mask_and_cap_recovery_field(&summary))
            .unwrap_or_else(|| {
                "infer the public behavior from the active request and project files".to_string()
            });
        format!(
            "[{worker}] Own this turn. Produce `{target}`. Public contract: {public_contract}. Allowed files: {allowed_files}. Evidence: run `{evidence}` after the edit. Lead with the contract and the target file; do not restate prior turn logs or change unrelated files."
        )
    }
}

fn required_deliverables_from_execution(
    deliverables: &[ExecutionDeliverable],
) -> Vec<ArtifactRole> {
    let mut roles = Vec::new();
    for deliverable in deliverables {
        if !roles.contains(&deliverable.role) {
            roles.push(deliverable.role);
        }
    }
    roles
}

fn execution_deliverables_for_contract(
    contract: &TaskContract,
    capability: &CapabilitySpec,
) -> Vec<ExecutionDeliverable> {
    if !contract.required_artifact_identities.is_empty() {
        return contract
            .required_artifact_identities
            .iter()
            .map(|obligation| ExecutionDeliverable {
                role: obligation.role,
                path: Some(PathBuf::from(obligation.path.as_str())),
                kind: Some(obligation.kind),
                format: obligation.format.clone(),
                schema: obligation.schema.clone(),
                required_sections: obligation.required_sections.clone(),
                acceptance_criteria: obligation.acceptance_criteria.clone(),
            })
            .collect();
    }
    capability
        .required_artifacts
        .iter()
        .map(|role| ExecutionDeliverable {
            role: *role,
            path: None,
            kind: None,
            format: None,
            schema: None,
            required_sections: Vec::new(),
            acceptance_criteria: Vec::new(),
        })
        .collect()
}

fn runtime_profile_for_deliverables(deliverables: &[ExecutionDeliverable]) -> RuntimeProfile {
    deliverables
        .iter()
        .filter_map(|deliverable| deliverable.path.as_deref())
        .find_map(RuntimeProfile::from_path)
        .unwrap_or(RuntimeProfile::Unspecified)
}

fn public_contract_for_task_contract(contract: &TaskContract) -> PublicContract {
    let Some(projection) = super::required_behavior::project_behavior_contract(contract) else {
        return PublicContract::default();
    };
    let goal = projection
        .behavior_goal
        .as_ref()
        .map(|goal| goal.label.clone());
    let signatures = projection
        .required_capabilities
        .iter()
        .map(|capability| capability.label.clone())
        .collect();
    PublicContract::from_parts(goal, signatures)
}

fn allowed_change_kinds_for_roles(roles: &[ArtifactRole], task_kind: TaskKind) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for role in roles {
        let target_role = DiagnosticRepairTargetRole::from_artifact_role(*role, task_kind);
        let change_kind = default_allowed_change_kind_for_target_role(target_role);
        if !out.contains(&change_kind) {
            out.push(change_kind);
        }
    }
    out
}

fn deliverable_worker_output_contract(objective_kind: ObjectiveKind) -> &'static str {
    match objective_kind {
        ObjectiveKind::Coding => {
            "implementation_source_created_or_updated_for_the_declared_public_contract"
        }
        ObjectiveKind::Docs => "required_document_sections_created_or_updated",
        ObjectiveKind::Data => "structured_output_file_created_or_updated",
        ObjectiveKind::Research => "research_report_or_notes_created_with_source_slots",
        ObjectiveKind::Ops => "command_observation_or_runbook_created_with_safety_notes",
        ObjectiveKind::Authoring => "prose_artifact_created_or_updated_for_requested_audience",
    }
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
        assert_eq!(
            request.repair_delta_kind(),
            RepairTargetDeltaKind::MissingEvidence
        );
        assert!(
            request
                .ledger_facts()
                .contains("evidence_command_present=true")
        );
        assert!(request.policy_message().contains("TestAuthorWorker"));
        assert!(
            request
                .policy_message()
                .contains("repair_delta=missing_evidence")
        );
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
            request.repair_delta_kind(),
            RepairTargetDeltaKind::EvidenceFailed
        );
        assert!(
            request
                .ledger_facts()
                .contains("diagnostic_failure_kind=compile_error")
        );
        assert_eq!(
            request.output_contract(),
            "single_bounded_repair_tool_action_for_the_declared_target_and_failure"
        );
        assert!(
            request
                .policy_message("exactly one compact Edit on the declared target")
                .contains("DiagnosticRepairWorker")
        );
        assert!(
            request
                .policy_message("exactly one compact Edit on the declared target")
                .contains("Treat repair_delta and ledger_facts as primary control data")
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

        let style_request = diagnostic_repair_worker_request_for_evidence_failed(
            &contract,
            "AssertionError: expected generated test expectation does not match the public contract",
            &target_hint,
            "test",
            Some("node --test tests/main.test.js"),
        );
        assert_eq!(
            style_request.repair_delta_kind(),
            RepairTargetDeltaKind::StyleMismatch
        );
    }

    #[test]
    fn diagnostic_repair_worker_request_carries_api_contract_delta_for_422() {
        let contract = TaskContract::from_request(
            "Create app.py and tests/test_app.py for an HTTP notes API. Implement POST /notes accepting JSON with title and body, returning the created note with id=1.",
        );
        let target_hint = RecoveryTargetHint {
            role: ArtifactRole::Implementation,
            path: "app.py".to_string(),
            reason: "POST /notes returned 422".to_string(),
        };
        let request = diagnostic_repair_worker_request_for_evidence_failed(
            &contract,
            "AssertionError: POST /notes expected 200 but got 422 Unprocessable Entity",
            &target_hint,
            "implementation",
            Some("python -m pytest tests/test_app.py"),
        );

        assert!(
            request
                .api_contract_delta()
                .contains("kind=request_schema_mismatch"),
            "{}",
            request.api_contract_delta()
        );
        assert!(
            request
                .api_contract_delta()
                .contains("request_json_body_fields=title|body"),
            "{}",
            request.api_contract_delta()
        );
        assert!(
            request.api_contract_delta().contains(
                "bind_declared_fields_from_json_request_body_object_not_query_or_form_params"
            ),
            "{}",
            request.api_contract_delta()
        );
        assert!(
            request
                .policy_message("exactly one bounded implementation edit")
                .contains("api_contract_delta=kind=request_schema_mismatch"),
        );
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
        assert_eq!(
            docs_request.repair_delta_kind(),
            RepairTargetDeltaKind::MissingDeliverable
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
        assert_eq!(
            data_request.repair_delta_kind(),
            RepairTargetDeltaKind::EvidenceFailed
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

    // ── Issue #1002: TaskExecutionContract + focused worker inputs ─────────

    #[test]
    fn task_execution_contract_rides_all_six_task_kinds_and_round_trips_objective() {
        let cases = [
            (
                "Create a Rust CLI word counter with Cargo tests and usage docs.",
                ObjectiveKind::Coding,
            ),
            (
                "Write README.md with install, validation, and rollback sections.",
                ObjectiveKind::Docs,
            ),
            (
                "Transform orders.csv into output.csv with id,total columns.",
                ObjectiveKind::Data,
            ),
            (
                "Investigate local LLM repair loops and produce a report in report.md with sources.",
                ObjectiveKind::Research,
            ),
            (
                "Prepare a deployment runbook with rollback and validation commands.",
                ObjectiveKind::Ops,
            ),
            (
                "Translate README.ja.md into English and write README.md.",
                ObjectiveKind::Authoring,
            ),
        ];

        for (request, expected_kind) in cases {
            let contract = TaskContract::from_request(request);
            let execution = TaskExecutionContract::from_task_contract(&contract);

            assert_eq!(execution.objective_kind, expected_kind, "{request}");
            // Every kind rides the same lifecycle: a deliverable, an evidence
            // kind, shared constraints, and a repair policy.
            assert!(!execution.deliverables.is_empty(), "{request}");
            assert!(!execution.constraints.allowed_tools.is_empty(), "{request}");
            assert!(!execution.repair_policy.strategies.is_empty(), "{request}");
            assert!(
                !execution.repair_policy.allowed_change_kinds.is_empty(),
                "{request}"
            );
            // Compat projection is lossless against the existing ObjectiveContract.
            assert_eq!(
                execution.objective_contract(),
                contract.objective_contract(),
                "{request}"
            );
        }
    }

    #[test]
    fn runtime_profile_maps_stack_labels_and_paths() {
        assert_eq!(
            RuntimeProfile::from_stack_label("rust"),
            RuntimeProfile::Rust
        );
        assert_eq!(
            RuntimeProfile::from_stack_label("python"),
            RuntimeProfile::Python
        );
        assert_eq!(
            RuntimeProfile::from_stack_label("javascript"),
            RuntimeProfile::Node
        );
        assert_eq!(
            RuntimeProfile::from_stack_label("typescript"),
            RuntimeProfile::TypeScript
        );
        assert_eq!(
            RuntimeProfile::from_stack_label("rocket-science"),
            RuntimeProfile::Unspecified
        );

        let rust = vec![ExecutionDeliverable {
            role: ArtifactRole::Implementation,
            path: Some(PathBuf::from("src/lib.rs")),
            kind: None,
            format: None,
            schema: None,
            required_sections: Vec::new(),
            acceptance_criteria: Vec::new(),
        }];
        assert_eq!(
            runtime_profile_for_deliverables(&rust),
            RuntimeProfile::Rust
        );

        let docs = vec![ExecutionDeliverable {
            role: ArtifactRole::UsageDocs,
            path: Some(PathBuf::from("README.md")),
            kind: None,
            format: None,
            schema: None,
            required_sections: Vec::new(),
            acceptance_criteria: Vec::new(),
        }];
        assert_eq!(
            runtime_profile_for_deliverables(&docs),
            RuntimeProfile::Unspecified
        );
    }

    #[test]
    fn deliverable_worker_request_is_focused_on_contract_and_target() {
        let contract = TaskContract::from_request(
            "Create a Rust slugify library and verify it with cargo test.",
        );
        let execution = TaskExecutionContract::from_task_contract(&contract)
            .with_runtime_profile(RuntimeProfile::Rust)
            .with_evidence_command("cargo test");
        let request = execution.deliverable_worker_request(
            &contract,
            Some("src/lib.rs"),
            Some("pub fn slugify(input: &str) -> String"),
        );

        // Coding's primary worker IS the implementation worker.
        assert_eq!(request.worker_kind(), WorkerKind::Implement);
        assert_eq!(request.target_path(), Some(Path::new("src/lib.rs")));
        assert_eq!(request.evidence_command(), Some("cargo test"));
        assert!(!request.public_contract().is_empty());
        assert_eq!(
            request.output_contract(),
            "implementation_source_created_or_updated_for_the_declared_public_contract"
        );

        // The context pack is the contract + target file, bounded — not a log replay.
        let pack = &request.worker_contract.context_pack;
        assert!(pack.entries().len() <= MAX_CONTEXT_PACK_ENTRIES);
        assert!(
            pack.entries()
                .iter()
                .any(|entry| entry.label() == "target_path")
        );
        assert!(
            pack.entries()
                .iter()
                .any(|entry| entry.label() == "public_contract")
        );
        assert!(
            pack.entries()
                .iter()
                .any(|entry| entry.label() == "evidence_command")
        );

        // The policy message leads with the target + contract and steers the
        // worker away from re-dumping prior logs.
        let message = request.policy_message();
        assert!(message.contains("[implement]"));
        assert!(message.contains("src/lib.rs"));
        assert!(message.contains("cargo test"));
        assert!(message.contains("do not restate prior turn logs"));
    }

    #[test]
    fn deliverable_worker_request_covers_non_coding_primary_worker() {
        let contract = TaskContract::from_request(
            "Write README.md with install, validation, and rollback sections.",
        );
        let execution = TaskExecutionContract::from_task_contract(&contract);
        let request = execution.deliverable_worker_request(&contract, Some("README.md"), None);

        assert_eq!(request.worker_kind(), WorkerKind::Docs);
        assert_eq!(request.target_path(), Some(Path::new("README.md")));
        assert_eq!(
            request.output_contract(),
            "required_document_sections_created_or_updated"
        );
        assert!(request.policy_message().contains("[docs]"));
    }

    #[test]
    fn repair_worker_request_draws_allowed_change_kind_from_policy() {
        let contract = TaskContract::from_request(
            "Create a Rust slugify library and verify it with cargo test.",
        );
        let execution = TaskExecutionContract::from_task_contract(&contract);

        // The repair worker always resolves a bounded change kind for the file
        // the failure points at, whether or not the role is a declared deliverable.
        assert_eq!(
            execution.allowed_change_kind_for_role(ArtifactRole::Implementation),
            "implementation"
        );
        assert_eq!(
            execution.allowed_change_kind_for_role(ArtifactRole::Test),
            "test"
        );
        // Implementation is a declared deliverable for this coding contract.
        assert!(execution.repair_policy_declares_role(ArtifactRole::Implementation));

        let target_hint = RecoveryTargetHint {
            role: ArtifactRole::Implementation,
            path: "src/lib.rs".to_string(),
            reason: "compiler points at missing public function".to_string(),
        };
        let request = execution.repair_worker_request(
            &contract,
            "error[E0425]: cannot find function `slugify` in this scope",
            &target_hint,
            Some("cargo test"),
        );

        assert_eq!(request.failure_kind(), EvidenceFailureKind::CompileError);
        assert_eq!(
            request.repair_delta_kind(),
            RepairTargetDeltaKind::EvidenceFailed
        );
        assert_eq!(
            request.target_role(),
            DiagnosticRepairTargetRole::Implementation
        );
        assert_eq!(request.allowed_change_kind(), "implementation");
        assert_eq!(request.evidence_command(), "cargo test");
    }

    #[test]
    fn task_execution_contract_adapters_inject_runtime_specifics() {
        let contract =
            TaskContract::from_request("Build a Next.js dashboard and verify it with npm test.");
        let execution = TaskExecutionContract::from_task_contract(&contract)
            .with_runtime_profile(RuntimeProfile::from_stack_label("typescript"))
            .with_scaffold_profile(ScaffoldProfile::Web(ScaffoldFramework::Next))
            .with_evidence_command("  npm test  ")
            .with_public_signature("GET /api/metrics returns JSON");

        assert_eq!(execution.runtime_profile, RuntimeProfile::TypeScript);
        assert_eq!(
            execution.scaffold_profile,
            ScaffoldProfile::Web(ScaffoldFramework::Next)
        );
        assert_eq!(execution.evidence.command.as_deref(), Some("npm test"));
        assert!(
            execution
                .public_contract
                .signatures()
                .iter()
                .any(|signature| signature.contains("GET /api/metrics"))
        );
    }

    #[test]
    fn public_contract_is_bounded_and_deduped() {
        let signatures: Vec<String> = (0..MAX_PUBLIC_CONTRACT_SIGNATURES + 4)
            .map(|idx| format!("operation_{idx}"))
            .collect();
        let mut contract =
            PublicContract::from_parts(Some("ship a slug API".to_string()), signatures);
        // Duplicate + empty pushes are ignored.
        contract.push_signature("operation_0");
        contract.push_signature("   ");

        assert_eq!(contract.signatures().len(), MAX_PUBLIC_CONTRACT_SIGNATURES);
        assert_eq!(contract.goal(), Some("ship a slug API"));
        let summary = contract
            .summary()
            .expect("non-empty contract has a summary");
        assert!(summary.contains("ship a slug API"));
        assert!(summary.contains("satisfies:"));
    }
}
