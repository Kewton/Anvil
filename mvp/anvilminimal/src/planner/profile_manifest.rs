use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::sync::OnceLock;

use serde::Deserialize;
use toml::value::Table;

use crate::planner::capability_catalog::{self, CatalogError, ResolvedCapability};

const NEXTJS_MANIFEST_TOML: &str = include_str!("profiles/nextjs/manifest.toml");

pub const MANIFEST_V0_SECTIONS: &[&str] = &[
    "metadata",
    "plan",
    "step_templates",
    "vocabulary",
    "guidance",
    "checks",
    "evidence_targets",
];

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ManifestV0 {
    pub metadata: ManifestMetadata,
    pub plan: ManifestPlan,
    pub step_templates: StepTemplates,
    pub vocabulary: VocabularyReference,
    pub guidance: ManifestGuidance,
    pub checks: BTreeMap<String, Vec<CheckBinding>>,
    pub evidence_targets: EvidenceTargetsReference,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ManifestMetadata {
    pub id: String,
    pub display_name: String,
    pub schema_version: SchemaVersion,
    pub status: ManifestStatus,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
pub enum SchemaVersion {
    #[serde(rename = "v0")]
    V0,
}

impl SchemaVersion {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::V0 => "v0",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ManifestStatus {
    Draft,
    Admitted,
}

impl ManifestStatus {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Draft => "draft",
            Self::Admitted => "admitted",
        }
    }
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ManifestPlan {
    pub profile: String,
    pub style: String,
    pub intent: String,
    pub placeholders: PlanPlaceholders,
    pub phases: Vec<ManifestPlanPhase>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PlanPlaceholders {
    pub goal: String,
    pub port: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ManifestPlanPhase {
    pub id: String,
    pub prompt: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StepTemplates {
    pub scaffold: ScaffoldTemplateMatcher,
    pub build_verify: PhaseKeywordMatcher,
    pub implementation_kill: PhaseKeywordMatcher,
    pub ownership: TemplateOwnership,
    pub artifacts: TemplateArtifacts,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ScaffoldTemplateMatcher {
    pub phase: Vec<String>,
    pub phase_id: Vec<String>,
    pub port_phase_markers: Vec<String>,
    pub port_script_phase: Vec<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PhaseKeywordMatcher {
    pub phase: Vec<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TemplateOwnership {
    pub setup_classifier: SetupClassifier,
    pub template_owned_artifacts: TemplateOwnedArtifacts,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SetupClassifier {
    pub package_phrases: Vec<String>,
    pub package_tokens: Vec<String>,
    pub scaffold_phrases: Vec<String>,
    pub scaffold_tokens: Vec<String>,
    pub scaffold_setup_markers: Vec<String>,
    pub scaffold_project_marker: String,
    pub scaffold_dependency_exclusion: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TemplateOwnedArtifacts {
    pub package_phrases: Vec<String>,
    pub package_tokens: Vec<String>,
    pub scaffold_phrases: Vec<String>,
    pub scaffold_tokens: Vec<String>,
    pub package_manifest_names: Vec<String>,
    pub artifact_path_suffixes: Vec<String>,
    pub artifact_path_contains: Vec<String>,
    pub package_check_marker: String,
    pub scaffold_check_marker: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TemplateArtifacts {
    pub package_script_build: String,
    pub package_script_dev: String,
    pub package_script_start: String,
    pub required_hooks: Vec<String>,
    pub scaffold_files: Vec<String>,
    pub tailwind_config_rels: Vec<String>,
    pub tailwind_config: String,
    pub tailwind_config_cjs: String,
    pub package_json: String,
    pub tsconfig: String,
    pub postcss_config: String,
    pub tailwind_css: String,
    pub global_d_ts: String,
    pub layout_tsx: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct VocabularyReference {
    pub source: SharedKnowledgeSource,
    pub sections: Vec<VocabularySection>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SharedKnowledgeSource {
    EvidenceKnowledge,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Deserialize)]
pub enum VocabularySection {
    #[serde(rename = "vocabulary")]
    Vocabulary,
    #[serde(rename = "goal_hints.translations")]
    GoalHintTranslations,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ManifestGuidance {
    pub generic: GenericGuidance,
    pub canvas_game: CanvasGameGuidance,
    pub persistence: PersistenceGuidance,
    pub contracts: ContractGuidance,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GenericGuidance {
    pub generic_interaction: String,
    pub start_interaction: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CanvasGameGuidance {
    pub canvas_game_interaction: String,
    pub canvas_render_loop_checklist: String,
    pub canvas_input_wiring_checklist: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PersistenceGuidance {
    pub persistence: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ContractGuidance {
    pub state_binding_contract: String,
    pub input_coupled_dimension_requirement: String,
    pub contract_attribute_missing_kind: String,
    pub contract_attribute_guidance: String,
    pub state_requirement: String,
    pub restart_requirement: String,
    pub input_requirement: String,
    pub primary_requirement: String,
    pub state_example: String,
    pub restart_example: String,
    pub input_example: String,
    pub primary_example: String,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CheckBinding {
    pub id: String,
    #[serde(default)]
    pub params: Table,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EvidenceTargetsReference {
    pub source: SharedKnowledgeSource,
    pub section: EvidenceTargetsSection,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
pub enum EvidenceTargetsSection {
    #[serde(rename = "repair_targets")]
    RepairTargets,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedCheck {
    pub id: String,
    pub capability: ResolvedCapability,
}

pub type ResolvedCheckBindings = BTreeMap<String, Vec<ResolvedCheck>>;

#[derive(Debug)]
pub enum ManifestError {
    Parse(toml::de::Error),
    Invalid {
        field: &'static str,
        reason: String,
    },
    CheckBinding {
        binding: String,
        index: usize,
        id: String,
        source: Box<CatalogError>,
    },
}

impl fmt::Display for ManifestError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Parse(err) => write!(f, "manifest TOML is invalid: {err}"),
            Self::Invalid { field, reason } => {
                write!(f, "manifest field `{field}` is invalid: {reason}")
            }
            Self::CheckBinding {
                binding,
                index,
                id,
                source,
            } => write!(
                f,
                "check binding `{binding}` entry {index} (`{id}`) is invalid: {source}"
            ),
        }
    }
}

impl std::error::Error for ManifestError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Parse(err) => Some(err),
            Self::CheckBinding { source, .. } => Some(source.as_ref()),
            Self::Invalid { .. } => None,
        }
    }
}

impl ManifestV0 {
    pub fn from_toml(input: &str) -> Result<Self, ManifestError> {
        let manifest = toml::from_str::<Self>(input).map_err(ManifestError::Parse)?;
        manifest.resolve()?;
        Ok(manifest)
    }

    pub fn resolve(&self) -> Result<ResolvedCheckBindings, ManifestError> {
        self.validate_structure()?;
        let mut resolved = BTreeMap::new();
        for (binding, checks) in &self.checks {
            let mut entries = Vec::with_capacity(checks.len());
            for (index, check) in checks.iter().enumerate() {
                let capability =
                    capability_catalog::resolve(&check.id, &check.params).map_err(|source| {
                        ManifestError::CheckBinding {
                            binding: binding.clone(),
                            index,
                            id: check.id.clone(),
                            source: Box::new(source),
                        }
                    })?;
                entries.push(ResolvedCheck {
                    id: check.id.clone(),
                    capability,
                });
            }
            resolved.insert(binding.clone(), entries);
        }
        Ok(resolved)
    }

    fn validate_structure(&self) -> Result<(), ManifestError> {
        require_non_empty("metadata.id", &self.metadata.id)?;
        require_non_empty("metadata.display_name", &self.metadata.display_name)?;
        require_non_empty("plan.profile", &self.plan.profile)?;
        require_non_empty("plan.style", &self.plan.style)?;
        require_non_empty("plan.intent", &self.plan.intent)?;
        if self.plan.profile != self.metadata.id {
            return Err(invalid(
                "plan.profile",
                "must match metadata.id for schema v0",
            ));
        }
        if self.plan.placeholders.goal != "{goal}" {
            return Err(invalid(
                "plan.placeholders.goal",
                "must be the literal {goal}",
            ));
        }
        if self.plan.placeholders.port != "{port}" {
            return Err(invalid(
                "plan.placeholders.port",
                "must be the literal {port}",
            ));
        }
        if self.plan.phases.is_empty() {
            return Err(invalid("plan.phases", "must contain at least one phase"));
        }
        let mut phase_ids = BTreeSet::new();
        for phase in &self.plan.phases {
            require_non_empty("plan.phases[].id", &phase.id)?;
            require_non_empty("plan.phases[].prompt", &phase.prompt)?;
            if !phase_ids.insert(phase.id.as_str()) {
                return Err(invalid("plan.phases[].id", "phase ids must be unique"));
            }
        }
        let vocabulary_sections = self
            .vocabulary
            .sections
            .iter()
            .copied()
            .collect::<BTreeSet<_>>();
        if vocabulary_sections
            != BTreeSet::from([
                VocabularySection::Vocabulary,
                VocabularySection::GoalHintTranslations,
            ])
        {
            return Err(invalid(
                "vocabulary.sections",
                "must reference vocabulary and goal_hints.translations",
            ));
        }
        if self.checks.is_empty() {
            return Err(invalid("checks", "must contain at least one binding"));
        }
        for (binding, checks) in &self.checks {
            if binding.trim().is_empty() {
                return Err(invalid("checks", "binding names may not be empty"));
            }
            if checks.is_empty() {
                return Err(invalid(
                    "checks.<binding>",
                    "must contain at least one check",
                ));
            }
            for check in checks {
                require_non_empty("checks.<binding>[].id", &check.id)?;
            }
        }
        Ok(())
    }
}

pub fn nextjs_manifest() -> &'static ManifestV0 {
    static MANIFEST: OnceLock<ManifestV0> = OnceLock::new();
    MANIFEST.get_or_init(|| {
        ManifestV0::from_toml(NEXTJS_MANIFEST_TOML)
            .expect("embedded Next.js profile manifest.toml must parse and resolve")
    })
}

fn require_non_empty(field: &'static str, value: &str) -> Result<(), ManifestError> {
    if value.trim().is_empty() {
        Err(invalid(field, "must not be empty"))
    } else {
        Ok(())
    }
}

fn invalid(field: &'static str, reason: impl Into<String>) -> ManifestError {
    ManifestError::Invalid {
        field,
        reason: reason.into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::planner::profiles::nextjs::knowledge;

    #[test]
    fn embedded_manifest_keeps_existing_nextjs_knowledge_values() {
        let manifest = nextjs_manifest();
        let existing = knowledge::get();

        assert_eq!(manifest.plan.profile, existing.preset.profile);
        assert_eq!(manifest.plan.style, existing.preset.style);
        assert_eq!(manifest.plan.intent, existing.preset.intent);
        assert_eq!(manifest.plan.phases.len(), existing.preset.phases.len());
        for (manifest_phase, existing_phase) in
            manifest.plan.phases.iter().zip(&existing.preset.phases)
        {
            assert_eq!(manifest_phase.id, existing_phase.id);
            assert_eq!(manifest_phase.prompt, existing_phase.prompt);
        }

        let templates = &manifest.step_templates;
        assert_eq!(
            templates.scaffold.phase,
            existing.deterministic_keywords.scaffold_phase
        );
        assert_eq!(
            templates.scaffold.phase_id,
            existing.deterministic_keywords.scaffold_phase_id
        );
        assert_eq!(
            templates.scaffold.port_phase_markers,
            existing.deterministic_keywords.port_phase_markers
        );
        assert_eq!(
            templates.scaffold.port_script_phase,
            existing.deterministic_keywords.port_script_phase
        );
        assert_eq!(
            templates.build_verify.phase,
            existing.deterministic_keywords.build_verify_phase
        );
        assert_eq!(
            templates.implementation_kill.phase,
            existing.deterministic_keywords.implementation_phase
        );
        assert_eq!(
            templates.ownership.setup_classifier.package_phrases,
            existing.setup_classifier.package_phrases
        );
        assert_eq!(
            templates.ownership.setup_classifier.package_tokens,
            existing.setup_classifier.package_tokens
        );
        assert_eq!(
            templates.ownership.setup_classifier.scaffold_phrases,
            existing.setup_classifier.scaffold_phrases
        );
        assert_eq!(
            templates.ownership.setup_classifier.scaffold_tokens,
            existing.setup_classifier.scaffold_tokens
        );
        assert_eq!(
            templates.ownership.setup_classifier.scaffold_setup_markers,
            existing.setup_classifier.scaffold_setup_markers
        );
        assert_eq!(
            templates.ownership.setup_classifier.scaffold_project_marker,
            existing.setup_classifier.scaffold_project_marker
        );
        assert_eq!(
            templates
                .ownership
                .setup_classifier
                .scaffold_dependency_exclusion,
            existing.setup_classifier.scaffold_dependency_exclusion
        );
        let owned = &templates.ownership.template_owned_artifacts;
        assert_eq!(
            owned.package_phrases,
            existing.template_owned_artifacts.package_phrases
        );
        assert_eq!(
            owned.package_tokens,
            existing.template_owned_artifacts.package_tokens
        );
        assert_eq!(
            owned.scaffold_phrases,
            existing.template_owned_artifacts.scaffold_phrases
        );
        assert_eq!(
            owned.scaffold_tokens,
            existing.template_owned_artifacts.scaffold_tokens
        );
        assert_eq!(
            owned.package_manifest_names,
            existing.template_owned_artifacts.package_manifest_names
        );
        assert_eq!(
            templates
                .ownership
                .template_owned_artifacts
                .artifact_path_suffixes,
            existing.template_owned_artifacts.artifact_path_suffixes
        );
        assert_eq!(
            owned.artifact_path_contains,
            existing.template_owned_artifacts.artifact_path_contains
        );
        assert_eq!(
            owned.package_check_marker,
            existing.template_owned_artifacts.package_check_marker
        );
        assert_eq!(
            owned.scaffold_check_marker,
            existing.template_owned_artifacts.scaffold_check_marker
        );

        let artifacts = &templates.artifacts;
        let canonical = &existing.canonical;
        assert_eq!(
            artifacts.package_script_build,
            canonical.package_script_build
        );
        assert_eq!(artifacts.package_script_dev, canonical.package_script_dev);
        assert_eq!(
            artifacts.package_script_start,
            canonical.package_script_start
        );
        assert_eq!(artifacts.required_hooks, canonical.required_hooks);
        assert_eq!(artifacts.scaffold_files, canonical.scaffold_files);
        assert_eq!(
            artifacts.tailwind_config_rels,
            canonical.tailwind_config_rels
        );
        assert_eq!(artifacts.tailwind_config, canonical.tailwind_config);
        assert_eq!(artifacts.tailwind_config_cjs, canonical.tailwind_config_cjs);
        assert_eq!(artifacts.package_json, canonical.package_json);
        assert_eq!(artifacts.tsconfig, canonical.tsconfig);
        assert_eq!(artifacts.postcss_config, canonical.postcss_config);
        assert_eq!(artifacts.tailwind_css, canonical.tailwind_css);
        assert_eq!(artifacts.global_d_ts, canonical.global_d_ts);
        assert_eq!(artifacts.layout_tsx, canonical.layout_tsx);

        let guidance = &manifest.guidance;
        assert_eq!(
            guidance.generic.generic_interaction,
            existing.repair_guidance.generic_interaction
        );
        assert_eq!(
            guidance.generic.start_interaction,
            existing.repair_guidance.start_interaction
        );
        assert_eq!(
            guidance.canvas_game.canvas_game_interaction,
            existing.repair_guidance.canvas_game_interaction
        );
        assert_eq!(
            guidance.canvas_game.canvas_render_loop_checklist,
            existing.repair_guidance.canvas_render_loop_checklist
        );
        assert_eq!(
            guidance.canvas_game.canvas_input_wiring_checklist,
            existing.repair_guidance.canvas_input_wiring_checklist
        );
        assert_eq!(
            guidance.persistence.persistence,
            existing.repair_guidance.persistence
        );
        assert_eq!(
            guidance.contracts.state_binding_contract,
            existing.contracts.state_binding_contract
        );
        assert_eq!(
            guidance.contracts.input_coupled_dimension_requirement,
            existing.contracts.input_coupled_dimension_requirement
        );
        assert_eq!(
            guidance.contracts.contract_attribute_missing_kind,
            existing.contracts.contract_attribute_missing_kind
        );
        assert_eq!(
            guidance.contracts.contract_attribute_guidance,
            existing.contracts.contract_attribute_guidance
        );
        assert_eq!(
            guidance.contracts.state_requirement,
            existing.contracts.state_requirement
        );
        assert_eq!(
            guidance.contracts.restart_requirement,
            existing.contracts.restart_requirement
        );
        assert_eq!(
            guidance.contracts.input_requirement,
            existing.contracts.input_requirement
        );
        assert_eq!(
            guidance.contracts.primary_requirement,
            existing.contracts.primary_requirement
        );
        assert_eq!(
            guidance.contracts.state_example,
            existing.contracts.state_example
        );
        assert_eq!(
            guidance.contracts.restart_example,
            existing.contracts.restart_example
        );
        assert_eq!(
            guidance.contracts.input_example,
            existing.contracts.input_example
        );
        assert_eq!(
            guidance.contracts.primary_example,
            existing.contracts.primary_example
        );
    }

    #[test]
    fn embedded_loader_parses_once() {
        assert!(std::ptr::eq(nextjs_manifest(), nextjs_manifest()));
    }

    #[test]
    fn unknown_root_section_is_rejected() {
        let invalid = format!("{NEXTJS_MANIFEST_TOML}\n[unknown_section]\nvalue = true\n");
        assert!(matches!(
            ManifestV0::from_toml(&invalid),
            Err(ManifestError::Parse(_))
        ));
    }

    #[test]
    fn unknown_check_id_is_rejected_during_load() {
        let invalid = NEXTJS_MANIFEST_TOML.replacen(
            "id = \"next_build_verify\"",
            "id = \"unknown_build_check\"",
            1,
        );
        assert!(matches!(
            ManifestV0::from_toml(&invalid),
            Err(ManifestError::CheckBinding { source, .. })
                if matches!(source.as_ref(), CatalogError::UnknownId(id) if id == "unknown_build_check")
        ));
    }

    #[test]
    fn invalid_check_params_are_rejected_during_load() {
        let invalid = NEXTJS_MANIFEST_TOML.replacen("port = 3011", "port = \"3011\"", 1);
        assert!(matches!(
            ManifestV0::from_toml(&invalid),
            Err(ManifestError::CheckBinding { source, .. })
                if matches!(
                    source.as_ref(),
                    CatalogError::TypeMismatch { parameter, .. } if parameter == "port"
                )
        ));
    }

    #[test]
    fn invalid_status_is_rejected() {
        let invalid =
            NEXTJS_MANIFEST_TOML.replacen("status = \"draft\"", "status = \"retired\"", 1);
        assert!(matches!(
            ManifestV0::from_toml(&invalid),
            Err(ManifestError::Parse(_))
        ));
    }
}
