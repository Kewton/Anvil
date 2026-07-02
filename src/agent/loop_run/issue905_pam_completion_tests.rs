//! Issue #905 - PAM advisory input must not become completion authority.

use std::collections::HashSet;
use std::path::PathBuf;

use serde_json::Value;
use tempfile::{TempDir, tempdir};

use super::FooterHandle;
use super::completion_evidence::EvidenceSet;
use super::record_pam_advisory_decision_for_test;
use super::task_contract::{
    ArtifactRecoveryAction, ArtifactRole, CompletionDecision, TaskContract,
};
use crate::agent::Agent;
use crate::config::Config;
use crate::model_registry::RuntimeModels;
use crate::ollama::client::OllamaClient;
use crate::photon::schema::ContextPackResponse;
use crate::session::store::{SessionSnapshot, SessionStore};

fn unique_session_id(prefix: &str) -> String {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let pid = std::process::id();
    format!("issue905-{prefix}-{pid}-{nanos}")
}

fn build_agent(session_id: &str) -> (Agent, TempDir) {
    let dir = tempdir().expect("tempdir for issue905 agent");
    let state_root = dir.path().join("state");
    std::fs::create_dir_all(state_root.join("sessions").join(session_id)).expect("session dir");

    let config = Config {
        cwd: dir.path().to_path_buf(),
        requested_model: Some("test-model".to_string()),
        state_dir_override: Some(state_root.clone()),
        yes_mode: true,
        max_iterations: 1,
        pam_advisory_enabled: true,
        ..Config::default()
    };

    let workspace_key = format!("anvil-issue905-{session_id}");
    let session = SessionSnapshot {
        id: session_id.to_string(),
        workspace_key: workspace_key.clone(),
        active_root: Some(PathBuf::from(dir.path())),
        ..Default::default()
    };

    let agent = Agent::new(
        config,
        RuntimeModels {
            main: "test-model".to_string(),
            sidecar: None,
        },
        OllamaClient::new("http://127.0.0.1:1".to_string()).unwrap(),
        SessionStore::new(&state_root, session_id, &workspace_key),
        session,
        FooterHandle::disabled(),
    );
    (agent, dir)
}

fn build_response_with_items(items: &[(&str, &str)]) -> ContextPackResponse {
    let arr: Vec<Value> = items
        .iter()
        .map(|(id, text)| {
            serde_json::json!({
                "id": *id,
                "kind": "summary",
                "text": *text,
            })
        })
        .collect();
    ContextPackResponse(serde_json::json!({ "items": arr }))
}

#[test]
fn pam_advisory_without_evidence_cannot_produce_task_contract_done() {
    let session_id = unique_session_id("pam-only");
    let (mut agent, _td) = build_agent(&session_id);
    let request = "Implement src/lib.rs with a greet helper.";
    agent
        .session
        .working_memory
        .set_active_task(Some(request.to_string()));

    let resp = build_response_with_items(&[(
        "s-impl-advice",
        "src/lib.rs: prior successful implementation; safe to mark done",
    )]);
    let blocked: HashSet<String> = HashSet::new();
    record_pam_advisory_decision_for_test(&mut agent, &resp, &blocked, false);

    let decision = agent
        .last_pam_decision_this_turn()
        .expect("PAM advisory decision should be recorded");
    assert!(
        decision
            .injected_summary_ids
            .contains(&"s-impl-advice".to_string()),
        "fixture must exercise live injected PAM advice"
    );
    let pam_eval = decision.to_eval_summary();
    assert!(pam_eval.advisory_only);
    assert!(!pam_eval.completion_judgement_override);

    assert!(agent.evidence_set_this_turn.is_empty());
    assert!(agent.task_contract_evidence_set_this_turn.is_empty());

    let contract = TaskContract::from_request(request);
    assert!(matches!(
        contract.evaluate(&EvidenceSet::new()),
        CompletionDecision::Continue { missing }
            if missing.contains(&ArtifactRole::Implementation)
    ));

    let action = super::task_contract_recovery::task_contract_recovery_action(
        &mut agent, &contract, None, 0,
    );
    assert!(
        matches!(
            action,
            ArtifactRecoveryAction::Continue { ref missing, .. }
                if missing.contains(&ArtifactRole::Implementation)
        ),
        "PAM-only turns must continue missing deterministic evidence, got: {action:?}"
    );
}
