use std::sync::OnceLock;

use serde::Deserialize;

const KNOWLEDGE_TOML: &str = include_str!("knowledge.toml");

#[derive(Debug, Deserialize)]
pub(crate) struct NextJsKnowledge {
    pub(crate) preset: PresetKnowledge,
    pub(crate) deterministic_keywords: DeterministicKeywords,
    pub(crate) setup_classifier: SetupClassifierKnowledge,
    pub(crate) template_owned_artifacts: TemplateOwnedArtifactKnowledge,
    #[allow(dead_code)]
    pub(crate) contracts: ContractKnowledge,
    #[allow(dead_code)]
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
#[allow(dead_code)]
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
#[allow(dead_code)]
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

#[cfg(test)]
mod tests {
    use super::*;

    const LEGACY_PRESET_PHASES: &[(&str, &str)] = &[
        (
            "project-setup",
            "Scaffold and setup the Next.js App Router project shell. Create or complete the package manifest, TypeScript config, styling config, and route-bound scaffold so the deterministic nextjs-scaffold template owns setup artifacts.",
        ),
        (
            "core-implementation",
            "Implement the core task-specific behavior for: {goal}. Keep one route-bound implementation, extend the instrumented skeleton instead of replacing it, and keep the implementation in the Next.js route-bound source.",
        ),
        (
            "contract-wiring",
            "Wire controls and data-anvil observability. Preserve or add data-anvil-action=\"primary\" on the main start/submit/action control, data-anvil-action=\"input\" on the main text entry surface when one exists, and data-anvil-state with a JSON snapshot of meaningful visible state. The data-anvil-state snapshot must include at least one dimension that immediately responds to input, such as player/paddle x position. When the contract includes start_or_restart_flow, every restart affordance (game-over, victory, and in-play when present) should carry data-anvil-action=\"restart\"; the initial primary action alone cannot satisfy recovery verification.",
        ),
        (
            "build-verification",
            "Run build verification for the deterministic Next.js scaffold. Verify package scripts, dependency boundary, and npm run build / next build only; keep this final phase verification-only.",
        ),
    ];

    const LEGACY_SCAFFOLD_PHASE: &[&str] = &[
        "scaffold",
        "setup",
        "set up",
        "project shell",
        "initialize",
        "initialise",
        "bootstrap",
        "app router scaffold",
        "初期",
        "セットアップ",
    ];
    const LEGACY_SCAFFOLD_PHASE_ID: &[&str] = &[
        "scaffold",
        "setup",
        "set-up",
        "project-setup",
        "bootstrap",
        "initialize",
        "initialise",
    ];
    const LEGACY_PORT_SCRIPT_PHASE: &[&str] = &[
        "script",
        "package",
        "package.json",
        "dev/start",
        "dev script",
        "start script",
        "設定",
    ];
    const LEGACY_BUILD_VERIFY_PHASE: &[&str] = &[
        "build verification",
        "verify build",
        "build verifier",
        "npm run build",
        "next build",
        "ビルド検証",
    ];
    const LEGACY_IMPLEMENTATION_PHASE: &[&str] = &[
        "game logic",
        "gameplay",
        "mechanic",
        "adversary",
        "challenge",
        "collision",
        "failure rule",
        "score",
        "player control",
        "canvas",
        "stateful update",
        "user input",
        "interactive surface",
        "ゲームロジック",
        "衝突",
        "スコア",
        "敵",
        "プレイヤー",
        "操作",
    ];

    fn strings(values: &[&str]) -> Vec<String> {
        values.iter().map(|value| (*value).to_string()).collect()
    }

    #[test]
    fn embedded_knowledge_parses_and_matches_legacy_preset_bytes() {
        let knowledge = get();
        assert_eq!(knowledge.preset.profile, "nextjs");
        assert_eq!(knowledge.preset.style, "default");
        assert_eq!(knowledge.preset.intent, "create");
        assert_eq!(knowledge.preset.phases.len(), LEGACY_PRESET_PHASES.len());
        for (phase, (legacy_id, legacy_prompt)) in knowledge
            .preset
            .phases
            .iter()
            .zip(LEGACY_PRESET_PHASES.iter())
        {
            assert_eq!(phase.id, *legacy_id);
            assert_eq!(phase.prompt, *legacy_prompt);
        }
    }

    #[test]
    fn deterministic_keywords_match_legacy_arrays() {
        let keywords = &get().deterministic_keywords;
        assert_eq!(keywords.scaffold_phase, strings(LEGACY_SCAFFOLD_PHASE));
        assert_eq!(
            keywords.scaffold_phase_id,
            strings(LEGACY_SCAFFOLD_PHASE_ID)
        );
        assert_eq!(keywords.port_phase_markers, strings(&["port", "ポート"]));
        assert_eq!(
            keywords.port_script_phase,
            strings(LEGACY_PORT_SCRIPT_PHASE)
        );
        assert_eq!(
            keywords.build_verify_phase,
            strings(LEGACY_BUILD_VERIFY_PHASE)
        );
        assert_eq!(
            keywords.implementation_phase,
            strings(LEGACY_IMPLEMENTATION_PHASE)
        );
    }

