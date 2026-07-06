use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

use serde_json::Value;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RunInventoryItem {
    pub id: String,
    pub short_id: String,
    pub started_at: String,
    pub status: String,
    pub assurance: String,
    pub completed_phases: Option<usize>,
    pub total_phases: Option<usize>,
    pub stop_reason: String,
    pub recovery: String,
    pub recovery_ultra_plan_path: String,
    pub events_path: PathBuf,
}

pub fn recent_runs(root: &Path, cap: usize) -> Vec<RunInventoryItem> {
    let runs_dir = root.join(".anvil").join("runs");
    let Ok(entries) = std::fs::read_dir(&runs_dir) else {
        return Vec::new();
    };
    let mut candidates = entries
        .filter_map(Result::ok)
        .filter_map(|entry| run_inventory_item(root, &entry.path()))
        .collect::<Vec<_>>();
    candidates.sort_by(|a, b| {
        run_sort_key(&b.events_path)
            .cmp(&run_sort_key(&a.events_path))
            .then_with(|| b.id.cmp(&a.id))
    });
    candidates.truncate(cap);
    candidates
}

pub fn render_runs_table(root: &Path) -> String {
    let runs = recent_runs(root, 10);
    if runs.is_empty() {
        return "No runs found for this workspace.".to_string();
    }
    let mut lines = vec![format!(
        "{:<10} {:<16} {:<24} {:<8} {:<9} {}",
        "RUN", "STARTED", "STATUS/ASSURANCE", "PHASES", "RECOVERY", "STOP"
    )];
    for run in runs {
        lines.push(format!(
            "{:<10} {:<16} {:<24} {:<8} {:<9} {}",
            run.short_id,
            run.started_at,
            format!("{}/{}", run.status, run.assurance),
            phase_display(run.completed_phases, run.total_phases),
            run.recovery,
            fit_one_line(&run.stop_reason, 96),
        ));
    }
    lines.join("\n")
}

pub fn newest_run_with_recovery(root: &Path) -> Option<RunInventoryItem> {
    recent_runs(root, 50)
        .into_iter()
        .find(|run| !run.recovery_ultra_plan_path.is_empty())
}

pub fn find_run(root: &Path, id_or_prefix: &str) -> Option<RunInventoryItem> {
    let needle = id_or_prefix.trim();
    if needle.is_empty() {
        return None;
    }
    recent_runs(root, 100)
        .into_iter()
        .find(|run| run.id == needle || run.short_id == needle || run.id.starts_with(needle))
}

fn run_inventory_item(root: &Path, dir: &Path) -> Option<RunInventoryItem> {
    if !dir.is_dir() {
        return None;
    }
    let id = dir.file_name()?.to_string_lossy().to_string();
    let events_path = dir.join("events.jsonl");
    if !events_path.is_file() {
        return None;
    }
    let events = read_events(&events_path);
    let stop = latest_terminal_event(&events);
    let snapshot = crate::eval_events::latest_completion_snapshot(Some(&events_path));
    let ok = stop
        .and_then(|event| event.get("ok").and_then(Value::as_bool))
        .unwrap_or(false);
    let projection = crate::eval_events::project_completion(ok, &snapshot);
    let status = stop
        .and_then(|event| event.get("status").and_then(Value::as_str))
        .filter(|value| !value.trim().is_empty())
        .unwrap_or(&projection.status)
        .to_string();
    let assurance = if projection.assurance_level.trim().is_empty() {
        "missing".to_string()
    } else {
        projection.assurance_level.clone()
    };
    let (completed_phases, total_phases) = phase_counts(&events);
    let stop_reason = stop
        .and_then(|event| latest_text(event, &["stop_reason", "primary_reason", "reason"]))
        .or_else(|| latest_events_text(&events, &["stop_reason", "primary_reason", "reason"]))
        .unwrap_or_else(|| "unknown".to_string());
    let recovery_ultra_plan_path = projection.recovery_ultra_plan_path.clone();
    let recovery_prompt_path = projection.recovery_prompt_path.clone();
    let recovery = recovery_display(root, &recovery_prompt_path, &recovery_ultra_plan_path);
    Some(RunInventoryItem {
        short_id: crate::util::truncate_at_char_boundary(&id, 8).to_string(),
        id,
        started_at: started_at_display(&events_path),
        status,
        assurance,
        completed_phases,
        total_phases,
        stop_reason,
        recovery,
        recovery_ultra_plan_path,
        events_path,
    })
}

fn read_events(path: &Path) -> Vec<Value> {
    std::fs::read_to_string(path)
        .unwrap_or_default()
        .lines()
        .filter_map(|line| serde_json::from_str::<Value>(line).ok())
        .collect()
}

fn latest_terminal_event(events: &[Value]) -> Option<&Value> {
    events.iter().rev().find(|event| {
        matches!(
            event.get("event").and_then(Value::as_str),
            Some("tui_command_stop" | "run_stop")
        )
    })
}

fn latest_events_text(events: &[Value], keys: &[&str]) -> Option<String> {
    events
        .iter()
        .rev()
        .find_map(|event| latest_text(event, keys))
}

fn latest_text(event: &Value, keys: &[&str]) -> Option<String> {
    keys.iter().find_map(|key| {
        event
            .get(*key)
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(ToOwned::to_owned)
    })
}

