use std::collections::BTreeSet;
use std::sync::OnceLock;

use crate::planner::profile_manifest::{ManifestStatus, ManifestV0};
use crate::planner::ultra_plan::{UltraPhase, UltraPlan};

const DATA_MANIFEST_TOML: &str = include_str!("manifest.toml");
const REQUIRED_PHASE_IDS: [&str; 5] = [
    "data-inspection",
    "data-cleaning",
    "data-aggregation",
    "data-reporting",
    "data-validation",
];
const REQUIRED_CHECK_IDS: [&str; 5] = [
    "pipeline_probe",
    "data_results_schema",
    "data_reconciliation",
    "data_claims_binding",
    "data_rerun_consistency",
];

pub fn get() -> &'static ManifestV0 {
    static MANIFEST: OnceLock<ManifestV0> = OnceLock::new();
    MANIFEST.get_or_init(|| {
        let manifest = ManifestV0::from_toml(DATA_MANIFEST_TOML)
            .expect("embedded data profile manifest.toml must parse and resolve");
        validate_data_contract(&manifest)
            .expect("embedded data profile manifest.toml must satisfy the fixed data contract");
        manifest
    })
}

pub fn preset_ultra_plan(goal: &str, style: &str, intent: &str) -> Option<UltraPlan> {
    let manifest = get();
    if !style.eq_ignore_ascii_case(&manifest.plan.style)
        || !intent.eq_ignore_ascii_case(&manifest.plan.intent)
    {
        return None;
    }
    Some(UltraPlan {
        goal: goal.to_string(),
        profile: manifest.plan.profile.clone(),
        style: manifest.plan.style.clone(),
        intent: manifest.plan.intent.clone(),
        phases: manifest
            .plan
            .phases
            .iter()
            .map(|phase| UltraPhase {
                id: phase.id.clone(),
                prompt: phase.prompt.replace("{goal}", goal),
            })
            .collect(),
    })
}

pub fn guidance() -> String {
    let guidance = &get().guidance;
    format!(
        "{}\n{}\n{}\n{}",
        guidance.generic.generic_interaction,
        guidance.generic.start_interaction,
        guidance.persistence.persistence,
        guidance.contracts.contract_attribute_guidance,
    )
}

pub fn generation_rules() -> &'static str {
    get().guidance.generic.start_interaction.as_str()
}

pub fn runtime_contract() -> String {
    let contracts = &get().guidance.contracts;
    format!(
        "- {}\n- {}\n- {}\n- {}",
        contracts.primary_requirement,
        contracts.state_requirement,
        contracts.input_coupled_dimension_requirement,
        get().guidance.persistence.persistence,
    )
}

pub fn required_artifacts() -> Vec<String> {
    vec![
        "pipeline/main.py".to_string(),
        "output/inspection.json".to_string(),
        "output/results.json".to_string(),
        "output/report.md".to_string(),
    ]
}

pub fn dependency_order_hint() -> String {
    format!(
        "Follow manifest phases in order: {}",
        get()
            .plan
            .phases
            .iter()
            .map(|phase| phase.id.as_str())
            .collect::<Vec<_>>()
            .join(" -> ")
    )
}

pub fn required_capability_ids() -> Vec<String> {
    let ids = check_ids();
    let mut required = [
        "data_reconciliation",
        "data_claims_binding",
        "data_rerun_consistency",
    ]
    .into_iter()
    .map(str::to_string)
    .collect::<Vec<_>>();
    if ids.iter().any(|id| id == "data_results_schema") {
        required.push("data_results_schema".to_string());
    }
    required
}

pub fn is_manifest_check_id(id: &str) -> bool {
    required_capability_ids()
        .iter()
        .any(|required| required == id)
}

pub fn evidence_target_paths(evidence_keys: &[String]) -> Vec<String> {
    let mut paths = Vec::new();
    for (evidence, targets) in &get().evidence_targets.mappings {
        if evidence_keys.iter().any(|key| key.contains(evidence)) {
            for target in targets {
                if !paths.contains(target) {
                    paths.push(target.clone());
                }
            }
        }
    }
    paths
}

pub fn source_paths() -> Vec<String> {
    let mut paths = Vec::new();
    for target in get().evidence_targets.mappings.values().flatten() {
        if !paths.contains(target) {
            paths.push(target.clone());
        }
    }
    paths
}

pub fn check_ids() -> Vec<String> {
    get()
        .checks
        .values()
        .flatten()
        .map(|check| check.id.clone())
        .collect()
}