    #[test]
    fn setup_and_template_owned_tokens_match_legacy_values() {
        let knowledge = get();
        assert_eq!(
            knowledge.setup_classifier.package_phrases,
            strings(&[
                "package.json",
                "package manifest",
                "package script",
                "port script",
            ])
        );
        assert_eq!(
            knowledge.setup_classifier.package_tokens,
            strings(&["script", "scripts", "manifest", "port"])
        );
        assert_eq!(
            knowledge.setup_classifier.scaffold_phrases,
            strings(&[
                "scaffold",
                "project shell",
                "tsconfig",
                "postcss",
                "tailwind",
            ])
        );
        assert_eq!(
            knowledge.setup_classifier.scaffold_tokens,
            strings(&["config"])
        );
        assert_eq!(
            knowledge.setup_classifier.scaffold_setup_markers,
            strings(&["setup", "set up"])
        );
        assert_eq!(
            knowledge.setup_classifier.scaffold_project_marker,
            "project"
        );
        assert_eq!(
            knowledge.setup_classifier.scaffold_dependency_exclusion,
            "dependenc"
        );
        assert_eq!(
            knowledge.template_owned_artifacts.package_phrases,
            strings(&[
                "package.json",
                "package manifest",
                "package script",
                "npm script",
                "port script",
                "dev script",
                "start script",
                "build script",
            ])
        );
        assert_eq!(
            knowledge.template_owned_artifacts.package_tokens,
            strings(&["scripts", "port", "ports"])
        );
        assert_eq!(
            knowledge.template_owned_artifacts.scaffold_phrases,
            strings(&[
                "tsconfig",
                "postcss",
                "tailwind",
                "next.config",
                "next-env.d.ts",
                "src/app/layout.tsx",
                "src/app/globals.css",
                "src/app/global.d.ts",
            ])
        );
        assert_eq!(
            knowledge.template_owned_artifacts.scaffold_tokens,
            strings(&["scaffold", "scaffolding"])
        );
        assert_eq!(
            knowledge.template_owned_artifacts.package_manifest_names,
            strings(&["package.json"])
        );
        assert_eq!(
            knowledge.template_owned_artifacts.artifact_path_suffixes,
            strings(&[
                "tsconfig.json",
                "next-env.d.ts",
                "src/app/layout.tsx",
                "src/app/globals.css",
                "src/app/global.d.ts",
            ])
        );
        assert_eq!(
            knowledge.template_owned_artifacts.artifact_path_contains,
            strings(&["postcss.config.", "tailwind.config.", "next.config."])
        );
        assert_eq!(
            knowledge.template_owned_artifacts.package_check_marker,
            "package.json scripts port"
        );
        assert_eq!(
            knowledge.template_owned_artifacts.scaffold_check_marker,
            "scaffold tsconfig postcss tailwind"
        );
    }

    #[test]
    fn canonical_values_match_legacy_rust_values() {
        use crate::planner::profile::ProfileHookAttribute;

        let canonical = &get().canonical;
        assert_eq!(canonical.package_script_build, "next build");
        assert_eq!(canonical.package_script_dev, "next dev -p {port}");
        assert_eq!(canonical.package_script_start, "next start -p {port}");
        assert_eq!(
            canonical.required_hooks,
            [
                ProfileHookAttribute::PrimaryAction.display(),
                ProfileHookAttribute::RestartAction.display(),
                ProfileHookAttribute::State.display(),
            ]
        );
        assert_eq!(
            canonical.tailwind_config_rels,
            strings(&[
                "tailwind.config.ts",
                "tailwind.config.js",
                "tailwind.config.cjs",
                "tailwind.config.mjs",
            ])
        );
        assert_eq!(
            canonical.tailwind_config,
            super::super::canonical_tailwind_config()
        );
        assert_eq!(
            canonical.tailwind_config_cjs,
            super::super::canonical_tailwind_config_cjs()
        );
        assert_eq!(
            canonical.package_json,
            super::super::canonical_package_json()
        );
        assert_eq!(canonical.tsconfig, super::super::canonical_tsconfig());
        assert_eq!(
            canonical.postcss_config,
            super::super::canonical_postcss_config()
        );
        assert_eq!(
            canonical.tailwind_css,
            super::super::canonical_tailwind_css()
        );
        assert_eq!(canonical.global_d_ts, super::super::canonical_global_d_ts());
        assert_eq!(canonical.layout_tsx, super::super::canonical_layout_tsx());

        let dir = tempfile::tempdir().unwrap();
        let loaded_paths = canonical
            .scaffold_files
            .iter()
            .map(|path| path.replace("{tailwind_config}", "tailwind.config.ts"))
            .collect::<Vec<_>>();
        assert_eq!(loaded_paths, super::super::setup_scaffold_paths(dir.path()));
    }
}
