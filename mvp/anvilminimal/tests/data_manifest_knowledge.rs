use anvilminimal::planner::profiles::data::manifest;

#[test]
fn data_manifest_knowledge_matches_b2b_golden() {
    let manifest = manifest::get();
    let phase = |id: &str| {
        manifest
            .plan
            .phases
            .iter()
            .find(|phase| phase.id == id)
            .unwrap_or_else(|| panic!("missing data manifest phase {id}"))
            .prompt
            .as_str()
    };
    let owned = &manifest.step_templates.ownership.template_owned_artifacts;
    let actual = format!(
        "data-inspection={}\n\
data-cleaning={}\n\
data-aggregation={}\n\
data-reporting={}\n\
data-validation={}\n\
guidance.generic={}\n\
guidance.inspection={}\n\
guidance.schema={}\n\
guidance.repair={}\n\
required_artifacts={}\n\
template_owned.package_phrases={}\n\
template_owned.scaffold_phrases={}\n\
template_owned.path_suffixes={}\n\
template_owned.path_contains={}\n",
        phase("data-inspection"),
        phase("data-cleaning"),
        phase("data-aggregation"),
        phase("data-reporting"),
        phase("data-validation"),
        manifest.guidance.generic.generic_interaction,
        manifest.guidance.canvas_game.canvas_input_wiring_checklist,
        manifest.guidance.contracts.state_requirement,
        manifest.guidance.contracts.contract_attribute_guidance,
        manifest::required_artifacts().join(","),
        owned.package_phrases.join(","),
        owned.scaffold_phrases.join(","),
        owned.artifact_path_suffixes.join(","),
        owned.artifact_path_contains.join(","),
    );

    assert_eq!(
        actual,
        include_str!("golden/data_manifest_v0_knowledge.txt")
    );
}
