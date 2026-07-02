use std::path::{Path, PathBuf};

use super::super::UltraProfile;
use super::super::profile::{ProfileSnapshot, ProtectedFile};

pub(in crate::agent::minimal_step_runner) fn generation_rules(
    profile: UltraProfile,
) -> &'static str {
    match profile {
        UltraProfile::DataAnalysis => {
            "- Profile data-analysis: treat input data as read-only. First inspect local files, schema, headers, row counts, missingness, and samples using scripts or shell. Produce reusable analysis scripts under scripts/ when needed and a human-readable report under reports/ or docs/. Do not require network access. Do not put raw data into prompts except small samples or summaries.\n"
        }
        UltraProfile::DataPipeline => {
            "- Profile data-pipeline: treat raw input data as read-only. Create reusable extraction/cleaning/validation scripts under scripts/ and processed outputs under data/processed/. Include checks for row counts, schema, missing values, and reproducibility. Do not require network access unless the user explicitly asks and grants it.\n"
        }
        _ => "",
    }
}

pub(in crate::agent::minimal_step_runner) fn runtime_contract(
    profile: UltraProfile,
) -> &'static str {
    match profile {
        UltraProfile::DataAnalysis => {
            "- Treat raw/input data files as read-only.\n- Do not paste full datasets into prompts; use schema, samples, counts, and summaries.\n- Put reusable analysis code under scripts/ when needed.\n- Put human-readable findings under reports/ or docs/.\n- Stay local-only unless the user explicitly requested network access."
        }
        UltraProfile::DataPipeline => {
            "- Treat raw/input data files as read-only.\n- Put reusable extraction/cleaning/validation scripts under scripts/.\n- Put processed outputs under data/processed/.\n- Include deterministic checks for row counts, schema, missing values, or reproducibility.\n- Stay local-only unless the user explicitly requested network access."
        }
        _ => "- Treat raw/input data files as read-only.",
    }
}

pub(in crate::agent::minimal_step_runner) fn snapshot(work_root: &Path) -> ProfileSnapshot {
    let mut lines = Vec::new();
    let mut protected_files = Vec::new();
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
    ProfileSnapshot {
        lines,
        protected_files,
    }
}

pub(in crate::agent::minimal_step_runner) fn verify(
    work_root: &Path,
    before: &ProfileSnapshot,
    failures: &mut Vec<String>,
) {
    for protected in &before.protected_files {
        let path = work_root.join(&protected.path);
        match std::fs::metadata(&path) {
            Ok(meta) if meta.len() == protected.len => {}
            Ok(meta) => failures.push(format!(
                "contract_violation: protected input data changed: {} ({} -> {} bytes)",
                protected.path,
                protected.len,
                meta.len()
            )),
            Err(_) => failures.push(format!(
                "contract_violation: protected input data missing: {}",
                protected.path
            )),
        }
    }
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
