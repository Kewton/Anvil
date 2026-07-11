use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use anvilminimal::eval_events::{
    CompletionSnapshot, append_completion_summary, project_completion,
};

#[test]
fn generic_profile_reduced_assurance_markers_still_render() {
    let dir = tempfile::tempdir().unwrap();
    let events_path = dir.path().join(".anvil/runs/generic/events.jsonl");
    let mut snapshot = CompletionSnapshot::empty();
    snapshot.assurance_level = "reduced".to_string();
    snapshot.assurance_reason =
        anvilminimal::eval_events::GENERIC_REDUCED_ASSURANCE_REASON.to_string();
    snapshot.runtime_acceptance_status = "pass".to_string();
    snapshot.final_acceptance_status = "partial".to_string();
    snapshot.release_gate_status = "partial".to_string();
    snapshot.release_gate_reasons = vec![
        "interaction_unverified:probe_unavailable".to_string(),
        "generic_profile_reduced_assurance".to_string(),
    ];
    snapshot.unverified_evidence = vec![
        "stateful_update_evidence:unverified:probe_unavailable".to_string(),
        "browser_interaction:unverified:generic_profile_reduced_assurance".to_string(),
    ];
    snapshot.browser_readiness_status =
        "unavailable:browser_readiness_evidence_missing".to_string();
    snapshot.interaction_evidence_status = "unavailable:interaction_evidence_missing".to_string();
    snapshot.evidence_arbitration_summary = "partial (probe unavailable)".to_string();

    let projection = project_completion(true, &snapshot);
    assert_eq!(projection.status, "complete_with_partial_release_gate");
    assert_eq!(projection.task_status, "partial (interaction unverified)");
    append_completion_summary(
        Some(&events_path),
        "process",
        Some("ultra-plan-run"),
        Some("/ultra-plan-run --profile generic build an interactive app"),
        "completed",
        "",
        &projection,
    );

    let summary =
        std::fs::read_to_string(events_path.parent().unwrap().join("summary.md")).expect("summary");
    for marker in [
        "Task status: partial (interaction unverified)",
        "Final acceptance: partial",
        "Release gate: partial",
        "Release gate reasons:",
        "- interaction_unverified:probe_unavailable",
        "- generic_profile_reduced_assurance",
        "Unverified (probe required):",
        "- stateful_update_evidence:unverified:probe_unavailable",
        "- browser_interaction:unverified:generic_profile_reduced_assurance",
        "Interaction verification: interaction_unverified:probe_unavailable",
        "Next action: run_setup_interaction_probe_to_enable_interaction_release_checks",
        "Assurance: reduced (generic profile — no capability contract, no behavioral verification)",
    ] {
        assert!(
            summary.contains(marker),
            "missing marker {marker:?}\n{summary}"
        );
    }
    assert!(
        !summary.contains("Final acceptance: full_success"),
        "{summary}"
    );
}

#[test]
fn generic_profile_static_assurance_markers_render() {
    let dir = tempfile::tempdir().unwrap();
    let events_path = dir.path().join(".anvil/runs/generic-static/events.jsonl");
    let mut snapshot = CompletionSnapshot::empty();
    snapshot.assurance_level = "static".to_string();
    snapshot.assurance_reason =
        anvilminimal::eval_events::GENERIC_STATIC_ASSURANCE_REASON.to_string();
    snapshot.runtime_acceptance_status = "pass".to_string();
    snapshot.final_acceptance_status = "full_success".to_string();
    snapshot.release_gate_status = "pass".to_string();

    let projection = project_completion(true, &snapshot);
    assert_eq!(projection.status, "complete");
    assert_eq!(projection.task_status, "completed (static assurance)");
    append_completion_summary(
        Some(&events_path),
        "process",
        Some("ultra-plan-run"),
        Some("/ultra-plan-run --profile generic ちょっとしたメモアプリを作って"),
        "completed",
        "",
        &projection,
    );

    let summary =
        std::fs::read_to_string(events_path.parent().unwrap().join("summary.md")).expect("summary");
    for marker in [
        "Task status: completed (static assurance)",
        "Runtime acceptance: pass",
        "Final acceptance: full_success",
        "Release gate: pass",
        "Assurance: static (generic profile — minimal interactive contract verified statically; no behavioral verification)",
    ] {
        assert!(
            summary.contains(marker),
            "missing marker {marker:?}\n{summary}"
        );
    }
}

