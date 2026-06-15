use std::path::{Path, PathBuf};

use super::{UltraPhase, UltraPlan, UltraProfile, required_artifact_contract_prompt};

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

pub(super) fn profile_generation_rules(profile: UltraProfile) -> &'static str {
    match profile {
        UltraProfile::Generic => "",
        UltraProfile::Nextjs => {
            "- Profile nextjs: preserve the existing Next.js structure when present. Keep package.json as a Next.js package, keep app/ or pages/ entrypoints, and end with a build verification phase.\n"
        }
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
        UltraProfile::DataAnalysis => {
            "- Profile data-analysis: treat input data as read-only. First inspect local files, schema, headers, row counts, missingness, and samples using scripts or shell. Produce reusable analysis scripts under scripts/ when needed and a human-readable report under reports/ or docs/. Do not require network access. Do not put raw data into prompts except small samples or summaries.\n"
        }
        UltraProfile::DataPipeline => {
            "- Profile data-pipeline: treat raw input data as read-only. Create reusable extraction/cleaning/validation scripts under scripts/ and processed outputs under data/processed/. Include checks for row counts, schema, missing values, and reproducibility. Do not require network access unless the user explicitly asks and grants it.\n"
        }
    }
}

pub(super) fn profile_snapshot(work_root: &Path, profile: UltraProfile) -> ProfileSnapshot {
    let mut lines = Vec::new();
    let mut protected_files = Vec::new();
    match profile {
        UltraProfile::DataAnalysis | UltraProfile::DataPipeline => {
            let data_files = discover_data_files(work_root);
            if data_files.is_empty() {
                lines.push("No local data files were detected yet.".to_string());
            } else {
                lines.push("Detected local data files:".to_string());
                for path in data_files.iter().take(12) {
                    let full_path = work_root.join(path);
                    let len = std::fs::metadata(&full_path).map(|m| m.len()).unwrap_or(0);
                    lines.push(format!("- {} ({} bytes)", path.display(), len));
                    if let Some(header) = data_file_header(&full_path) {
                        lines.push(format!("  header/sample: {header}"));
                    }
                    if is_protected_data_input(path) {
                        protected_files.push(ProtectedFile {
                            path: path.to_string_lossy().to_string(),
                            len,
                        });
                    }
                }
            }
            for dir in ["data/raw", "data/processed", "scripts", "reports", "docs"] {
                if work_root.join(dir).exists() {
                    lines.push(format!("Existing directory: {dir}"));
                }
            }
        }
        UltraProfile::Nextjs => {
            for path in [
                "package.json",
                "tsconfig.json",
                "app/page.tsx",
                "app/layout.tsx",
                "pages/index.tsx",
                "next.config.js",
            ] {
                if work_root.join(path).exists() {
                    lines.push(format!("Existing file: {path}"));
                }
            }
            if let Some(summary) = package_json_summary(&work_root.join("package.json")) {
                lines.push(summary);
            }
        }
        UltraProfile::Python => {
            for path in ["pyproject.toml", "requirements.txt", "src", "tests"] {
                if work_root.join(path).exists() {
                    lines.push(format!("Existing path: {path}"));
                }
            }
        }
        UltraProfile::Rust => {
            for path in ["Cargo.toml", "src/lib.rs", "src/main.rs", "tests"] {
                if work_root.join(path).exists() {
                    lines.push(format!("Existing path: {path}"));
                }
            }
        }
        UltraProfile::Investigation | UltraProfile::Docs | UltraProfile::Generic => {}
    }
    ProfileSnapshot {
        lines,
        protected_files,
    }
}

pub(super) fn build_profiled_phase_prompt(
    ultra_plan: &UltraPlan,
    phase: &UltraPhase,
    snapshot: &ProfileSnapshot,
) -> String {
    let snapshot = if snapshot.lines.is_empty() {
        "- none detected".to_string()
    } else {
        snapshot.lines.join("\n")
    };
    let required_artifacts = required_artifact_contract_prompt(&ultra_plan.goal);
    format!(
        "Ultra goal:\n{goal}\n\n{required_artifacts}Ultra profile: {profile}\nUltra style: {style}\n\nCurrent phase id: {id}\nCurrent phase goal:\n{prompt}\n\nExisting workspace snapshot:\n{snapshot}\n\nProfile contract:\n{contract}\n\nRun only this phase. Preserve the profile contract. Prefer Read/Bash inspection before changing existing project structure. Use Write/Edit for file changes. End with deterministic checks when practical.",
        goal = ultra_plan.goal,
        required_artifacts = required_artifacts,
        profile = ultra_plan.profile,
        style = ultra_plan.style,
        id = phase.id,
        prompt = phase.prompt,
        snapshot = snapshot,
        contract = profile_runtime_contract(ultra_plan.profile),
    )
}

