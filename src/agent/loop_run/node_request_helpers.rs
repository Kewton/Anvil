//! Issue #977 (parent #974, Issue C): Node request / workspace-state
//! inspection helpers for the MissingDeliverable / MissingEvidence recovery
//! lifecycle.
//!
//! Mirrors `python_request_helpers`: small, side-effect-free signals that
//! gate the deterministic Node test-runner manifest completion. The Node
//! recovery fires when the request targets a Node/JS/TS stack that requires
//! tests, test artifacts already exist, but no runnable test command can be
//! bound (`package.json` missing, or present without a usable `scripts.test`).
//!
//! `pub(super)` limited / no facade re-export (DR3-001).

use std::path::Path;

use super::Agent;
use super::evidence_binding::{BindingCheck, BindingState, evaluate_binding};
use super::node_runner_manifest::{NodeManifestCompletion, complete_node_test_runner_manifest};

/// True when the active request targets a Node/JS/TS coding stack and the
/// task contract requires test execution. Reuses the request-family
/// classifier (`synthesized_missing_test_target_path_for_request`) so the
/// stack gate stays consistent with the rest of the verifier orchestration.
pub(super) fn active_node_request_requires_tests(agent: &Agent) -> bool {
    let Some(request) = super::workspace_access::active_request_text(agent) else {
        return false;
    };
    if !request_targets_node_stack(&request) {
        return false;
    }
    super::task_classification::task_contract_authority(agent)
        .is_some_and(|contract| contract.completion_policy.test_execution_required())
}

fn request_targets_node_stack(request: &str) -> bool {
    matches!(
        super::verifier_orchestration::synthesized_missing_test_target_path_for_request(request),
        Some((_, "javascript" | "typescript"))
    )
}

/// True when a Node test artifact already exists in the workspace (top-level
/// or in a conventional `tests` / `test` / `__tests__` directory).
pub(super) fn node_test_artifact_exists(agent: &Agent) -> bool {
    workspace_has_node_test_file(&agent.work_root)
}

/// The deterministic manifest completion for the current workspace, if any.
/// `None` means the runner is already bindable (a usable `scripts.test`
/// exists) or the manifest is malformed and must not be clobbered.
pub(super) fn node_test_runner_completion(agent: &Agent) -> Option<NodeManifestCompletion> {
    let existing = std::fs::read_to_string(agent.work_root.join("package.json")).ok();
    complete_node_test_runner_manifest(existing.as_deref())
}

/// True when a runnable Node test command can already be bound from the
/// current `package.json` (so no deterministic completion is needed).
pub(super) fn node_test_runner_bindable(agent: &Agent) -> bool {
    node_test_runner_completion(agent).is_none()
}

/// Issue #993 (parent #988, Issue E): the generic evidence-binding state for the
/// current Node workspace. A Node test deliverable that exists but cannot bind a
/// test runner (`package.json`/`scripts.test`) is a binding-order failure
/// ([`BindingState::Failed`] with [`BindingCheck::RunnerManifest`]), not a
/// missing-evidence terminal — the deliverable is present, only the runner
/// binding is missing, so recovery materializes the manifest and reruns the
/// EvidenceRunner rather than asking for more evidence.
pub(super) fn node_evidence_binding_state(agent: &Agent) -> BindingState {
    evaluate_binding(
        BindingCheck::RunnerManifest,
        node_test_artifact_exists(agent),
        node_test_runner_bindable(agent),
    )
}

const NODE_TEST_FILE_SUFFIXES: &[&str] = &[
    ".test.js",
    ".test.mjs",
    ".test.cjs",
    ".test.jsx",
    ".test.ts",
    ".test.mts",
    ".test.cts",
    ".test.tsx",
    ".spec.js",
    ".spec.mjs",
    ".spec.cjs",
    ".spec.jsx",
    ".spec.ts",
    ".spec.mts",
    ".spec.cts",
    ".spec.tsx",
];

fn is_node_test_filename(name: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    NODE_TEST_FILE_SUFFIXES
        .iter()
        .any(|suffix| lower.ends_with(suffix))
}

fn workspace_has_node_test_file(work_root: &Path) -> bool {
    if dir_has_node_test_file(work_root) {
        return true;
    }
    ["tests", "test", "__tests__"]
        .iter()
        .any(|sub| dir_has_node_test_file(&work_root.join(sub)))
}

