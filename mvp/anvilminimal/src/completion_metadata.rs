use crate::config::Config;
use crate::eval_events::{
    CompletionSnapshot, GENERIC_REDUCED_ASSURANCE_REASON, GENERIC_STATIC_ASSURANCE_REASON,
};
use crate::planner::profile::canonical_profile_name;
use crate::planner::profiles::data::runtime::{DataAssurance, assurance_from_evidence};

pub(crate) fn apply_config_completion_metadata(config: &Config, snapshot: &mut CompletionSnapshot) {
    if let Some(inference) = config.profile_inference {
        snapshot.profile_inferred = inference.profile.to_string();
        snapshot.profile_inference_source = inference.source.as_str().to_string();
    }
    if snapshot.profile.trim().is_empty() {
        snapshot.profile = config.profile.clone();
    }
    if snapshot.effective_profile.trim().is_empty() {
        snapshot.effective_profile = snapshot.profile.clone();
    }
    if snapshot.prompt_layout.trim().is_empty() {
        snapshot.prompt_layout = config.prompt_layout.as_str().to_string();
    }

    if canonical_profile_name(&snapshot.effective_profile) == "data" {
        let (assurance, reason) = data_completion_assurance(config);
        snapshot.assurance_level = assurance.as_str().to_string();
        snapshot.assurance_reason = reason.to_string();
    } else if canonical_profile_name(&snapshot.profile) == "generic" {
        if snapshot.assurance_level == "static" {
            snapshot.assurance_reason = GENERIC_STATIC_ASSURANCE_REASON.to_string();
        } else {
            snapshot.assurance_level = "reduced".to_string();
            snapshot.assurance_reason = GENERIC_REDUCED_ASSURANCE_REASON.to_string();
        }
    } else {
        snapshot.assurance_level = "full".to_string();
        snapshot.assurance_reason.clear();
    }
}

fn data_completion_assurance(config: &Config) -> (DataAssurance, &'static str) {
    let assurance = assurance_from_evidence(&config.workspace_root);
    let reason = match assurance {
        DataAssurance::Full => "",
        DataAssurance::Partial => "data_assurance_partial",
        DataAssurance::Static => "data_profile_probe_not_run",
        DataAssurance::Failed if !config.workspace_root.join("pipeline/main.py").is_file() => {
            "data_profile_script_not_generated"
        }
        DataAssurance::Failed => "data_assurance_failed",
    };
    (assurance, reason)
}