fn phase_counts(events: &[Value]) -> (Option<usize>, Option<usize>) {
    for event in events.iter().rev() {
        let completed = event
            .get("completed_phase_ids")
            .and_then(Value::as_array)
            .map(Vec::len)
            .or_else(|| {
                event
                    .get("completed_phase_count")
                    .and_then(Value::as_u64)
                    .map(|value| value as usize)
            });
        let pending = event
            .get("pending_phase_ids")
            .and_then(Value::as_array)
            .map(Vec::len);
        if completed.is_some() || pending.is_some() {
            let total = completed.zip(pending).map(|(done, todo)| {
                done + todo
                    + usize::from(
                        event
                            .get("failed_phase_id")
                            .and_then(Value::as_str)
                            .is_some_and(|value| !value.is_empty()),
                    )
            });
            return (completed, total);
        }
        if let Some(total) = event.get("total_phases").and_then(Value::as_u64) {
            let index = event
                .get("phase_index")
                .and_then(Value::as_u64)
                .unwrap_or(0) as usize;
            let event_name = event.get("event").and_then(Value::as_str).unwrap_or("");
            let completed = if matches!(
                event_name,
                "ultra_phase_complete" | "ultra_phase_execute_complete" | "ultra_plan_complete"
            ) {
                Some(index)
            } else if event_name == "ultra_phase_failed" {
                Some(index.saturating_sub(1))
            } else {
                None
            };
            return (completed, Some(total as usize));
        }
    }
    (None, None)
}

fn phase_display(completed: Option<usize>, total: Option<usize>) -> String {
    match (completed, total) {
        (Some(done), Some(total)) => format!("{done}/{total}"),
        (Some(done), None) => format!("{done}/?"),
        (None, Some(total)) => format!("?/{total}"),
        (None, None) => "?/?".to_string(),
    }
}

fn recovery_display(root: &Path, prompt: &str, yaml: &str) -> String {
    let prompt_exists = artifact_exists(root, prompt);
    let yaml_exists = artifact_exists(root, yaml);
    match (prompt_exists, yaml_exists) {
        (_, true) => "yaml".to_string(),
        (true, false) => "prompt".to_string(),
        (false, false) if !yaml.trim().is_empty() => "yaml-missing".to_string(),
        (false, false) if !prompt.trim().is_empty() => "prompt-missing".to_string(),
        _ => "none".to_string(),
    }
}

fn artifact_exists(root: &Path, value: &str) -> bool {
    let value = value.trim();
    if value.is_empty() {
        return false;
    }
    let path = Path::new(value);
    if path.is_absolute() {
        path.is_file()
    } else {
        root.join(path).is_file()
    }
}

fn started_at_display(path: &Path) -> String {
    let secs = run_sort_key(path);
    if secs == 0 {
        "unknown".to_string()
    } else {
        format!("unix:{secs}")
    }
}

fn run_sort_key(path: &Path) -> u64 {
    path.metadata()
        .and_then(|metadata| metadata.modified())
        .ok()
        .and_then(|modified| modified.duration_since(UNIX_EPOCH).ok())
        .map(|duration| duration.as_secs())
        .unwrap_or(0)
}

fn fit_one_line(value: &str, max: usize) -> String {
    let one_line = value.replace(['\n', '\r'], " ");
    crate::util::excerpt_with_marker(&one_line, max, "...")
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn runs_inventory_lists_newest_recoverable_run() {
        let dir = tempfile::tempdir().unwrap();
        let plan = dir
            .path()
            .join(".anvil/plans/recovery-ultra-plan-phase-a.yaml");
        std::fs::create_dir_all(plan.parent().unwrap()).unwrap();
        std::fs::write(
            &plan,
            "goal: \"g\"\nphases:\n  - id: \"p\"\n    prompt: \"p\"\n",
        )
        .unwrap();
        let run_dir = dir.path().join(".anvil/runs/018f1111-aaaa");
        std::fs::create_dir_all(&run_dir).unwrap();
        std::fs::write(
            run_dir.join("events.jsonl"),
            format!(
                "{}\n{}\n",
                json!({
                    "event": "ultra_partial_artifact_summary",
                    "completed_phase_ids": ["setup"],
                    "failed_phase_id": "implement",
                    "pending_phase_ids": ["verify"],
                    "recovery_ultra_plan_path": ".anvil/plans/recovery-ultra-plan-phase-a.yaml"
                }),
                json!({
                    "event": "tui_command_stop",
                    "ok": false,
                    "status": "interrupted",
                    "task_status": "interrupted",
                    "assurance_level": "full",
                    "runtime_acceptance_status": "pass",
                    "final_acceptance_status": "failed",
                    "release_gate_status": "failed",
                    "stop_reason": "interrupted by user while 日本語 path was active",
                    "recovery_ultra_plan_path": ".anvil/plans/recovery-ultra-plan-phase-a.yaml"
                })
            ),
        )
        .unwrap();

        let rendered = render_runs_table(dir.path());

        assert!(rendered.contains("018f1111"), "{rendered}");
        assert!(rendered.contains("interrupted/partial"), "{rendered}");
        assert!(rendered.contains("1/3"), "{rendered}");
        assert!(rendered.contains("yaml"), "{rendered}");
        assert!(rendered.contains("日本語"), "{rendered}");
        assert_eq!(
            newest_run_with_recovery(dir.path()).unwrap().short_id,
            "018f1111"
        );
    }

    #[test]
    fn empty_inventory_is_honest() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(
            render_runs_table(dir.path()),
            "No runs found for this workspace."
        );
    }
}
