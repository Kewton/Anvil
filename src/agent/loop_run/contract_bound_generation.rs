//! Contract-bound generation phase planning.
//!
//! This is a small controller-side prompt aid. It derives a compact generation
//! protocol from the sealed execution contract, not from raw request text, so
//! hard tasks can be split into interface/setup/deliverable/evidence phases
//! without adding benchmark-specific rules.

use std::path::{Path, PathBuf};

use super::task_contract::ArtifactRole;
use super::worker_contract::{
    ExecutionDeliverable, RuntimeProfile, TaskExecutionContract, WorkerKind,
};
use crate::session::store::ConversationMessage;

const MAX_PHASES_IN_MESSAGE: usize = 7;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum ContractGenerationPhaseKind {
    ContractAlignment,
    Setup,
    PrimaryDeliverable,
    SupportingDeliverable,
    Evidence,
    Reconcile,
    RepairDelta,
}

impl ContractGenerationPhaseKind {
    fn label(self) -> &'static str {
        match self {
            Self::ContractAlignment => "contract_alignment",
            Self::Setup => "setup",
            Self::PrimaryDeliverable => "primary_deliverable",
            Self::SupportingDeliverable => "supporting_deliverable",
            Self::Evidence => "evidence",
            Self::Reconcile => "reconcile",
            Self::RepairDelta => "repair_delta",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct ContractGenerationPhase {
    pub(super) kind: ContractGenerationPhaseKind,
    pub(super) worker_kind: WorkerKind,
    pub(super) target_role: Option<ArtifactRole>,
    pub(super) target_path: Option<PathBuf>,
    pub(super) completion_predicate: &'static str,
}

impl ContractGenerationPhase {
    fn new(
        kind: ContractGenerationPhaseKind,
        worker_kind: WorkerKind,
        target_role: Option<ArtifactRole>,
        target_path: Option<PathBuf>,
        completion_predicate: &'static str,
    ) -> Self {
        Self {
            kind,
            worker_kind,
            target_role,
            target_path,
            completion_predicate,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct ContractBoundGenerationPlan {
    phases: Vec<ContractGenerationPhase>,
    alignment_predicate: &'static str,
    runtime_profile: RuntimeProfile,
}

impl ContractBoundGenerationPlan {
    pub(super) fn from_execution_contract(execution: &TaskExecutionContract) -> Option<Self> {
        if execution.constraints.read_only || execution.deliverables.is_empty() {
            return None;
        }

        let alignment_predicate = alignment_predicate_for(execution);
        let mut phases = vec![ContractGenerationPhase::new(
            ContractGenerationPhaseKind::ContractAlignment,
            WorkerKind::primary_for_task_kind(execution.objective_kind.to_task_kind()),
            None,
            None,
            alignment_predicate,
        )];

        for deliverable in generation_ordered_deliverables(&execution.deliverables) {
            phases.push(phase_for_deliverable(execution, deliverable));
        }

        if execution.evidence.required {
            phases.push(ContractGenerationPhase::new(
                ContractGenerationPhaseKind::Evidence,
                WorkerKind::Evidence,
                None,
                None,
                "evidence_runner_observation_bound_to_declared_artifacts",
            ));
        }

        phases.push(ContractGenerationPhase::new(
            ContractGenerationPhaseKind::Reconcile,
            WorkerKind::Evidence,
            None,
            None,
            "latest_artifact_and_evidence_facts_match_objective_contract",
        ));
        phases.push(ContractGenerationPhase::new(
            ContractGenerationPhaseKind::RepairDelta,
            WorkerKind::DiagnosticRepair,
            None,
            None,
            "only_the_failed_contract_delta_is_repaired",
        ));

        Some(Self {
            phases,
            alignment_predicate,
            runtime_profile: execution.runtime_profile,
        })
    }

    #[cfg(test)]
    pub(super) fn phases(&self) -> &[ContractGenerationPhase] {
        &self.phases
    }

    pub(super) fn policy_message(&self) -> String {
        let phase_text = self
            .phases
            .iter()
            .take(MAX_PHASES_IN_MESSAGE)
            .enumerate()
            .map(|(idx, phase)| {
                let target = phase_target_label(phase);
                format!(
                    "{}:{}:{}:{}:{}",
                    idx + 1,
                    phase.kind.label(),
                    phase.worker_kind.label(),
                    target,
                    phase.completion_predicate
                )
            })
            .collect::<Vec<_>>()
            .join(" -> ");
        format!(
            "[Contract-Bound Generation] Use small phases derived from the sealed ObjectiveContract, not raw prompt reinterpretation. alignment={}; runtime={}; runtime_constraint={}; phases={}. A deliverable phase is complete only after its target role/path satisfies its predicate. Do not final-answer between required deliverable phases; after each write, continue to the next phase or repair only the failed contract delta.",
            self.alignment_predicate,
            self.runtime_profile.label(),
            runtime_constraint_for(self.runtime_profile),
            phase_text
        )
    }
}

pub(super) fn contract_bound_generation_message_for_execution(
    execution: &TaskExecutionContract,
) -> Option<ConversationMessage> {
    ContractBoundGenerationPlan::from_execution_contract(execution)
        .map(|plan| ConversationMessage::system(plan.policy_message()))
}

fn generation_ordered_deliverables(
    deliverables: &[ExecutionDeliverable],
) -> Vec<&ExecutionDeliverable> {
    let mut ordered = Vec::with_capacity(deliverables.len());
    append_deliverables_with_role(deliverables, ArtifactRole::Setup, &mut ordered);
    append_primary_deliverables(deliverables, &mut ordered);
    append_deliverables_with_role(deliverables, ArtifactRole::Test, &mut ordered);
    ordered
}

fn append_deliverables_with_role<'a>(
    deliverables: &'a [ExecutionDeliverable],
    role: ArtifactRole,
    ordered: &mut Vec<&'a ExecutionDeliverable>,
) {
    for deliverable in deliverables
        .iter()
        .filter(|deliverable| deliverable.role == role)
    {
        if !ordered
            .iter()
            .any(|existing| std::ptr::eq(*existing, deliverable))
        {
            ordered.push(deliverable);
        }
    }
}

fn append_primary_deliverables<'a>(
    deliverables: &'a [ExecutionDeliverable],
    ordered: &mut Vec<&'a ExecutionDeliverable>,
) {
    for deliverable in deliverables
        .iter()
        .filter(|deliverable| !matches!(deliverable.role, ArtifactRole::Setup | ArtifactRole::Test))
    {
        if !ordered
            .iter()
            .any(|existing| std::ptr::eq(*existing, deliverable))
        {
            ordered.push(deliverable);
        }
    }
}

fn phase_for_deliverable(
    execution: &TaskExecutionContract,
    deliverable: &ExecutionDeliverable,
) -> ContractGenerationPhase {
    let kind = match deliverable.role {
        ArtifactRole::Setup => ContractGenerationPhaseKind::Setup,
        ArtifactRole::Test => ContractGenerationPhaseKind::SupportingDeliverable,
        ArtifactRole::Implementation | ArtifactRole::UsageDocs | ArtifactRole::DataOutput => {
            ContractGenerationPhaseKind::PrimaryDeliverable
        }
    };
    ContractGenerationPhase::new(
        kind,
        worker_kind_for_role(execution, deliverable.role),
        Some(deliverable.role),
        deliverable.path.clone(),
        completion_predicate_for_role(deliverable.role),
    )
}

fn worker_kind_for_role(execution: &TaskExecutionContract, role: ArtifactRole) -> WorkerKind {
    match role {
        ArtifactRole::Implementation | ArtifactRole::Setup => WorkerKind::Implement,
        ArtifactRole::Test => WorkerKind::TestAuthor,
        ArtifactRole::UsageDocs => {
            WorkerKind::primary_for_task_kind(execution.objective_kind.to_task_kind())
        }
        ArtifactRole::DataOutput => WorkerKind::Data,
    }
}

fn completion_predicate_for_role(role: ArtifactRole) -> &'static str {
    match role {
        ArtifactRole::Implementation => "source_artifact_satisfies_public_contract",
        ArtifactRole::Test => "test_artifact_targets_the_declared_public_contract",
        ArtifactRole::UsageDocs => "document_artifact_contains_required_content",
        ArtifactRole::Setup => "setup_artifact_supports_the_evidence_runner",
        ArtifactRole::DataOutput => "data_artifact_satisfies_declared_schema",
    }
}

fn alignment_predicate_for(execution: &TaskExecutionContract) -> &'static str {
    if has_roles(
        &execution.deliverables,
        &[ArtifactRole::Implementation, ArtifactRole::Test],
    ) {
        return "source_api_and_test_expectations_agree";
    }
    if has_roles(&execution.deliverables, &[ArtifactRole::DataOutput]) {
        return "output_path_schema_and_rows_agree";
    }
    if execution.evidence.required {
        return "deliverables_and_evidence_expectation_agree";
    }
    "declared_deliverables_match_objective"
}

fn runtime_constraint_for(runtime_profile: RuntimeProfile) -> &'static str {
    match runtime_profile {
        RuntimeProfile::Python => {
            "do_not_assume_python_version_specific_stdlib_modules_without_verifying_runtime"
        }
        RuntimeProfile::Rust => "manifest_crate_targets_source_tests_and_cargo_evidence_must_agree",
        RuntimeProfile::Node | RuntimeProfile::TypeScript => {
            "module_exports_cli_shape_tests_and_package_scripts_must_agree"
        }
        RuntimeProfile::Unspecified => "use_only_capabilities_declared_by_the_contract_and_files",
    }
}

