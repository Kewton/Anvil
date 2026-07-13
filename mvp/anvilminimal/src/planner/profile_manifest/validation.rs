use std::collections::BTreeSet;

use super::{
    EvidenceTargetsSection, ManifestError, ManifestV0, SharedKnowledgeSource, VocabularySection,
};

pub(super) fn validate(manifest: &ManifestV0) -> Result<(), ManifestError> {
    require_non_empty("metadata.id", &manifest.metadata.id)?;
    require_non_empty("metadata.display_name", &manifest.metadata.display_name)?;
    require_non_empty("plan.profile", &manifest.plan.profile)?;
    require_non_empty("plan.style", &manifest.plan.style)?;
    require_non_empty("plan.intent", &manifest.plan.intent)?;
    if manifest.plan.profile != manifest.metadata.id {
        return Err(invalid(
            "plan.profile",
            "must match metadata.id for schema v0",
        ));
    }
    if manifest.plan.placeholders.goal != "{goal}" {
        return Err(invalid(
            "plan.placeholders.goal",
            "must be the literal {goal}",
        ));
    }
    if manifest
        .plan
        .placeholders
        .port
        .as_deref()
        .is_some_and(|port| port != "{port}")
    {
        return Err(invalid(
            "plan.placeholders.port",
            "when present, must be the literal {port}",
        ));
    }
    validate_phases(manifest)?;
    validate_vocabulary(manifest)?;
    validate_checks(manifest)?;
    validate_evidence_targets(manifest)?;
    Ok(())
}

fn validate_phases(manifest: &ManifestV0) -> Result<(), ManifestError> {
    if manifest.plan.phases.is_empty() {
        return Err(invalid("plan.phases", "must contain at least one phase"));
    }
    let mut phase_ids = BTreeSet::new();
    for phase in &manifest.plan.phases {
        require_non_empty("plan.phases[].id", &phase.id)?;
        require_non_empty("plan.phases[].prompt", &phase.prompt)?;
        if !phase_ids.insert(phase.id.as_str()) {
            return Err(invalid("plan.phases[].id", "phase ids must be unique"));
        }
    }
    Ok(())
}

fn validate_vocabulary(manifest: &ManifestV0) -> Result<(), ManifestError> {
    let sections = manifest
        .vocabulary
        .sections
        .iter()
        .copied()
        .collect::<BTreeSet<_>>();
    if sections
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
    Ok(())
}

fn validate_checks(manifest: &ManifestV0) -> Result<(), ManifestError> {
    if manifest.checks.is_empty() {
        return Err(invalid("checks", "must contain at least one binding"));
    }
    for (binding, checks) in &manifest.checks {
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

fn validate_evidence_targets(manifest: &ManifestV0) -> Result<(), ManifestError> {
    let targets = &manifest.evidence_targets;
    match (targets.source, targets.section, targets.mappings.is_empty()) {
        (
            Some(SharedKnowledgeSource::EvidenceKnowledge),
            Some(EvidenceTargetsSection::RepairTargets),
            true,
        ) => Ok(()),
        (None, None, false) => {
            for (evidence, paths) in &targets.mappings {
                require_non_empty("evidence_targets.mappings.<evidence>", evidence)?;
                if paths.is_empty() {
                    return Err(invalid(
                        "evidence_targets.mappings.<evidence>",
                        "must contain at least one path",
                    ));
                }
                for path in paths {
                    crate::tools::path_guard::validate_workspace_relative(path).map_err(
                        |error| {
                            invalid("evidence_targets.mappings.<evidence>[]", error.to_string())
                        },
                    )?;
                }
            }
            Ok(())
        }
        _ => Err(invalid(
            "evidence_targets",
            "must be exactly one shared repair_targets reference or one non-empty local mapping",
        )),
    }
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
