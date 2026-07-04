use std::path::Path;

use crate::agent::text_tokens;

use super::{
    UltraPhase, UltraPlan, UltraProfile, WorkIntent, profiles, required_artifact_contract_prompt,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct ProfileSnapshot {
    pub(super) lines: Vec<String>,
    pub(super) protected_files: Vec<ProtectedFile>,
    pub(super) requested_port: Option<u16>,
    pub(super) probe_port: Option<u16>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct ProtectedFile {
    pub(super) path: String,
    pub(super) len: u64,
}

impl ProfileSnapshot {
    pub(super) fn new(lines: Vec<String>, protected_files: Vec<ProtectedFile>) -> Self {
        Self {
            lines,
            protected_files,
            requested_port: None,
            probe_port: None,
        }
    }
}

pub(super) fn profile_generation_rules(profile: UltraProfile, intent: WorkIntent) -> &'static str {
    match profile {
        UltraProfile::Generic => "",
        UltraProfile::Nextjs => profiles::nextjs::generation_rules(intent),
        UltraProfile::Python => {
            "- Profile python: preserve the package/import layout, prefer pytest or python -m py_compile checks, and add tests for behavioral changes when practical.\n"
        }
        UltraProfile::Rust => {
            "- Profile rust: preserve Cargo.toml and crate entrypoints, prefer cargo check/cargo test checks, and keep changes scoped to the requested crate behavior.\n"
        }
        UltraProfile::Investigation => {
            "- Profile investigation: produce a concrete triage/report artifact, separate facts from hypotheses, and do not modify source code unless the user explicitly asks for fixes.\n"
        }
        UltraProfile::Docs => {
            "- Profile docs: produce or update documentation artifacts, avoid source-code changes unless explicitly requested, and include a final review phase for accuracy.\n"
        }
        UltraProfile::DataAnalysis | UltraProfile::DataPipeline => {
            profiles::data::generation_rules(profile)
        }
    }
}

pub(super) fn profile_snapshot(work_root: &Path, profile: UltraProfile) -> ProfileSnapshot {
    match profile {
        UltraProfile::DataAnalysis | UltraProfile::DataPipeline => {
            profiles::data::snapshot(work_root)
        }
        UltraProfile::Nextjs => profiles::nextjs::snapshot(work_root),
        UltraProfile::Python => simple_existing_paths(
            work_root,
            &["pyproject.toml", "requirements.txt", "src", "tests"],
        ),
        UltraProfile::Rust => simple_existing_paths(
            work_root,
            &["Cargo.toml", "src/lib.rs", "src/main.rs", "tests"],
        ),
        UltraProfile::Investigation | UltraProfile::Docs | UltraProfile::Generic => {
            ProfileSnapshot::new(Vec::new(), Vec::new())
        }
    }
}

pub(super) fn profile_snapshot_for_ultra_plan(
    work_root: &Path,
    ultra_plan: &UltraPlan,
) -> ProfileSnapshot {
    let requested_port = super::requested_port_for_ultra_plan(ultra_plan);
    let mut snapshot = profile_snapshot(work_root, ultra_plan.profile);
    if ultra_plan.profile == UltraProfile::Nextjs {
        snapshot.requested_port = requested_port;
        snapshot.probe_port = Some(profiles::nextjs::probe_port(work_root, requested_port));
    }
    snapshot
}

pub(super) fn build_profiled_phase_prompt(
    ultra_plan: &UltraPlan,
    phase: &UltraPhase,
    snapshot: &ProfileSnapshot,
    intent: WorkIntent,
) -> String {
    let mut snapshot_lines = snapshot.lines.clone();
    if ultra_plan.profile == UltraProfile::Nextjs {
        if text_tokens::contains_canvas_token(&ultra_plan.goal)
            || ultra_plan
                .phases
                .iter()
                .any(|phase| text_tokens::contains_canvas_token(&phase.prompt))
        {
            snapshot_lines.push(
                "Canvas surface requirement detected from goal or ultra-plan phases.".to_string(),
            );
        }
        if let Some(port) = snapshot.requested_port {
            snapshot_lines.push(format!(
                "Requested port invariant: keep package.json dev/start scripts on port {port}."
            ));
        }
        if let Some(port) = snapshot.probe_port {
            snapshot_lines.push(format!("Readiness/interaction probe target port: {port}."));
        }
    }
    let snapshot = if snapshot_lines.is_empty() {
        "- none detected".to_string()
    } else {
        snapshot_lines.join("\n")
    };
    let required_artifacts = required_artifact_contract_prompt(&ultra_plan.goal);
    format!(
        "Ultra goal:\n{goal}\n\n{required_artifacts}Ultra profile: {profile}\nUltra style: {style}\nDetected intent: {intent}\n\nCurrent phase id: {id}\nCurrent phase goal:\n{prompt}\n\nExisting workspace snapshot:\n{snapshot}\n\nProfile contract:\n{contract}\n\nRun only this phase. Preserve the profile contract. Prefer Read/Bash inspection before changing existing project structure. Use Write/Edit for file changes. End with deterministic checks when practical.",
        goal = ultra_plan.goal,
        required_artifacts = required_artifacts,
        profile = ultra_plan.profile,
        style = ultra_plan.style,
        intent = intent,
        id = phase.id,
        prompt = phase.prompt,
        snapshot = snapshot,
        contract = profile_runtime_contract(ultra_plan.profile, intent),
    )
}

#[cfg(test)]
pub(super) fn verify_profile_after_phase(
    work_root: &Path,
    profile: UltraProfile,
    intent: WorkIntent,
    before: &ProfileSnapshot,
) -> Result<(), String> {
    verify_profile_after_phase_with_requested_port(work_root, profile, intent, before, None)
}

pub(super) fn verify_profile_after_phase_with_requested_port(
    work_root: &Path,
    profile: UltraProfile,
    intent: WorkIntent,
    before: &ProfileSnapshot,
    requested_port: Option<u16>,
) -> Result<(), String> {
    let mut failures = Vec::new();
    match profile {
        UltraProfile::DataAnalysis | UltraProfile::DataPipeline => {
            profiles::data::verify(work_root, before, &mut failures);
        }
        UltraProfile::Nextjs => {
            profiles::nextjs::verify(work_root, intent, requested_port, &mut failures)
        }
        UltraProfile::Python
        | UltraProfile::Rust
        | UltraProfile::Investigation
        | UltraProfile::Docs
        | UltraProfile::Generic => {}
    }
    if failures.is_empty() {
        Ok(())
    } else {
        Err(failures.join("; "))
    }
}

fn profile_runtime_contract(profile: UltraProfile, intent: WorkIntent) -> &'static str {
    match profile {
        UltraProfile::Generic => {
            "- Generic profile: no capability contract is bound; behavioral verification is not run."
        }
        UltraProfile::Nextjs => profiles::nextjs::runtime_contract(intent),
        UltraProfile::Python => {
            "- Preserve the existing Python package/import layout.\n- Prefer pytest and python -m py_compile for verification.\n- Do not rewrite project metadata unless this phase explicitly requires it."
        }
        UltraProfile::Rust => {
            "- Preserve Cargo.toml and crate entrypoints.\n- Prefer cargo check/cargo test for verification.\n- Keep public behavior scoped to the requested phase."
        }
        UltraProfile::Investigation => {
            "- Produce a concrete report artifact.\n- Separate observed facts, hypotheses, and proposed next steps.\n- Do not modify source code unless the phase explicitly asks for fixes."
        }
        UltraProfile::Docs => {
            "- Produce or update documentation artifacts.\n- Avoid source-code changes unless explicitly requested.\n- Keep claims grounded in files inspected during this phase."
        }
        UltraProfile::DataAnalysis | UltraProfile::DataPipeline => {
            profiles::data::runtime_contract(profile)
        }
    }
}

fn simple_existing_paths(work_root: &Path, paths: &[&str]) -> ProfileSnapshot {
    let lines = paths
        .iter()
        .filter(|path| work_root.join(path).exists())
        .map(|path| format!("Existing path: {path}"))
        .collect();
    ProfileSnapshot::new(lines, Vec::new())
}
