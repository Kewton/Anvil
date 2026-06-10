//! Contract-bound generation phase planning.
//!
//! This is a small controller-side prompt aid. It derives a compact generation
//! protocol from the sealed execution contract, not from raw request text, so
//! hard tasks can be split into interface/setup/deliverable/evidence phases
//! without adding benchmark-specific rules.

use std::path::{Path, PathBuf};

use super::authoring_style::AuthoringStyleDecision;
use super::contract_generation_expectations::{
    declared_artifacts_summary, declared_expectations_summary,
};
use super::task_contract::ArtifactRole;
use super::worker_contract::{
    ExecutionDeliverable, RuntimeProfile, TaskExecutionContract, WorkerKind,
};
use crate::session::store::ConversationMessage;

const MAX_PHASES_IN_MESSAGE: usize = 9;
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum ContractGenerationPhaseKind {
    ContractAlignment,
    InterfaceOrSchemaExpectation,
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
            Self::InterfaceOrSchemaExpectation => "interface_schema_expectation",
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
    authoring_style_policy: &'static str,
    authoring_style_decision: AuthoringStyleDecision,
    test_binding_policy: &'static str,
    failure_taxonomy: &'static str,
    declared_artifacts: String,
    declared_expectations: String,
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
        if needs_expectation_phase(execution) {
            phases.push(ContractGenerationPhase::new(
                ContractGenerationPhaseKind::InterfaceOrSchemaExpectation,
                WorkerKind::primary_for_task_kind(execution.objective_kind.to_task_kind()),
                None,
                None,
                "declared_interface_schema_and_runtime_expectations_are_explicit_before_writes",
            ));
        }

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
            authoring_style_policy: super::authoring_style::authoring_style_policy_note_for_runtime(
                execution.runtime_profile,
            ),
            authoring_style_decision: execution.authoring_style_decision,
            test_binding_policy: test_binding_policy_for(execution.runtime_profile),
            failure_taxonomy: failure_taxonomy_for(execution),
            declared_artifacts: declared_artifacts_summary(&execution.deliverables),
            declared_expectations: declared_expectations_summary(execution),
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
            "[Contract-Bound Generation] Use small phases derived from the sealed ObjectiveContract, not raw prompt reinterpretation. alignment={}; runtime={}; runtime_constraint={}; authoring_style_decision={}; authoring_style_policy={}; test_binding_policy={}; failure_taxonomy={}; declared_artifacts={}; declared_expectations={}; phases={}. Treat contract_alignment and interface_schema_expectation as internal checklist phases before writing files; do not spend a final answer on them. A deliverable phase is complete only after its target role/path satisfies its predicate. For tests, assert only behavior declared by the ObjectiveContract or user request; do not invent tie-breaks, ordering, error modes, dependencies, or APIs. When a required artifact is small, prefer one coherent whole-file update over fragile fragment insertion, while preserving existing required behavior. Do not final-answer between required deliverable phases; after each write, continue to the next phase or repair only the failed contract delta.",
            self.alignment_predicate,
            self.runtime_profile.label(),
            runtime_constraint_for(self.runtime_profile),
            self.authoring_style_decision.summary(),
            self.authoring_style_policy,
            self.test_binding_policy,
            self.failure_taxonomy,
            self.declared_artifacts,
            self.declared_expectations,
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

fn needs_expectation_phase(execution: &TaskExecutionContract) -> bool {
    execution.runtime_profile != RuntimeProfile::Unspecified
        || !execution.public_contract.is_empty()
        || execution
            .deliverables
            .iter()
            .any(|deliverable| deliverable.schema.is_some())
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

fn test_binding_policy_for(runtime_profile: RuntimeProfile) -> &'static str {
    match runtime_profile {
        RuntimeProfile::Python => {
            "python_cli_subprocess_tests_bind_repo_local_entrypoints_to_repo_root_or_absolute_script_path_temp_dirs_hold_input_data_not_relative_script_cwd"
        }
        RuntimeProfile::Node | RuntimeProfile::TypeScript => {
            "node_cli_subprocess_tests_bind_repo_local_entrypoints_to_package_root_or_absolute_script_path_temp_dirs_hold_input_data_not_relative_script_cwd"
        }
        RuntimeProfile::Rust => {
            "rust_cli_tests_bind_binary_or_manifest_from_crate_root_not_from_unrelated_temp_working_directory"
        }
        RuntimeProfile::Unspecified => {
            "test_entrypoints_and_working_directories_must_match_declared_artifacts"
        }
    }
}

fn failure_taxonomy_for(execution: &TaskExecutionContract) -> &'static str {
    if execution.evidence.required
        && has_roles(
            &execution.deliverables,
            &[ArtifactRole::Implementation, ArtifactRole::Test],
        )
    {
        return "missing_deliverable|missing_evidence|evidence_runner_binding|evidence_failed|authoring_style_mismatch|contract_expectation_drift";
    }
    if execution.evidence.required {
        return "missing_deliverable|missing_evidence|evidence_runner_binding|evidence_failed|contract_expectation_drift";
    }
    "missing_deliverable|contract_expectation_drift"
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
            plan.phases()[1].kind,
            ContractGenerationPhaseKind::InterfaceOrSchemaExpectation
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
                .contains("evidence_runner_is_not_test_authoring_style")
        );
        assert!(
            plan.policy_message()
                .contains("style=unittest_class_style,authority=explicit_user_request"),
            "{}",
            plan.policy_message()
        );
        assert!(
            plan.policy_message()
                .contains("python_cli_subprocess_tests_bind_repo_local_entrypoints_to_repo_root")
        );
        assert!(
            plan.policy_message()
                .contains("test_artifact_targets_the_declared_public_contract")
        );
        assert!(
            plan.policy_message()
                .contains("interface_schema_expectation")
        );
        assert!(
            plan.policy_message()
                .contains("Do not final-answer between required deliverable phases")
        );
        assert!(
            plan.policy_message()
                .contains("do not invent tie-breaks, ordering, error modes")
        );
        assert!(
            plan.policy_message()
                .contains("prefer one coherent whole-file update")
        );
        assert!(
            plan.policy_message()
                .contains("authoring_style_mismatch|contract_expectation_drift")
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
            phase.kind == ContractGenerationPhaseKind::InterfaceOrSchemaExpectation
                && phase.worker_kind == WorkerKind::Data
        }));
        assert!(plan.phases().iter().any(|phase| {
            phase.target_role == Some(ArtifactRole::DataOutput)
                && phase.worker_kind == WorkerKind::Data
        }));
        assert!(!plan.policy_message().contains("cargo"));
        assert!(
            plan.policy_message()
                .contains("missing_deliverable|contract_expectation_drift")
        );
    }

    #[test]
    fn generation_packet_includes_admitted_rust_manifest_and_artifact_formats() {
        let contract = TaskContract::from_request(
            "Create a Rust library. Include Cargo.toml, implementation, tests, and README.md.",
        );
        let execution = TaskExecutionContract::from_task_contract(&contract)
            .with_runtime_profile(RuntimeProfile::Rust)
            .with_evidence_command("cargo test");
        let plan = ContractBoundGenerationPlan::from_execution_contract(&execution)
            .expect("rust contract should produce a generation plan");
        let message = plan.policy_message();

        assert!(message.contains("declared_artifacts="));
        assert!(message.contains("setup@Cargo.toml(kind=file;format=toml)"));
        assert!(message.contains("implementation@src/lib.rs(kind=file;format=rust_source)"));
        assert!(message.contains("test@tests/lib.rs(kind=file;format=rust_source)"));
        assert!(message.contains("evidence=test_run(required),command=cargo test"));
        assert!(message.contains("rust_cli_tests_bind_binary_or_manifest_from_crate_root"));
    }

    #[test]
    fn generation_packet_includes_data_schema_without_coding_branches() {
        let contract = TaskContract::from_request(
            "Generate data/output.csv with exactly columns id,total and exactly rows 1,100 and 2,250.",
        );
        let execution = TaskExecutionContract::from_task_contract(&contract);
        let plan = ContractBoundGenerationPlan::from_execution_contract(&execution)
            .expect("data contract should produce a generation plan");
        let message = plan.policy_message();

        assert!(message.contains("data_output@data/output.csv"));
        assert!(
            message.contains("schema=structured_record:columns=id|total;expected_rows=1|100|2|250"),
            "{message}"
        );
        assert!(!message.contains("module_exports_cli_shape"));
    }

    #[test]
    fn generation_packet_includes_required_document_sections() {
        let contract = TaskContract::from_request(
            "Create README.md with sections Overview, Usage, Validation.",
        );
        let execution = TaskExecutionContract::from_task_contract(&contract);
        let plan = ContractBoundGenerationPlan::from_execution_contract(&execution)
            .expect("docs contract should produce a generation plan");
        let message = plan.policy_message();

        assert!(message.contains("usage_docs@README.md"));
        assert!(
            message.contains("schema=required_sections:overview|usage"),
            "{message}"
        );
    }

    #[test]
    fn expectation_phase_precedes_manifest_implementation_and_tests() {
        let contract = TaskContract::from_request(
            "Create a Rust library. Include Cargo.toml, implementation, tests, and README.md.",
        );
        let execution = TaskExecutionContract::from_task_contract(&contract)
            .with_runtime_profile(RuntimeProfile::Rust)
            .with_evidence_command("cargo test");
        let plan = ContractBoundGenerationPlan::from_execution_contract(&execution)
            .expect("rust contract should produce a generation plan");
        let phases = plan.phases();

        let expectation_idx = phase_index(
            phases,
            ContractGenerationPhaseKind::InterfaceOrSchemaExpectation,
        )
        .expect("expectation phase should be present");
        let setup_idx = role_phase_index(phases, ArtifactRole::Setup).expect("setup phase");
        let implementation_idx =
            role_phase_index(phases, ArtifactRole::Implementation).expect("implementation phase");
        let test_idx = role_phase_index(phases, ArtifactRole::Test).expect("test phase");

        assert!(expectation_idx < setup_idx);
        assert!(setup_idx < implementation_idx);
        assert!(implementation_idx < test_idx);
        assert!(plan.policy_message().contains(
            "declared_interface_schema_and_runtime_expectations_are_explicit_before_writes"
        ));
    }

    #[test]
    fn answer_only_contract_does_not_add_generation_protocol() {
        let contract = TaskContract::from_request("Explain what this repository does.");
        let execution = TaskExecutionContract::from_task_contract(&contract);

        assert!(ContractBoundGenerationPlan::from_execution_contract(&execution).is_none());
    }

    fn phase_index(
        phases: &[ContractGenerationPhase],
        kind: ContractGenerationPhaseKind,
    ) -> Option<usize> {
        phases.iter().position(|phase| phase.kind == kind)
    }

    fn role_phase_index(phases: &[ContractGenerationPhase], role: ArtifactRole) -> Option<usize> {
        phases
            .iter()
            .position(|phase| phase.target_role == Some(role))
    }
}
