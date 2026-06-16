use std::path::Path;

use super::{
    UltraPhase, UltraPlan, UltraProfile, WorkIntent, profiles, required_artifact_contract_prompt,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct ProfileSnapshot {
    pub(super) lines: Vec<String>,
    pub(super) protected_files: Vec<ProtectedFile>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct ProtectedFile {
    pub(super) path: String,
    pub(super) len: u64,
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
            ProfileSnapshot {
                lines: Vec::new(),
                protected_files: Vec::new(),
            }
        }
    }
}

pub(super) fn build_profiled_phase_prompt(
    ultra_plan: &UltraPlan,
    phase: &UltraPhase,
    snapshot: &ProfileSnapshot,
    intent: WorkIntent,
) -> String {
    let snapshot = if snapshot.lines.is_empty() {
        "- none detected".to_string()
    } else {
        snapshot.lines.join("\n")
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

pub(super) fn verify_profile_after_phase(
    work_root: &Path,
    profile: UltraProfile,
    intent: WorkIntent,
    before: &ProfileSnapshot,
) -> Result<(), String> {
    let mut failures = Vec::new();
    match profile {
        UltraProfile::DataAnalysis | UltraProfile::DataPipeline => {
            profiles::data::verify(work_root, before, &mut failures);
        }
        UltraProfile::Nextjs => profiles::nextjs::verify(work_root, intent, &mut failures),
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
        UltraProfile::Generic => "- Keep changes scoped to the current phase.",
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
    ProfileSnapshot {
        lines,
        protected_files: Vec::new(),
    }
}