fn validate_data_contract(manifest: &ManifestV0) -> Result<(), String> {
    if manifest.metadata.id != "data" || manifest.plan.profile != "data" {
        return Err("metadata.id and plan.profile must both be data".to_string());
    }
    if manifest.metadata.status != ManifestStatus::Draft {
        return Err("metadata.status must remain draft until B-3".to_string());
    }
    if manifest.plan.placeholders.port.is_some() {
        return Err("data profile must not declare an unused port placeholder".to_string());
    }
    let phase_ids = manifest
        .plan
        .phases
        .iter()
        .map(|phase| phase.id.as_str())
        .collect::<Vec<_>>();
    if phase_ids != REQUIRED_PHASE_IDS {
        return Err(format!(
            "expected five fixed data phases, got {phase_ids:?}"
        ));
    }
    let check_ids = manifest
        .checks
        .values()
        .flatten()
        .map(|check| check.id.as_str())
        .collect::<BTreeSet<_>>();
    let check_count = manifest.checks.values().map(Vec::len).sum::<usize>();
    if check_count != REQUIRED_CHECK_IDS.len() || check_ids != BTreeSet::from(REQUIRED_CHECK_IDS) {
        return Err(format!("data check bindings are incomplete: {check_ids:?}"));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::planner::capability_catalog::{
        DataInternalCheck, InternalCapability, ProbeCapability, ResolvedCapability,
    };

    #[test]
    fn embedded_data_manifest_loads_once_with_five_phases_and_no_port() {
        let manifest = get();
        assert!(std::ptr::eq(manifest, get()));
        assert_eq!(manifest.metadata.status, ManifestStatus::Draft);
        assert_eq!(manifest.plan.phases.len(), 5);
        assert!(manifest.plan.placeholders.port.is_none());
        assert_eq!(
            manifest
                .plan
                .phases
                .iter()
                .map(|phase| phase.id.as_str())
                .collect::<Vec<_>>(),
            REQUIRED_PHASE_IDS
        );
    }

    #[test]
    fn every_data_check_binding_resolves_to_a_typed_catalog_adapter() {
        let resolved = get().resolve().unwrap();
        let capabilities = resolved
            .values()
            .flatten()
            .map(|check| &check.capability)
            .collect::<Vec<_>>();

        assert!(
            capabilities.contains(&&ResolvedCapability::Internal(InternalCapability::Data(
                DataInternalCheck::ResultsSchema
            )))
        );
        assert!(
            capabilities.contains(&&ResolvedCapability::Internal(InternalCapability::Data(
                DataInternalCheck::Reconciliation
            )))
        );
        assert!(
            capabilities.contains(&&ResolvedCapability::Internal(InternalCapability::Data(
                DataInternalCheck::ClaimsBinding
            )))
        );
        assert!(capabilities.iter().any(|capability| matches!(
            capability,
            ResolvedCapability::Probe(ProbeCapability::Pipeline { entry, timeout_seconds })
                if entry == "pipeline/main.py" && *timeout_seconds == 30
        )));
        assert!(capabilities.iter().any(|capability| matches!(
            capability,
            ResolvedCapability::Probe(ProbeCapability::DataRerunConsistency { entry, timeout_seconds })
                if entry == "pipeline/main.py" && *timeout_seconds == 30
        )));
    }

    #[test]
    fn manifest_drives_plan_guidance_requirements_and_repair_targets() {
        let plan = preset_ultra_plan("Summarize sales", "default", "create").unwrap();
        assert_eq!(plan.phases.len(), 5);
        assert!(plan.phases[0].prompt.contains("Summarize sales"));
        assert!(guidance().contains("fixed seed"));
        assert_eq!(
            required_capability_ids(),
            [
                "data_reconciliation",
                "data_claims_binding",
                "data_rerun_consistency",
                "data_results_schema",
            ]
        );
        assert_eq!(
            evidence_target_paths(&["claims_binding_violation".to_string()]),
            ["pipeline/main.py"]
        );
        let acceptance = crate::minimal_loop::evidence::verify_runtime_acceptance(
            &tempfile::tempdir().unwrap().path().to_path_buf(),
            &[],
            &[],
            &required_capability_ids(),
            &[],
            &[],
            &[],
        );
        assert!(acceptance.missing_capabilities.is_empty(), "{acceptance:?}");
    }

    #[test]
    fn data_manifest_rejects_bad_port_and_evidence_target_forms() {
        let bad_port = DATA_MANIFEST_TOML.replacen(
            "goal = \"{goal}\"",
            "goal = \"{goal}\"\nport = \"{goal}\"",
            1,
        );
        assert!(ManifestV0::from_toml(&bad_port).is_err());

        let mixed_targets = DATA_MANIFEST_TOML.replacen(
            "[evidence_targets.mappings]",
            "[evidence_targets]\nsource = \"evidence_knowledge\"\nsection = \"repair_targets\"\n\n[evidence_targets.mappings]",
            1,
        );
        assert!(ManifestV0::from_toml(&mixed_targets).is_err());

        let unsafe_target = DATA_MANIFEST_TOML.replacen(
            "results_schema = [\"pipeline/main.py\"]",
            "results_schema = [\"../pipeline/main.py\"]",
            1,
        );
        assert!(ManifestV0::from_toml(&unsafe_target).is_err());
    }
}