fn has_roles(deliverables: &[ExecutionDeliverable], roles: &[ArtifactRole]) -> bool {
    roles.iter().all(|role| {
        deliverables
            .iter()
            .any(|deliverable| deliverable.role == *role)
    })
}

fn phase_target_label(phase: &ContractGenerationPhase) -> String {
    let role = phase
        .target_role
        .map(|role| role.label())
        .unwrap_or("contract");
    let path = phase
        .target_path
        .as_deref()
        .map(display_path)
        .unwrap_or_else(|| "declared".to_string());
    format!("{role}@{path}")
}

fn display_path(path: &Path) -> String {
    super::task_contract::mask_and_cap_recovery_field(&path.to_string_lossy())
}

#[cfg(test)]
mod tests {
    use super::super::task_contract::TaskContract;
    use super::*;

    #[test]
    fn coding_contract_plans_alignment_before_source_and_test_generation() {
        let contract = TaskContract::from_request(
            "Create merge_toml.py and tests/test_merge_toml.py. Implement merge_toml_files(left, right) and verify with python -m unittest.",
        );
        let execution = TaskExecutionContract::from_task_contract(&contract)
            .with_runtime_profile(RuntimeProfile::Python)
            .with_evidence_command("python -m unittest discover -s tests");
        let plan = ContractBoundGenerationPlan::from_execution_contract(&execution)
            .expect("coding contract should produce a generation plan");

        assert_eq!(
            plan.phases()[0].kind,
            ContractGenerationPhaseKind::ContractAlignment
        );
        assert_eq!(
            plan.phases()[0].completion_predicate,
            "source_api_and_test_expectations_agree"
        );
        assert!(plan.phases().iter().any(|phase| {
            phase.target_role == Some(ArtifactRole::Implementation)
                && phase.worker_kind == WorkerKind::Implement
        }));
        assert!(plan.phases().iter().any(|phase| {
            phase.target_role == Some(ArtifactRole::Test)
                && phase.worker_kind == WorkerKind::TestAuthor
        }));
        assert!(plan.policy_message().contains("runtime=python"));
        assert!(
            plan.policy_message()
                .contains("do_not_assume_python_version_specific_stdlib_modules")
        );
        assert!(
            plan.policy_message()
                .contains("source_api_and_test_expectations_agree")
        );
        assert!(
            plan.policy_message()
                .contains("test_artifact_targets_the_declared_public_contract")
        );
        assert!(
            plan.policy_message()
                .contains("Do not final-answer between required deliverable phases")
        );
    }

    #[test]
    fn data_contract_uses_same_phase_shape_without_coding_workers() {
        let contract = TaskContract::from_request(
            "Create data/summary.json with fields topic, status, count.",
        );
        let execution = TaskExecutionContract::from_task_contract(&contract);
        let plan = ContractBoundGenerationPlan::from_execution_contract(&execution)
            .expect("data contract should produce a generation plan");

        assert_eq!(
            plan.phases()[0].completion_predicate,
            "output_path_schema_and_rows_agree"
        );
        assert!(plan.phases().iter().any(|phase| {
            phase.target_role == Some(ArtifactRole::DataOutput)
                && phase.worker_kind == WorkerKind::Data
        }));
        assert!(!plan.policy_message().contains("cargo"));
    }

    #[test]
    fn answer_only_contract_does_not_add_generation_protocol() {
        let contract = TaskContract::from_request("Explain what this repository does.");
        let execution = TaskExecutionContract::from_task_contract(&contract);

        assert!(ContractBoundGenerationPlan::from_execution_contract(&execution).is_none());
    }
}