#[test]
fn nextjs_boundary_erosion_tripwire_keeps_dispatch_sites_audited() {
    let actual = nextjs_literal_counts_outside_profiles();
    let expected = BTreeMap::from([
        ("src/minimal_loop/browser_probe.rs".to_string(), 2),
        ("src/minimal_loop/completion.rs".to_string(), 1),
        ("src/minimal_loop/evidence.rs".to_string(), 4),
        ("src/minimal_loop/import_scan.rs".to_string(), 1),
        ("src/minimal_loop/loop_run.rs".to_string(), 2),
        ("src/planner/assurance.rs".to_string(), 1),
        ("src/planner/lint.rs".to_string(), 2),
        ("src/planner/final_acceptance.rs".to_string(), 1),
        ("src/planner/profile.rs".to_string(), 3),
        ("src/planner/runner.rs".to_string(), 14),
        ("src/planner/verify.rs".to_string(), 3),
    ]);
    assert_eq!(
        actual, expected,
        "new production \"nextjs\" literal outside src/planner/profiles must be audited here or moved behind the profile boundary"
    );
}

#[test]
fn runner_chokepoints_do_not_grow_past_interim_budget() {
    for (path, baseline) in [
        ("src/planner/runner.rs", 18_242usize),
        ("src/minimal_loop/loop_run.rs", 7_444usize),
        ("src/minimal_loop/repair_pressure.rs", 746usize),
        ("src/planner/repair_targeting.rs", 597usize),
        ("src/planner/final_acceptance.rs", 2_942usize),
        ("src/planner/ultra_plan_flow.rs", 1_570usize),
        ("src/planner/assurance.rs", 1_311usize),
    ] {
        let text = std::fs::read_to_string(path)
            .unwrap_or_else(|err| panic!("failed to read {path}: {err}"));
        let current = text.lines().count();
        let allowed = baseline + ((baseline * 2).div_ceil(100));
        assert!(
            current <= allowed,
            "{path} grew to {current} lines; baseline is {baseline}, allowed max is {allowed}. Move new subsystems to new modules or land a shrinking refactor first."
        );
    }
}

fn nextjs_literal_counts_outside_profiles() -> BTreeMap<String, usize> {
    let mut counts = BTreeMap::new();
    for path in rust_source_files(Path::new("src")) {
        let rel = path.to_string_lossy().replace('\\', "/");
        if rel.starts_with("src/planner/profiles/") {
            continue;
        }
        let text = std::fs::read_to_string(&path)
            .unwrap_or_else(|err| panic!("failed to read {}: {err}", path.display()));
        let count = production_lines(&text)
            .into_iter()
            .filter(|line| line.contains("\"nextjs\""))
            .count();
        if count > 0 {
            counts.insert(rel, count);
        }
    }
    counts
}

fn rust_source_files(root: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    collect_rust_source_files(root, &mut out);
    out.sort();
    out
}

fn collect_rust_source_files(root: &Path, out: &mut Vec<PathBuf>) {
    for entry in std::fs::read_dir(root)
        .unwrap_or_else(|err| panic!("failed to read {}: {err}", root.display()))
        .flatten()
    {
        let path = entry.path();
        if path.is_dir() {
            collect_rust_source_files(&path, out);
        } else if path.extension().and_then(|ext| ext.to_str()) == Some("rs") {
            out.push(path);
        }
    }
}

fn production_lines(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut skip_next_cfg_test_item = false;
    let mut skipping_cfg_test_block = false;
    let mut block_depth = 0i32;

    for line in text.lines() {
        let trimmed = line.trim();
        if skipping_cfg_test_block {
            block_depth += brace_delta(line);
            if block_depth <= 0 {
                skipping_cfg_test_block = false;
                block_depth = 0;
            }
            continue;
        }
        if skip_next_cfg_test_item {
            if trimmed.starts_with("#[") {
                continue;
            }
            if trimmed.starts_with("mod tests") {
                break;
            }
            let delta = brace_delta(line);
            if delta > 0 {
                skipping_cfg_test_block = true;
                block_depth = delta;
                if block_depth <= 0 {
                    skipping_cfg_test_block = false;
                }
            }
            skip_next_cfg_test_item = false;
            continue;
        }
        if trimmed == "#[cfg(test)]" {
            skip_next_cfg_test_item = true;
            continue;
        }
        out.push(line.to_string());
    }
    out
}

fn brace_delta(line: &str) -> i32 {
    line.chars().fold(0, |depth, ch| match ch {
        '{' => depth + 1,
        '}' => depth - 1,
        _ => depth,
    })
}