fn dir_has_node_test_file(dir: &Path) -> bool {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return false;
    };
    entries.flatten().any(|entry| {
        entry.path().is_file()
            && entry
                .file_name()
                .to_str()
                .is_some_and(is_node_test_filename)
    })
}

#[cfg(test)]
mod tests {
    use super::super::evidence_binding::{BindingCheck, BindingRecovery, BindingState};
    use super::super::node_runner_manifest::NodeManifestAction;
    use super::*;

    #[test]
    fn node_test_filename_predicate_matches_common_shapes() {
        assert!(is_node_test_filename("index.test.js"));
        assert!(is_node_test_filename("main.test.ts"));
        assert!(is_node_test_filename("App.spec.jsx"));
        assert!(is_node_test_filename("cli.test.mjs"));
        assert!(!is_node_test_filename("index.js"));
        assert!(!is_node_test_filename("test_main.py"));
        assert!(!is_node_test_filename("README.md"));
    }

    #[test]
    fn workspace_test_file_detected_top_level_and_in_tests_dir() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        assert!(!workspace_has_node_test_file(root));

        std::fs::write(root.join("index.js"), "export const x = 1;\n").unwrap();
        assert!(!workspace_has_node_test_file(root));

        std::fs::create_dir_all(root.join("tests")).unwrap();
        std::fs::write(root.join("tests").join("index.test.js"), "// test\n").unwrap();
        assert!(workspace_has_node_test_file(root));
    }

    #[test]
    fn workspace_test_file_detected_at_top_level() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        std::fs::write(root.join("main.spec.ts"), "// test\n").unwrap();
        assert!(workspace_has_node_test_file(root));
    }

    /// Issue #993 (parent #988, Issue E) acceptance: a Node CSV-equivalent
    /// workspace with `tests/main.test.js` but no `package.json` is a
    /// binding-order failure that proceeds to `package.json` materialization
    /// (`CreateManifest`), NOT a missing-evidence terminal. This pins the
    /// workspace-probe + generic `evaluate_binding` decision the
    /// `node_evidence_binding_state` Agent wrapper delegates to.
    #[test]
    fn node_tests_main_test_js_without_manifest_proceeds_to_materialization() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        std::fs::create_dir_all(root.join("tests")).unwrap();
        std::fs::write(
            root.join("tests").join("main.test.js"),
            "import { test } from 'node:test';\ntest('ok', () => {});\n",
        )
        .unwrap();

        // The test deliverable exists.
        let test_artifact_exists = workspace_has_node_test_file(root);
        assert!(test_artifact_exists);

        // No `package.json` -> the runner cannot bind; deterministic completion
        // creates a fresh manifest (proceeds to materialization).
        let existing = std::fs::read_to_string(root.join("package.json")).ok();
        let completion = complete_node_test_runner_manifest(existing.as_deref())
            .expect("missing manifest proceeds to materialization");
        assert_eq!(completion.action, NodeManifestAction::CreateManifest);
        let runner_bindable = false;

        // The generic binding evaluation classifies this as a binding failure
        // routed to runner-manifest materialization.
        let state = evaluate_binding(
            BindingCheck::RunnerManifest,
            test_artifact_exists,
            runner_bindable,
        );
        let job = state
            .failed_job()
            .expect("deliverable present + unbound runner is a binding failure");
        assert_eq!(job.check, BindingCheck::RunnerManifest);
        assert_eq!(job.recovery, BindingRecovery::MaterializeRunnerManifest);
    }

    /// Once the manifest binds a usable `scripts.test`, the same workspace is no
    /// longer a binding failure — recovery does not loop.
    #[test]
    fn node_workspace_with_bound_runner_is_not_a_binding_failure() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        std::fs::create_dir_all(root.join("tests")).unwrap();
        std::fs::write(root.join("tests").join("main.test.js"), "// test\n").unwrap();
        std::fs::write(
            root.join("package.json"),
            r#"{"name":"app","scripts":{"test":"node --test"}}"#,
        )
        .unwrap();

        let existing = std::fs::read_to_string(root.join("package.json")).ok();
        let runner_bindable = complete_node_test_runner_manifest(existing.as_deref()).is_none();
        assert!(runner_bindable);

        let state = evaluate_binding(
            BindingCheck::RunnerManifest,
            workspace_has_node_test_file(root),
            runner_bindable,
        );
        assert_eq!(state, BindingState::Bound);
    }
}
