//! Artifact contract construction for TaskContract.
//!
//! This module owns the small orchestration step that turns interpreted request
//! inputs into required/optional artifact roles plus typed artifact obligations.
//! The lower-level request scanners stay in `task_contract` until those
//! inference helpers are split behind a semantic-candidate boundary.

use super::project_profile_projection::{ProfileForbiddenRoles, ProjectProfileContractInputs};
use super::task_contract::{
    ArtifactObligation, ArtifactRole, OutputContextScan, ProjectIntent, TaskIntent, TaskKind,
    data_path_has_output_context_with_scan, explicit_artifact_obligations_from_request_with_scan,
    inferred_data_obligations_from_request_with_scan, inferred_docs_obligations_from_request,
    inferred_ops_obligations_from_request, request_asks_for_code_work,
    required_research_sections_from_request, research_report_artifact_intended_with_scan,
    research_report_path_from_request_with_scan,
};
use super::task_contract_controller_packet::ControllerStatePacket;
use super::task_contract_input_projection::ContractRequestInputs;
use super::task_contract_obligation_planning::{
    inferred_artifact_obligations_from_project_intent,
    inferred_obligation_shadowed_by_explicit_identity,
    profile_obligation_shadowed_by_prior_identity, push_or_merge_artifact_obligation,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct ArtifactContractParts {
    pub(super) required: Vec<ArtifactRole>,
    pub(super) optional: Vec<ArtifactRole>,
    pub(super) required_artifact_identities: Vec<ArtifactObligation>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ArtifactRoleSeed {
    required: Vec<ArtifactRole>,
    optional: Vec<ArtifactRole>,
    research_report_intended: bool,
}

pub(super) struct ArtifactContractBuildInputs<'a> {
    pub(super) scan: &'a OutputContextScan,
    pub(super) request: &'a str,
    pub(super) lower: &'a str,
    pub(super) task_kind: TaskKind,
    pub(super) intent: TaskIntent,
    pub(super) request_inputs: ContractRequestInputs,
    pub(super) project_intent: &'a ProjectIntent,
    pub(super) profile_inputs: Option<&'a ProjectProfileContractInputs>,
    pub(super) controller_state: Option<&'a ControllerStatePacket>,
}

pub(super) fn build_artifact_contract_parts(
    inputs: ArtifactContractBuildInputs<'_>,
) -> ArtifactContractParts {
    let mut role_seed = seed_artifact_roles(&inputs);
    let mut required_artifact_identities = build_required_artifact_identities(
        &inputs,
        &mut role_seed.required,
        role_seed.research_report_intended,
    );
    role_seed.required.sort();
    role_seed.required.dedup();
    required_artifact_identities
        .sort_by(|a, b| (a.role, a.path.as_str()).cmp(&(b.role, b.path.as_str())));

    ArtifactContractParts {
        required: role_seed.required,
        optional: role_seed.optional,
        required_artifact_identities,
    }
}

fn seed_artifact_roles(inputs: &ArtifactContractBuildInputs<'_>) -> ArtifactRoleSeed {
    let mut required = Vec::new();
    let mut optional = Vec::new();
    let profile_forbids = ProfileForbiddenRoles::from_inputs(inputs.profile_inputs);

    if inputs.task_kind == TaskKind::Coding
        && (inputs.request_inputs.asks_for_implementation
            || inputs.request_inputs.project_intent_implies_implementation)
        && !profile_forbids.implementation
    {
        required.push(ArtifactRole::Implementation);
    }
    if inputs.request_inputs.asks_for_tests && !profile_forbids.tests {
        required.push(ArtifactRole::Test);
    }
    if inputs.request_inputs.asks_for_usage_docs && !profile_forbids.usage_docs {
        required.push(ArtifactRole::UsageDocs);
    }
    if inputs.task_kind == TaskKind::Authoring && !profile_forbids.usage_docs {
        required.push(ArtifactRole::UsageDocs);
    }
    let setup_required = matches!(inputs.intent, TaskIntent::Install)
        && !request_asks_for_code_work(inputs.request, inputs.lower);
    if inputs.request_inputs.asks_for_setup && !profile_forbids.setup {
        if setup_required {
            required.push(ArtifactRole::Setup);
        } else {
            optional.push(ArtifactRole::Setup);
        }
    }
    if inputs.request_inputs.asks_for_data_output {
        required.push(ArtifactRole::DataOutput);
    }
    if let Some(role) = inputs
        .profile_inputs
        .and_then(|inputs| inputs.required_role)
    {
        required.push(role);
    }

    let research_report_intended = inputs.task_kind == TaskKind::Research
        && research_report_artifact_intended_with_scan(inputs.scan, inputs.request);
    if research_report_intended {
        required.push(ArtifactRole::UsageDocs);
    }

    optional.sort();
    optional.dedup();

    ArtifactRoleSeed {
        required,
        optional,
        research_report_intended,
    }
}

fn build_required_artifact_identities(
    inputs: &ArtifactContractBuildInputs<'_>,
    required: &mut Vec<ArtifactRole>,
    research_report_intended: bool,
) -> Vec<ArtifactObligation> {
    let mut required_artifact_identities =
        explicit_artifact_obligations_from_request_with_scan(inputs.scan, inputs.request);
    if let Some(controller_state) = inputs.controller_state {
        controller_state.extend_contract_parts(required, &mut required_artifact_identities);
    }
    for identity in inputs
        .profile_inputs
        .into_iter()
        .flat_map(|inputs| inputs.artifact_obligations.iter())
        .cloned()
    {
        if identity.role == ArtifactRole::DataOutput
            && !data_path_has_output_context_with_scan(inputs.scan, &identity.path)
        {
            continue;
        }
        if profile_obligation_shadowed_by_prior_identity(&required_artifact_identities, &identity) {
            continue;
        }
        if !required.contains(&identity.role) {
            required.push(identity.role);
        }
        push_or_merge_artifact_obligation(&mut required_artifact_identities, identity);
    }
    if inputs.request_inputs.asks_for_data_output
        && required_artifact_identities
            .iter()
            .any(|identity| identity.role == ArtifactRole::DataOutput)
    {
        required.push(ArtifactRole::DataOutput);
    }
    required_artifact_identities.retain(|identity| required.contains(&identity.role));
    for identity in
        inferred_artifact_obligations_from_project_intent(inputs.project_intent, required)
    {
        if inferred_obligation_shadowed_by_explicit_identity(
            &required_artifact_identities,
            &identity,
        ) {
            continue;
        }
        if !required.contains(&identity.role) {
            required.push(identity.role);
        }
        push_or_merge_artifact_obligation(&mut required_artifact_identities, identity);
    }
    for identity in inferred_docs_obligations_from_request(inputs.request, inputs.lower, required) {
        push_or_merge_artifact_obligation(&mut required_artifact_identities, identity);
    }
    if research_report_intended {
        let sections = required_research_sections_from_request(inputs.request);
        let path = research_report_path_from_request_with_scan(inputs.scan, inputs.request);
        push_or_merge_artifact_obligation(
            &mut required_artifact_identities,
            ArtifactObligation::research_report(path, sections),
        );
    }
    for identity in inferred_data_obligations_from_request_with_scan(inputs.scan, inputs.request) {
        if !required.contains(&identity.role) {
            required.push(identity.role);
        }
        push_or_merge_artifact_obligation(&mut required_artifact_identities, identity);
    }
    for identity in
        inferred_ops_obligations_from_request(inputs.request, inputs.lower, inputs.task_kind)
    {
        if !required.contains(&identity.role) {
            required.push(identity.role);
        }
        push_or_merge_artifact_obligation(&mut required_artifact_identities, identity);
    }
    required_artifact_identities
}
