//! Python package-marker materialization extracted from `turn.rs`
//! (parent #680).
//!
//! Hosts the two production entry points and their shared materializer:
//!
//! - `materialize_python_package_markers_for_external_import` — creates
//!   `__init__.py` markers under the workspace for packages observed in
//!   verifier stdout/stderr external-import diagnostics.
//! - `materialize_python_package_markers_for_owned_test_imports` —
//!   same materialization but seeded from the owned test artifact list.
//! - `materialize_python_package_marker_candidates` — shared per-path
//!   `create_new` writer + evidence/telemetry record.
//!
//! Originally `impl Agent` methods; converted to free functions taking
//! `&mut Agent`, matching the `actor_loop_flow` / `anti_pattern_flow` /
//! `case_record_flow` pattern. `pub(super)` limited / no facade
//! re-export (DR3-001).

use super::Agent;
use super::auto_test;
use crate::logging::{log_llm_event, stable_path_hash};

pub(super) fn materialize_python_package_markers_for_external_import(
    agent: &mut Agent,
    stdout: &str,
    stderr: &str,
) -> Vec<String> {
    let candidates = auto_test::python_package_marker_candidates_for_external_import(
        &agent.work_root,
        stdout,
        stderr,
    );
    materialize_python_package_marker_candidates(agent, candidates)
}

pub(super) fn materialize_python_package_markers_for_owned_test_imports(
    agent: &mut Agent,
    owned_test_artifacts: &[String],
) -> Vec<String> {
    let candidates = auto_test::python_package_marker_candidates_for_owned_test_imports(
        &agent.work_root,
        owned_test_artifacts,
    );
    materialize_python_package_marker_candidates(agent, candidates)
}

fn materialize_python_package_marker_candidates(
    agent: &mut Agent,
    candidates: Vec<String>,
) -> Vec<String> {
    let mut created = Vec::new();
    for relative_path in candidates {
        let full_path = agent.work_root.join(&relative_path);
        let Some(parent) = full_path.parent() else {
            continue;
        };
        if !parent.is_dir() {
            continue;
        }
        if std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&full_path)
            .is_err()
        {
            continue;
        }
        super::repo_edit_observation::observe_evidence_from_repo_edit(agent, &relative_path);
        log_llm_event(
            "agent.verifier.python_package_marker.created",
            serde_json::json!({
                "session_id": agent.session_store.session_id(),
                "turn_index": agent.current_turn_index,
                "path_hash": stable_path_hash(
                    &crate::session::feedback::mask_secrets(&relative_path)
                ),
            }),
        );
        created.push(relative_path);
    }
    created
}
