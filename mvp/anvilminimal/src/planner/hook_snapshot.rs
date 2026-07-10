use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use regex::Regex;
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::config::Config;
use crate::eval_events;
use crate::planner::profile::{ProfileHookAttribute, profile_hook_snapshot_targets};
use crate::planner::verify::VerificationReport;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HookSnapshotSaveReport {
    pub saved_paths: Vec<String>,
    pub skipped_paths: Vec<String>,
    pub failures: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HookSnapshotDiagnostic {
    pub path: String,
    pub snapshot_path: String,
    pub snapshot_phase: String,
    pub missing_attributes: Vec<String>,
    pub diff: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HookSnapshotRestore {
    pub restored_path: String,
    pub snapshot_path: String,
    pub snapshot_phase: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct HookSnapshotManifest {
    entries: BTreeMap<String, HookSnapshotManifestEntry>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct HookSnapshotManifestEntry {
    snapshot_phase: String,
    snapshot_path: String,
    attributes: Vec<ProfileHookAttribute>,
}

pub fn save_phase_snapshots(
    config: &Config,
    profile: &str,
    goal: &str,
    phase_id: &str,
) -> HookSnapshotSaveReport {
    let report = save_phase_snapshots_at(
        &config.workspace_root,
        config.eval_events_path.as_deref(),
        profile,
        goal,
        phase_id,
    );
    if !report.saved_paths.is_empty() || !report.failures.is_empty() {
        eval_events::emit(
            config.eval_events_path.as_deref(),
            json!({
                "event": "hook_snapshot_saved",
                "phase_id": phase_id,
                "snapshot_phase": phase_id,
                "saved_paths": report.saved_paths.clone(),
                "skipped_paths": report.skipped_paths.clone(),
                "snapshot_failures": report.failures.clone(),
            }),
        );
    }
    report
}

pub fn report_missing_as_profile_failure(
    config: &Config,
    profile: &str,
    goal: &str,
    report: VerificationReport,
) -> VerificationReport {
    if !report.is_pass() {
        return report;
    }
    let Some(diagnostic) = detect_missing(config, profile, goal) else {
        return report;
    };
    VerificationReport::profile_failed(profile_failure_reason(&diagnostic))
}

pub fn detect_missing(
    config: &Config,
    profile: &str,
    goal: &str,
) -> Option<HookSnapshotDiagnostic> {
    detect_missing_at(
        &config.workspace_root,
        config.eval_events_path.as_deref(),
        profile,
        goal,
    )
}

pub fn deterministic_feedback(diagnostic: &HookSnapshotDiagnostic) -> String {
    format!(
        "Hook snapshot regression detected before LLM repair:\n\
- path: {}\n\
- missing attributes: {}\n\
- last-known-good: these attributes existed after phase `{}` and are stored at `{}`\n\
- action: restore or re-add these exact data-anvil hooks before changing behavior.\n\n\
Unified diff near missing hooks:\n```diff\n{}\n```",
        diagnostic.path,
        diagnostic.missing_attributes.join(", "),
        diagnostic.snapshot_phase,
        diagnostic.snapshot_path,
        diagnostic.diff
    )
}

pub fn emit_feedback(
    config: &Config,
    lifecycle_stage: &str,
    scope_id: Option<&str>,
    diagnostic: &HookSnapshotDiagnostic,
) {
    eval_events::emit(
        config.eval_events_path.as_deref(),
        json!({
            "event": "hook_snapshot_feedback",
            "lifecycle_stage": lifecycle_stage,
            "scope_id": scope_id.unwrap_or_default(),
            "path": diagnostic.path,
            "snapshot_path": diagnostic.snapshot_path,
            "snapshot_phase": diagnostic.snapshot_phase,
            "missing_attributes": diagnostic.missing_attributes,
            "diff": eval_events::body_snippet(&diagnostic.diff),
        }),
    );
}

pub fn prefix_feedback_if_missing(
    config: &Config,
    profile: &str,
    goal: &str,
    lifecycle_stage: &str,
    scope_id: Option<&str>,
    feedback_given: &mut bool,
    prompt: String,
) -> String {
    if *feedback_given {
        return prompt;
    }
    let Some(diagnostic) = detect_missing(config, profile, goal) else {
        return prompt;
    };
    *feedback_given = true;
    emit_feedback(config, lifecycle_stage, scope_id, &diagnostic);
    format!("{}\n\n{}", deterministic_feedback(&diagnostic), prompt)
}

pub fn restore_first_missing(
    config: &Config,
    profile: &str,
    goal: &str,
) -> anyhow::Result<Option<HookSnapshotRestore>> {
    let Some(diagnostic) = detect_missing(config, profile, goal) else {
        return Ok(None);
    };
    let Some(rel) = safe_source_rel_path(&diagnostic.path) else {
        return Ok(None);
    };
    let Some(snapshot_root) =
        snapshot_root(&config.workspace_root, config.eval_events_path.as_deref())
    else {
        return Ok(None);
    };
    let source = snapshot_root.join(&rel);
    if !source.is_file() {
        return Ok(None);
    }
    let destination = config.workspace_root.join(&rel);
    if let Some(parent) = destination.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::copy(&source, &destination)?;
    let restored = HookSnapshotRestore {
        restored_path: rel,
        snapshot_path: diagnostic.snapshot_path,
        snapshot_phase: diagnostic.snapshot_phase,
    };
    eval_events::emit(
        config.eval_events_path.as_deref(),
        json!({
            "event": "hook_snapshot_restored",
            "restored_path": restored.restored_path,
            "snapshot_path": restored.snapshot_path,
            "snapshot_phase": restored.snapshot_phase,
        }),
    );
    Ok(Some(restored))
}

pub fn current_missing_profile_failure_reason(
    config: &Config,
    profile: &str,
    goal: &str,
) -> Option<String> {
    detect_missing(config, profile, goal).map(|diagnostic| profile_failure_reason(&diagnostic))
}

fn save_phase_snapshots_at(
    root: &Path,
    eval_events_path: Option<&Path>,
    profile: &str,
    goal: &str,
    phase_id: &str,
) -> HookSnapshotSaveReport {
    let Some(snapshot_root) = snapshot_root(root, eval_events_path) else {
        return HookSnapshotSaveReport {
            saved_paths: Vec::new(),
            skipped_paths: Vec::new(),
            failures: vec!["snapshot_run_id_unavailable".to_string()],
        };
    };
    let mut manifest = read_manifest(&snapshot_root);
    let mut saved_paths = Vec::new();
    let mut skipped_paths = Vec::new();
    let mut failures = Vec::new();
    for target in profile_hook_snapshot_targets(root, profile, goal) {
        let Some(rel) = safe_source_rel_path(&target.relative_path) else {
            continue;
        };
        let source = root.join(&rel);
        let Ok(content) = std::fs::read_to_string(&source) else {
            skipped_paths.push(rel);
            continue;
        };
        if !hook_attributes_present(&content, &target.required_attributes) {
            skipped_paths.push(rel);
            continue;
        }
        let destination = snapshot_root.join(&rel);
        if let Some(parent) = destination.parent()
            && let Err(err) = std::fs::create_dir_all(parent)
        {
            failures.push(format!("{rel}: create snapshot dir failed: {err}"));
            continue;
        }
        match std::fs::copy(&source, &destination) {
            Ok(_) => {
                saved_paths.push(rel.clone());
                manifest.entries.insert(
                    rel.clone(),
                    HookSnapshotManifestEntry {
                        snapshot_phase: phase_id.to_string(),
                        snapshot_path: display_snapshot_path(root, &destination),
                        attributes: target.required_attributes,
                    },
                );
            }
            Err(err) => failures.push(format!("{rel}: snapshot copy failed: {err}")),
        }
    }
    if !saved_paths.is_empty()
        && let Err(err) = write_manifest(&snapshot_root, &manifest)
    {
        failures.push(format!("manifest write failed: {err}"));
    }
    HookSnapshotSaveReport {
        saved_paths,
        skipped_paths,
        failures,
    }
}

fn detect_missing_at(
    root: &Path,
    eval_events_path: Option<&Path>,
    profile: &str,
    goal: &str,
) -> Option<HookSnapshotDiagnostic> {
    let snapshot_root = snapshot_root(root, eval_events_path)?;
    let manifest = read_manifest(&snapshot_root);
    for target in profile_hook_snapshot_targets(root, profile, goal) {
        let rel = safe_source_rel_path(&target.relative_path)?;
        let entry = manifest.entries.get(&rel);
        let snapshot_path = snapshot_root.join(&rel);
        if !snapshot_path.is_file() {
            continue;
        }
        let snapshot = std::fs::read_to_string(&snapshot_path).ok()?;
        let current = std::fs::read_to_string(root.join(&rel)).unwrap_or_default();
        let attrs = entry
            .map(|entry| entry.attributes.clone())
            .unwrap_or_else(|| target.required_attributes.clone());
        let missing = attrs
            .iter()
            .copied()
            .filter(|attr| hook_attribute_present(&snapshot, *attr))
            .filter(|attr| !hook_attribute_present(&current, *attr))
            .map(ProfileHookAttribute::display)
            .map(str::to_string)
            .collect::<Vec<_>>();
        if missing.is_empty() {
            continue;
        }
        return Some(HookSnapshotDiagnostic {
            path: rel,
            snapshot_path: entry
                .map(|entry| entry.snapshot_path.clone())
                .unwrap_or_else(|| display_snapshot_path(root, &snapshot_path)),
            snapshot_phase: entry
                .map(|entry| entry.snapshot_phase.clone())
                .filter(|phase| !phase.is_empty())
                .unwrap_or_else(|| "unknown".to_string()),
            diff: unified_diff_near_missing(&snapshot, &current, &missing),
            missing_attributes: missing,
        });
    }
    None
}

fn profile_failure_reason(diagnostic: &HookSnapshotDiagnostic) -> String {
    format!(
        "hook_snapshot_regression:{} missing {}; last-known-good phase {}",
        diagnostic.path,
        diagnostic.missing_attributes.join(","),
        diagnostic.snapshot_phase
    )
}

fn hook_attributes_present(source: &str, attributes: &[ProfileHookAttribute]) -> bool {
    attributes
        .iter()
        .all(|attribute| hook_attribute_present(source, *attribute))
}

fn hook_attribute_present(source: &str, attribute: ProfileHookAttribute) -> bool {
    match attribute {
        ProfileHookAttribute::PrimaryAction => data_anvil_action_present(source, "primary"),
        ProfileHookAttribute::RestartAction => data_anvil_action_present(source, "restart"),
        ProfileHookAttribute::State => Regex::new(r#"(?s)data-anvil-state(?:\s*=|\b)"#)
            .is_ok_and(|regex| regex.is_match(source)),
    }
}

fn data_anvil_action_present(source: &str, value: &str) -> bool {
    let pattern = format!(
        r#"(?s)data-anvil-action\s*=\s*(?:"{0}"|'{0}'|\{{\s*(?:"{0}"|'{0}'|`{0}`)\s*\}})"#,
        regex::escape(value)
    );
    Regex::new(&pattern).is_ok_and(|regex| regex.is_match(source))
}

fn unified_diff_near_missing(
    snapshot: &str,
    current: &str,
    missing_attributes: &[String],
) -> String {
    let old_lines = snapshot.lines().collect::<Vec<_>>();
    let new_lines = current.lines().collect::<Vec<_>>();
    let center = old_lines
        .iter()
        .position(|line| missing_attributes.iter().any(|attr| line.contains(attr)))
        .or_else(|| first_different_line(&old_lines, &new_lines))
        .unwrap_or(0);
    let start = center.saturating_sub(2);
    let end = (center + 3).min(old_lines.len().max(new_lines.len()));
    let old_count = end
        .saturating_sub(start)
        .min(old_lines.len().saturating_sub(start));
    let new_count = end
        .saturating_sub(start)
        .min(new_lines.len().saturating_sub(start));
    let mut out = format!(
        "--- last-known-good\n+++ current\n@@ -{},{} +{},{} @@\n",
        start + 1,
        old_count,
        start + 1,
        new_count
    );
    for index in start..end {
        let old = old_lines.get(index).copied();
        let new = new_lines.get(index).copied();
        match (old, new) {
            (Some(old), Some(new)) if old == new => {
                out.push(' ');
                out.push_str(old);
                out.push('\n');
            }
            (Some(old), Some(new)) => {
                out.push('-');
                out.push_str(old);
                out.push('\n');
                out.push('+');
                out.push_str(new);
                out.push('\n');
            }
            (Some(old), None) => {
                out.push('-');
                out.push_str(old);
                out.push('\n');
            }
            (None, Some(new)) => {
                out.push('+');
                out.push_str(new);
                out.push('\n');
            }
            (None, None) => {}
        }
    }
    out
}

fn first_different_line(old_lines: &[&str], new_lines: &[&str]) -> Option<usize> {
    let end = old_lines.len().max(new_lines.len());
    (0..end).find(|index| old_lines.get(*index) != new_lines.get(*index))
}

fn snapshot_root(root: &Path, eval_events_path: Option<&Path>) -> Option<PathBuf> {
    let run_id = run_id_from_events_path(eval_events_path?)?;
    Some(root.join(".anvil").join("snapshots").join(run_id))
}

fn run_id_from_events_path(path: &Path) -> Option<String> {
    let run_dir = path.parent()?;
    let is_run_dir = run_dir
        .parent()
        .and_then(Path::file_name)
        .and_then(|value| value.to_str())
        .is_some_and(|value| value == "runs");
    if !is_run_dir {
        return None;
    }
    run_dir
        .file_name()
        .and_then(|value| value.to_str())
        .filter(|value| !value.trim().is_empty())
        .map(str::to_string)
}

fn manifest_path(snapshot_root: &Path) -> PathBuf {
    snapshot_root.join("manifest.json")
}

fn read_manifest(snapshot_root: &Path) -> HookSnapshotManifest {
    std::fs::read_to_string(manifest_path(snapshot_root))
        .ok()
        .and_then(|text| serde_json::from_str(&text).ok())
        .unwrap_or_default()
}

fn write_manifest(snapshot_root: &Path, manifest: &HookSnapshotManifest) -> anyhow::Result<()> {
    std::fs::create_dir_all(snapshot_root)?;
    let text = serde_json::to_string_pretty(manifest)?;
    std::fs::write(manifest_path(snapshot_root), text)?;
    Ok(())
}

fn display_snapshot_path(root: &Path, path: &Path) -> String {
    path.strip_prefix(root)
        .map(|path| path.display().to_string())
        .unwrap_or_else(|_| path.display().to_string())
        .replace('\\', "/")
}

fn safe_source_rel_path(raw: &str) -> Option<String> {
    let rel = raw.trim().trim_start_matches("./").replace('\\', "/");
    if rel.is_empty() || rel.starts_with('/') || rel.contains('\0') {
        return None;
    }
    let path = Path::new(&rel);
    if !path
        .components()
        .all(|component| matches!(component, std::path::Component::Normal(_)))
    {
        return None;
    }
    let lower = rel.to_ascii_lowercase();
    if lower == ".anvil"
        || lower.starts_with(".anvil/")
        || lower.contains("/.anvil/")
        || lower == "node_modules"
        || lower.starts_with("node_modules/")
        || lower.contains("/node_modules/")
    {
        return None;
    }
    let ext = path.extension().and_then(|ext| ext.to_str())?;
    matches!(ext, "tsx" | "ts" | "jsx" | "js" | "mjs" | "cjs").then_some(rel)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{Action, Config, ConfigFieldSources, Provider};
    use crate::config::{NarrationMode, PromptLayout};
    use crate::planner::profile::ProfileInference;

    fn config(root: &Path) -> Config {
        Config {
            workspace_root: root.to_path_buf(),
            state_dir: root.join(".anvil/state"),
            eval_events_path: Some(root.join(".anvil/runs/test-run/events.jsonl")),
            completion_contract_path: None,
            yes: true,
            offline: true,
            context_budget: 12000,
            model: "test".to_string(),
            provider: Provider::Ollama,
            prompt_layout: PromptLayout::Stable,
            planner_model: "test".to_string(),
            planner_provider: Provider::Ollama,
            ollama_host: String::new(),
            num_predict: 0,
            max_iterations: 1,
            chat_timeout_secs: 1,
            chat_timeout_source: "test".to_string(),
            field_sources: ConfigFieldSources::default(),
            chat_retries: 0,
            resume: None,
            fresh_session: true,
            no_footer: true,
            narration: NarrationMode::Quiet,
            profile: "nextjs".to_string(),
            profile_explicit: true,
            profile_inference: Some(ProfileInference {
                profile: "nextjs",
                source: crate::planner::profile::ProfileInferenceSource::Goal,
            }),
            style: String::new(),
            action: Action::Repl,
        }
    }

    fn write_nextjs_app(root: &Path, page: &str) {
        std::fs::create_dir_all(root.join("src/app")).unwrap();
        std::fs::write(
            root.join("package.json"),
            r#"{"dependencies":{"next":"14.2.0","react":"18.3.0","react-dom":"18.3.0"},"scripts":{"dev":"next dev -p 3011","build":"next build"}}"#,
        )
        .unwrap();
        std::fs::write(root.join("src/app/page.tsx"), page).unwrap();
    }

    fn good_page() -> &'static str {
        r#"export default function Page() {
  const restart = () => {};
  return <main data-anvil-state={JSON.stringify({ score: 0 })}>
    <button data-anvil-action="primary">Start</button>
    <button data-anvil-action="restart" onClick={restart}>Restart</button>
  </main>;
}
"#
    }

    #[test]
    fn snapshot_saved_after_hook_verified_phase() {
        let dir = tempfile::tempdir().unwrap();
        write_nextjs_app(dir.path(), good_page());
        let cfg = config(dir.path());

        let report = save_phase_snapshots(&cfg, "nextjs", "game", "phase-one");

        assert_eq!(report.saved_paths, vec!["src/app/page.tsx"]);
        assert!(
            dir.path()
                .join(".anvil/snapshots/test-run/src/app/page.tsx")
                .is_file()
        );
    }

    #[test]
    fn missing_primary_feedback_names_attribute_and_diff() {
        let dir = tempfile::tempdir().unwrap();
        write_nextjs_app(dir.path(), good_page());
        let cfg = config(dir.path());
        save_phase_snapshots(&cfg, "nextjs", "game", "phase-one");
        std::fs::write(
            dir.path().join("src/app/page.tsx"),
            good_page().replace(r#" data-anvil-action="primary""#, ""),
        )
        .unwrap();

        let diagnostic = detect_missing(&cfg, "nextjs", "game").unwrap();
        let feedback = deterministic_feedback(&diagnostic);

        assert_eq!(
            diagnostic.missing_attributes,
            vec![r#"data-anvil-action="primary""#]
        );
        assert!(feedback.contains("last-known-good"), "{feedback}");
        assert!(
            feedback.contains(r#"data-anvil-action="primary""#),
            "{feedback}"
        );
        assert!(feedback.contains("--- last-known-good"), "{feedback}");
        assert!(feedback.contains("+++ current"), "{feedback}");
        assert!(feedback.contains("@@"), "{feedback}");
    }

    #[test]
    fn restore_after_feedback_reinstates_primary_hook() {
        let dir = tempfile::tempdir().unwrap();
        write_nextjs_app(dir.path(), good_page());
        let cfg = config(dir.path());
        save_phase_snapshots(&cfg, "nextjs", "game", "phase-one");
        std::fs::write(
            dir.path().join("src/app/page.tsx"),
            good_page().replace(r#" data-anvil-action="primary""#, ""),
        )
        .unwrap();
        let diagnostic = detect_missing(&cfg, "nextjs", "game").unwrap();
        emit_feedback(
            &cfg,
            "step_verify_repair",
            Some("verify-primary"),
            &diagnostic,
        );

        let restored = restore_first_missing(&cfg, "nextjs", "game")
            .unwrap()
            .unwrap();
        let restored_source = std::fs::read_to_string(dir.path().join("src/app/page.tsx")).unwrap();
        let events =
            std::fs::read_to_string(dir.path().join(".anvil/runs/test-run/events.jsonl")).unwrap();

        assert_eq!(restored.restored_path, "src/app/page.tsx");
        assert!(restored_source.contains(r#"data-anvil-action="primary""#));
        assert!(events.contains(r#""event":"hook_snapshot_restored""#));
        assert!(events.contains(r#""snapshot_phase":"phase-one""#));
    }

    #[test]
    fn file_that_has_not_passed_hook_verification_is_not_snapshotted() {
        let dir = tempfile::tempdir().unwrap();
        write_nextjs_app(
            dir.path(),
            &good_page().replace(
                r#"<button data-anvil-action="restart" onClick={restart}>Restart</button>"#,
                "",
            ),
        );
        let cfg = config(dir.path());

        let report = save_phase_snapshots(&cfg, "nextjs", "game", "phase-one");

        assert!(report.saved_paths.is_empty());
        assert_eq!(report.skipped_paths, vec!["src/app/page.tsx"]);
        assert!(
            !dir.path()
                .join(".anvil/snapshots/test-run/src/app/page.tsx")
                .exists()
        );
    }
}