pub(super) fn verify_profile_after_phase(
    work_root: &Path,
    profile: UltraProfile,
    before: &ProfileSnapshot,
) -> Result<(), String> {
    let mut failures = Vec::new();
    if matches!(
        profile,
        UltraProfile::DataAnalysis | UltraProfile::DataPipeline
    ) {
        for protected in &before.protected_files {
            let path = work_root.join(&protected.path);
            match std::fs::metadata(&path) {
                Ok(meta) if meta.len() == protected.len => {}
                Ok(meta) => failures.push(format!(
                    "protected input data changed: {} ({} -> {} bytes)",
                    protected.path,
                    protected.len,
                    meta.len()
                )),
                Err(_) => {
                    failures.push(format!("protected input data missing: {}", protected.path))
                }
            }
        }
    }
    if profile == UltraProfile::Nextjs {
        verify_nextjs_profile(work_root, &mut failures);
    }
    if failures.is_empty() {
        Ok(())
    } else {
        Err(failures.join("; "))
    }
}

fn profile_runtime_contract(profile: UltraProfile) -> &'static str {
    match profile {
        UltraProfile::Generic => "- Keep changes scoped to the current phase.",
        UltraProfile::Nextjs => {
            "- Preserve the workspace as a Next.js app when one exists.\n- Do not convert package.json to a standalone TypeScript/Node project.\n- Keep next/react/react-dom dependencies when already present.\n- Keep scripts.build as next build when already present.\n- If a 3011 port requirement exists, keep the dev script on port 3011.\n- Do not set tsconfig rootDir to ./src in a way that excludes app/.\n- If source imports use @/* aliases, tsconfig.json must set compilerOptions.baseUrl to \".\" and map @/* under compilerOptions.paths; otherwise use relative imports."
        }
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
        UltraProfile::DataAnalysis => {
            "- Treat raw/input data files as read-only.\n- Do not paste full datasets into prompts; use schema, samples, counts, and summaries.\n- Put reusable analysis code under scripts/ when needed.\n- Put human-readable findings under reports/ or docs/.\n- Stay local-only unless the user explicitly requested network access."
        }
        UltraProfile::DataPipeline => {
            "- Treat raw/input data files as read-only.\n- Put reusable extraction/cleaning/validation scripts under scripts/.\n- Put processed outputs under data/processed/.\n- Include deterministic checks for row counts, schema, missing values, or reproducibility.\n- Stay local-only unless the user explicitly requested network access."
        }
    }
}

fn verify_nextjs_profile(work_root: &Path, failures: &mut Vec<String>) {
    let package_path = work_root.join("package.json");
    let app_dir_exists = work_root.join("app").is_dir() || work_root.join("pages").is_dir();
    if package_path.exists() && app_dir_exists {
        let Ok(raw) = std::fs::read_to_string(&package_path) else {
            failures.push("package.json exists but could not be read".to_string());
            return;
        };
        if raw.contains("\"next\"") {
            if let Ok(json) = serde_json::from_str::<serde_json::Value>(&raw) {
                let build = json
                    .pointer("/scripts/build")
                    .and_then(|v| v.as_str())
                    .unwrap_or("");
                if !build.is_empty() && build != "next build" {
                    failures.push(format!(
                        "package.json build script is no longer `next build`: {build}"
                    ));
                }
                let dev = json
                    .pointer("/scripts/dev")
                    .and_then(|v| v.as_str())
                    .unwrap_or("");
                if dev.contains("3011") && !dev.contains("next dev") {
                    failures.push(format!(
                        "package.json dev script mentions 3011 but is not next dev: {dev}"
                    ));
                }
            }
        } else {
            failures.push("package.json no longer contains next dependency".to_string());
        }
    }
    let tsconfig_path = work_root.join("tsconfig.json");
    if tsconfig_path.exists()
        && app_dir_exists
        && let Ok(raw) = std::fs::read_to_string(tsconfig_path)
    {
        if raw.contains("\"rootDir\"") && raw.contains("\"./src\"") {
            failures.push("tsconfig.json rootDir ./src excludes Next.js app/ files".to_string());
        }
        if nextjs_source_uses_at_alias(work_root) && nextjs_tsconfig_missing_at_alias_base_url(&raw)
        {
            failures.push(
                "Next.js source imports @/* aliases but tsconfig.json lacks compilerOptions.baseUrl \".\""
                    .to_string(),
            );
        }
    }
}

