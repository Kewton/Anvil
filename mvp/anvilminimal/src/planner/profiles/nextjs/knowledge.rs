use std::sync::OnceLock;

use serde::Deserialize;

const KNOWLEDGE_TOML: &str = include_str!("knowledge.toml");

#[derive(Debug, Deserialize)]
pub(crate) struct NextJsKnowledge {
    pub(crate) preset: PresetKnowledge,
    pub(crate) deterministic_keywords: DeterministicKeywords,
    pub(crate) setup_classifier: SetupClassifierKnowledge,
    pub(crate) template_owned_artifacts: TemplateOwnedArtifactKnowledge,
    pub(crate) repair_guidance: RepairGuidanceKnowledge,
    pub(crate) contracts: ContractKnowledge,
    pub(crate) canonical: CanonicalKnowledge,
}

#[derive(Debug, Deserialize)]
pub(crate) struct PresetKnowledge {
    pub(crate) profile: String,
    pub(crate) style: String,
    pub(crate) intent: String,
    pub(crate) phases: Vec<PresetPhaseKnowledge>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct PresetPhaseKnowledge {
    pub(crate) id: String,
    pub(crate) prompt: String,
}

#[derive(Debug, Deserialize)]
pub(crate) struct DeterministicKeywords {
    pub(crate) scaffold_phase: Vec<String>,
    pub(crate) scaffold_phase_id: Vec<String>,
    pub(crate) port_phase_markers: Vec<String>,
    pub(crate) port_script_phase: Vec<String>,
    pub(crate) build_verify_phase: Vec<String>,
    pub(crate) implementation_phase: Vec<String>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct SetupClassifierKnowledge {
    pub(crate) package_phrases: Vec<String>,
    pub(crate) package_tokens: Vec<String>,
    pub(crate) scaffold_phrases: Vec<String>,
    pub(crate) scaffold_tokens: Vec<String>,
    pub(crate) scaffold_setup_markers: Vec<String>,
    pub(crate) scaffold_project_marker: String,
    pub(crate) scaffold_dependency_exclusion: String,
}

#[derive(Debug, Deserialize)]
pub(crate) struct TemplateOwnedArtifactKnowledge {
    pub(crate) package_phrases: Vec<String>,
    pub(crate) package_tokens: Vec<String>,
    pub(crate) scaffold_phrases: Vec<String>,
    pub(crate) scaffold_tokens: Vec<String>,
    pub(crate) package_manifest_names: Vec<String>,
    pub(crate) artifact_path_suffixes: Vec<String>,
    pub(crate) artifact_path_contains: Vec<String>,
    pub(crate) package_check_marker: String,
    pub(crate) scaffold_check_marker: String,
}

#[derive(Debug, Deserialize)]
pub(crate) struct RepairGuidanceKnowledge {
    pub(crate) generic_interaction: String,
    pub(crate) start_interaction: String,
    pub(crate) canvas_game_interaction: String,
    pub(crate) canvas_render_loop_checklist: String,
    pub(crate) canvas_input_wiring_checklist: String,
    pub(crate) persistence: String,
}

#[derive(Debug, Deserialize)]
pub(crate) struct ContractKnowledge {
    pub(crate) state_binding_contract: String,
    pub(crate) input_coupled_dimension_requirement: String,
    pub(crate) contract_attribute_missing_kind: String,
    pub(crate) contract_attribute_guidance: String,
    pub(crate) state_requirement: String,
    pub(crate) restart_requirement: String,
    pub(crate) input_requirement: String,
    pub(crate) primary_requirement: String,
    pub(crate) state_example: String,
    pub(crate) restart_example: String,
    pub(crate) input_example: String,
    pub(crate) primary_example: String,
}

#[derive(Debug, Deserialize)]
pub(crate) struct CanonicalKnowledge {
    pub(crate) package_script_build: String,
    pub(crate) package_script_dev: String,
    pub(crate) package_script_start: String,
    pub(crate) required_hooks: Vec<String>,
    pub(crate) scaffold_files: Vec<String>,
    pub(crate) tailwind_config_rels: Vec<String>,
    pub(crate) tailwind_config: String,
    pub(crate) tailwind_config_cjs: String,
    pub(crate) package_json: String,
    pub(crate) tsconfig: String,
    pub(crate) postcss_config: String,
    pub(crate) tailwind_css: String,
    pub(crate) global_d_ts: String,
    pub(crate) layout_tsx: String,
}

pub(crate) fn get() -> &'static NextJsKnowledge {
    static KNOWLEDGE: OnceLock<NextJsKnowledge> = OnceLock::new();
    KNOWLEDGE.get_or_init(|| {
        toml::from_str(KNOWLEDGE_TOML).expect("embedded nextjs knowledge.toml must parse")
    })
}

pub(crate) fn generic_interaction_repair_guidance(failure_kind: &str) -> Vec<String> {
    interaction_failure_kind(failure_kind)
        .then(|| vec![get().repair_guidance.generic_interaction.clone()])
        .unwrap_or_default()
}

pub(crate) fn interaction_repair_guidance(
    failure_kind: &str,
    required_capabilities: &[String],
    required_evidence: &[String],
) -> Vec<String> {
    let generic = generic_interaction_repair_guidance(failure_kind);
    if generic.is_empty() {
        return generic;
    }
    let mut guidance = Vec::new();
    let knowledge = &get().repair_guidance;
    if failure_kind.contains("start_transition_missing") {
        push_unique(&mut guidance, &knowledge.start_interaction);
    }
    if game_interaction_contract(required_capabilities, required_evidence) {
        push_unique(&mut guidance, &knowledge.canvas_game_interaction);
        if render_loop_failure(failure_kind) {
            push_unique(&mut guidance, &knowledge.canvas_render_loop_checklist);
            push_unique(&mut guidance, &knowledge.canvas_input_wiring_checklist);
        }
    }
    let persistence_required = required_evidence
        .iter()
        .any(|evidence| evidence == "persistence_evidence");
    if persistence_required && failure_kind.contains("persistence_after_reload_reset") {
        push_unique(&mut guidance, &knowledge.persistence);
    }
    for line in generic {
        push_unique(&mut guidance, &line);
    }
    if persistence_required {
        push_unique(&mut guidance, &knowledge.persistence);
    }
    guidance
}

fn interaction_failure_kind(failure_kind: &str) -> bool {
    let lower = failure_kind.to_ascii_lowercase();
    [
        "browser_interaction_failed",
        "interaction_",
        "input_state_",
        "canvas_",
        "start_transition",
        "persistence_",
        "text_",
        "token_",
        "surface_",
    ]
    .iter()
    .any(|token| lower.contains(token))
}

fn game_interaction_contract(
    required_capabilities: &[String],
    required_evidence: &[String],
) -> bool {
    required_capabilities.iter().any(|capability| {
        let lower = capability.to_ascii_lowercase();
        lower.contains("adversary")
            || lower.contains("challenge")
            || lower.contains("failure")
            || lower.contains("collision")
    }) || required_evidence.iter().any(|evidence| {
        matches!(
            evidence.as_str(),
            "challenge_or_adversary_evidence" | "failure_or_collision_evidence"
        )
    })
}

fn render_loop_failure(failure_kind: &str) -> bool {
    let lower = failure_kind.to_ascii_lowercase();
    lower.contains("input_state_change_missing_after_start") || lower.contains("canvas_blank")
}

fn push_unique(out: &mut Vec<String>, value: &str) {
    if !out.iter().any(|existing| existing == value) {
        out.push(value.to_string());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn embedded_preset_has_byte_stable_phase_body() {
        let preset = &get().preset;
        assert_eq!(preset.profile, "nextjs");
        assert_eq!(preset.style, "default");
        assert_eq!(preset.intent, "create");
        assert_eq!(
            preset
                .phases
                .iter()
                .map(|phase| phase.id.as_str())
                .collect::<Vec<_>>(),
            vec![
                "project-setup",
                "core-implementation",
                "contract-wiring",
                "build-verification",
            ]
        );
        assert_eq!(
            preset.phases[0].prompt,
            "Scaffold and setup the Next.js App Router project shell. Create or complete the package manifest, TypeScript config, styling config, and route-bound scaffold so the deterministic nextjs-scaffold template owns setup artifacts."
        );
        assert_eq!(
            preset.phases[1].prompt,
            "Implement the core task-specific behavior for: {goal}. Keep one route-bound implementation, extend the instrumented skeleton instead of replacing it, and keep the implementation in the Next.js route-bound source."
        );
        assert!(
            preset.phases[2]
                .prompt
                .contains("at least one dimension that immediately responds to input")
        );
        assert_eq!(
            preset.phases[3].prompt,
            "Run build verification for the deterministic Next.js scaffold. Verify package scripts, dependency boundary, and npm run build / next build only; keep this final phase verification-only."
        );
    }

    #[test]
    fn embedded_matcher_knowledge_keeps_required_tokens() {
        let knowledge = get();
        for token in ["scaffold", "setup", "セットアップ"] {
            assert!(
                knowledge
                    .deterministic_keywords
                    .scaffold_phase
                    .iter()
                    .any(|candidate| candidate == token)
            );
        }
        for token in ["game logic", "collision", "スコア", "敵"] {
            assert!(
                knowledge
                    .deterministic_keywords
                    .implementation_phase
                    .iter()
                    .any(|candidate| candidate == token)
            );
        }
        assert!(
            knowledge
                .template_owned_artifacts
                .package_phrases
                .contains(&"port script".to_string())
        );
        assert!(
            knowledge
                .template_owned_artifacts
                .artifact_path_contains
                .contains(&"postcss.config.".to_string())
        );
    }

    #[test]
    fn embedded_contract_knowledge_keeps_required_body() {
        let contracts = &get().contracts;
        assert_eq!(
            contracts.contract_attribute_missing_kind,
            "contract_attribute_missing"
        );
        assert!(
            contracts
                .state_binding_contract
                .contains("after start and after input")
        );
        assert!(
            contracts
                .state_binding_contract
                .contains("at least one dimension that immediately responds to input")
        );
        assert!(
            contracts
                .input_coupled_dimension_requirement
                .contains("入力連動次元（例: プレイヤー/パドルのx座標）")
        );
        for placeholder in [
            "{classification}",
            "{attribute}",
            "{path}",
            "{requirement}",
            "{excerpts}",
            "{example}",
        ] {
            assert!(contracts.contract_attribute_guidance.contains(placeholder));
        }
        assert_eq!(
            contracts.state_example,
            "data-anvil-state={JSON.stringify({ phase, score, playerX })}"
        );
        assert_eq!(contracts.restart_example, "data-anvil-action=\"restart\"");
        assert_eq!(contracts.input_example, "data-anvil-action=\"input\"");
        assert_eq!(contracts.primary_example, "data-anvil-action=\"primary\"");
    }

    #[test]
    fn embedded_repair_guidance_keeps_scenario_variants_byte_stable() {
        let guidance = &get().repair_guidance;
        assert_eq!(
            guidance.generic_interaction,
            "input operations must visibly change actual application state, and that change must be reflected in the data-anvil-state JSON snapshot; wire input handlers to state updates."
        );
        assert_eq!(
            guidance.start_interaction,
            "primary/start controls must transition the visible app state before input is evaluated; wire the start action into state and render updates."
        );
        assert_eq!(
            guidance.canvas_game_interaction,
            "keyboard or pointer input must visibly change game state (player position, projectiles, score/health, or state transitions); wire input handlers into the render/update loop."
        );
        assert_eq!(
            guidance.canvas_render_loop_checklist,
            "render-loop checklist: ref attached -> effect runs -> rAF loop starts -> draw calls"
        );
        assert_eq!(
            guidance.canvas_input_wiring_checklist,
            "input-wiring checklist: keyboard or pointer input must visibly change game state (player position, projectiles, score/health, or state transitions); wire input handlers into the render/update loop."
        );
        assert_eq!(
            guidance.persistence,
            "load persisted state on mount (e.g. read localStorage in initialization) and write on mutation"
        );
    }

    #[test]
    fn embedded_canonical_knowledge_keeps_scaffold_contract() {
        let canonical = &get().canonical;
        assert_eq!(canonical.package_script_build, "next build");
        assert_eq!(canonical.package_script_dev, "next dev -p {port}");
        assert_eq!(canonical.package_script_start, "next start -p {port}");
        assert_eq!(
            canonical.required_hooks,
            vec![
                "data-anvil-action=\"primary\"",
                "data-anvil-action=\"restart\"",
                "data-anvil-state",
            ]
        );
        assert_eq!(canonical.scaffold_files.len(), 8);
        assert_eq!(canonical.scaffold_files[0], "package.json");
        assert_eq!(canonical.scaffold_files[3], "{tailwind_config}");
        assert_eq!(canonical.scaffold_files[7], "src/app/global.d.ts");
        assert_eq!(canonical.tailwind_config_rels[0], "tailwind.config.ts");
        assert_eq!(canonical.tailwind_config_rels.len(), 4);

        let package: serde_json::Value = serde_json::from_str(&canonical.package_json).unwrap();
        assert_eq!(package["scripts"]["build"], "next build");
        assert_eq!(package["scripts"]["dev"], "next dev -p 3011");
        assert_eq!(package["scripts"]["start"], "next start -p 3011");
        assert!(canonical.package_json.ends_with("}\n"));
        assert!(serde_json::from_str::<serde_json::Value>(&canonical.tsconfig).is_ok());
        assert_eq!(
            canonical.postcss_config,
            "module.exports = { plugins: { tailwindcss: {}, autoprefixer: {} } };\n"
        );
        assert_eq!(
            canonical.tailwind_css,
            "@tailwind base;\n@tailwind components;\n@tailwind utilities;\n"
        );
        assert_eq!(canonical.global_d_ts, "declare module \"*.css\";\n");
        assert!(
            canonical
                .tailwind_config
                .ends_with("export default config;\n")
        );
        assert!(canonical.tailwind_config_cjs.ends_with("};\n"));
        assert!(
            canonical
                .layout_tsx
                .contains("export default function RootLayout")
        );
    }
}
