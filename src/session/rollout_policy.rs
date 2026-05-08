use std::io::BufRead;
use std::path::Path;

use crate::session::discovery;

pub const MAX_ROLLOUT_EVAL_BYTES: usize = 64 * 1024 * 1024;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConditionStatus {
    Ok,
    Ng(String),
    ManualRequired(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConditionResult {
    pub id: u8,
    pub label: String,
    pub status: ConditionStatus,
    pub note: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RolloutStatus {
    pub conditions: Vec<ConditionResult>,
    pub automatic_checks_passed: bool,
    pub manual_required: bool,
    pub ready_for_canary: bool,
    pub eval_turns_found: u32,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RolloutStats {
    pub photon_eval_turns: u32,
    pub skipped_corrupt_records: usize,
    pub skipped_oversize_sessions: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RolloutPolicyConfig {
    pub min_eval_turns: u32,
}

/// Minimal probe struct — reads only fields needed for rollout decisions.
/// Does not add Deserialize to the full EvalRecord chain.
#[derive(serde::Deserialize)]
struct RolloutEvalRecordProbe {
    #[serde(default)]
    photon_eval: Option<serde_json::Value>,
    #[serde(default)]
    photon_canary: u16,
}

/// Walk all session eval.jsonl files and count photon_eval records.
pub fn collect_rollout_stats_from_state(state_root: &Path) -> Result<RolloutStats, String> {
    let mut stats = RolloutStats::default();

    let root_canonical = state_root
        .canonicalize()
        .unwrap_or_else(|_| state_root.to_path_buf());

    for entry in discovery::iter_session_dirs(state_root) {
        let eval_log_path = entry.dir.join("logs").join("eval.jsonl");
        if !eval_log_path.exists() {
            continue;
        }

        if let Err(_e) = ensure_regular_file_inside_root(&eval_log_path, &root_canonical, &entry.id)
        {
            stats.skipped_oversize_sessions += 1;
            continue;
        }

        let meta = match std::fs::metadata(&eval_log_path) {
            Ok(m) => m,
            Err(_) => {
                stats.skipped_oversize_sessions += 1;
                continue;
            }
        };
        if meta.len() > MAX_ROLLOUT_EVAL_BYTES as u64 {
            stats.skipped_oversize_sessions += 1;
            continue;
        }

        let file = match std::fs::File::open(&eval_log_path) {
            Ok(f) => f,
            Err(_) => {
                stats.skipped_oversize_sessions += 1;
                continue;
            }
        };
        let reader = std::io::BufReader::new(file);
        for line in reader.lines() {
            let line = match line {
                Ok(l) => l,
                Err(_) => {
                    stats.skipped_corrupt_records += 1;
                    continue;
                }
            };
            if line.trim().is_empty() {
                continue;
            }
            let Ok(record) = serde_json::from_str::<RolloutEvalRecordProbe>(&line) else {
                stats.skipped_corrupt_records += 1;
                continue;
            };
            let _ = record.photon_canary; // used when analysing externally
            if record.photon_eval.is_some() {
                stats.photon_eval_turns += 1;
            }
        }
    }

    Ok(stats)
}

/// Pure function: derive rollout readiness from stats.
pub fn evaluate_rollout_conditions(
    stats: &RolloutStats,
    config: RolloutPolicyConfig,
) -> RolloutStatus {
    let cond2_status = if stats.photon_eval_turns >= config.min_eval_turns {
        ConditionStatus::Ok
    } else {
        ConditionStatus::Ng(format!(
            "found {} photon_eval turns, need {}",
            stats.photon_eval_turns, config.min_eval_turns
        ))
    };

    let conditions = vec![
        ConditionResult {
            id: 1,
            label: "sidecar fail-open".into(),
            status: ConditionStatus::Ok,
            note: Some("already implemented (Issue #554)".into()),
        },
        ConditionResult {
            id: 2,
            label: "minimum eval turns".into(),
            status: cond2_status,
            note: None,
        },
        ConditionResult {
            id: 3,
            label: "unsafe context filter".into(),
            status: ConditionStatus::Ok,
            note: Some("already implemented (Issue #557)".into()),
        },
        ConditionResult {
            id: 4,
            label: "prompt size cap".into(),
            status: ConditionStatus::Ok,
            note: Some("already implemented (Issue #557)".into()),
        },
        ConditionResult {
            id: 5,
            label: "task success rate".into(),
            status: ConditionStatus::ManualRequired(
                "compare canary/non-canary anvil_score.success_score externally".into(),
            ),
            note: Some("photon_canary is logged for external analysis".into()),
        },
    ];

    let automatic_checks_passed = conditions
        .iter()
        .all(|c| !matches!(c.status, ConditionStatus::Ng(_)));
    let manual_required = conditions
        .iter()
        .any(|c| matches!(c.status, ConditionStatus::ManualRequired(_)));
    let ready_for_canary = automatic_checks_passed && !manual_required;

    RolloutStatus {
        conditions,
        automatic_checks_passed,
        manual_required,
        ready_for_canary,
        eval_turns_found: stats.photon_eval_turns,
    }
}

/// Verify path is a regular file (not symlink) confined inside root_canonical.
fn ensure_regular_file_inside_root(
    path: &Path,
    root_canonical: &Path,
    session_id: &str,
) -> Result<(), String> {
    let symlink_meta = path
        .symlink_metadata()
        .map_err(|_| format!("cannot stat eval log for session {session_id}"))?;
    if symlink_meta.file_type().is_symlink() {
        return Err(format!("eval log is a symlink for session {session_id}"));
    }
    let meta = std::fs::metadata(path)
        .map_err(|_| format!("cannot stat eval log for session {session_id}"))?;
    if !meta.is_file() {
        return Err(format!(
            "eval log is not a regular file for session {session_id}"
        ));
    }
    let canonical = path
        .canonicalize()
        .map_err(|_| format!("cannot canonicalize eval log for session {session_id}"))?;
    if !canonical.starts_with(root_canonical) {
        return Err(format!(
            "eval log path escape detected for session {session_id}"
        ));
    }
    Ok(())
}