fn nextjs_tsconfig_missing_at_alias_base_url(raw: &str) -> bool {
    let Ok(json) = serde_json::from_str::<serde_json::Value>(raw) else {
        return true;
    };
    let compiler = json
        .get("compilerOptions")
        .and_then(|value| value.as_object());
    let has_base_url = compiler
        .and_then(|map| map.get("baseUrl"))
        .and_then(|value| value.as_str())
        == Some(".");
    let has_alias = compiler
        .and_then(|map| map.get("paths"))
        .and_then(|value| value.get("@/*"))
        .and_then(|value| value.as_array())
        .is_some_and(|items| {
            items
                .iter()
                .filter_map(|item| item.as_str())
                .any(|item| item == "./*" || item == "*")
        });
    !(has_base_url && has_alias)
}

fn nextjs_source_uses_at_alias(work_root: &Path) -> bool {
    for relative in discover_source_files(work_root, work_root, 0) {
        let path = work_root.join(relative);
        if let Ok(raw) = std::fs::read_to_string(path)
            && (raw.contains("from '@/") || raw.contains("from \"@/"))
        {
            return true;
        }
    }
    false
}

fn discover_source_files(root: &Path, current: &Path, depth: usize) -> Vec<PathBuf> {
    if depth > 4 {
        return Vec::new();
    }
    let Ok(entries) = std::fs::read_dir(current) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if name.starts_with('.') || name == "node_modules" || name == "target" {
            continue;
        }
        if path.is_dir() {
            out.extend(discover_source_files(root, &path, depth + 1));
        } else if is_source_file(&path)
            && let Ok(relative) = path.strip_prefix(root)
        {
            out.push(relative.to_path_buf());
        }
    }
    out
}

fn is_source_file(path: &Path) -> bool {
    let Some(ext) = path.extension().and_then(|value| value.to_str()) else {
        return false;
    };
    matches!(
        ext.to_ascii_lowercase().as_str(),
        "ts" | "tsx" | "js" | "jsx" | "mjs" | "cjs"
    )
}

fn discover_data_files(work_root: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    discover_data_files_inner(work_root, work_root, 0, &mut out);
    out.sort();
    out
}

fn discover_data_files_inner(root: &Path, current: &Path, depth: usize, out: &mut Vec<PathBuf>) {
    if depth > 4 || out.len() >= 48 {
        return;
    }
    let Ok(entries) = std::fs::read_dir(current) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if name.starts_with('.') || name == "node_modules" || name == "target" {
            continue;
        }
        if path.is_dir() {
            discover_data_files_inner(root, &path, depth + 1, out);
        } else if is_data_file(&path)
            && let Ok(relative) = path.strip_prefix(root)
        {
            out.push(relative.to_path_buf());
        }
    }
}

fn is_data_file(path: &Path) -> bool {
    let Some(ext) = path.extension().and_then(|value| value.to_str()) else {
        return false;
    };
    matches!(
        ext.to_ascii_lowercase().as_str(),
        "csv" | "tsv" | "json" | "jsonl" | "ndjson" | "parquet" | "xlsx" | "xls" | "sqlite" | "db"
    )
}

fn is_protected_data_input(path: &Path) -> bool {
    let first = path
        .components()
        .next()
        .and_then(|component| match component {
            std::path::Component::Normal(value) => value.to_str(),
            _ => None,
        });
    if first == Some("data") {
        let second = path
            .components()
            .nth(1)
            .and_then(|component| match component {
                std::path::Component::Normal(value) => value.to_str(),
                _ => None,
            });
        return second != Some("processed");
    }
    is_data_file(path)
}

fn data_file_header(path: &Path) -> Option<String> {
    let ext = path.extension()?.to_str()?.to_ascii_lowercase();
    if !matches!(ext.as_str(), "csv" | "tsv" | "json" | "jsonl" | "ndjson") {
        return None;
    }
    let raw = std::fs::read_to_string(path).ok()?;
    raw.lines()
        .next()
        .map(|line| line.chars().take(180).collect::<String>())
}

fn package_json_summary(path: &Path) -> Option<String> {
    let raw = std::fs::read_to_string(path).ok()?;
    let json = serde_json::from_str::<serde_json::Value>(&raw).ok()?;
    let build = json
        .pointer("/scripts/build")
        .and_then(|value| value.as_str())
        .unwrap_or("none");
    let dev = json
        .pointer("/scripts/dev")
        .and_then(|value| value.as_str())
        .unwrap_or("none");
    let deps = json
        .get("dependencies")
        .and_then(|value| value.as_object())
        .map(|map| map.keys().take(8).cloned().collect::<Vec<_>>().join(", "))
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| "none".to_string());
    Some(format!(
        "package.json scripts: build=`{build}`, dev=`{dev}`; dependencies: {deps}"
    ))
}
