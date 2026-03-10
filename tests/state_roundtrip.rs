use std::path::PathBuf;

use anvil::roles::RoleRegistry;
use anvil::runtime::{NetworkPolicy, PermissionMode};
use anvil::state::handoff::HandoffFile;
use anvil::state::session::{
    AgentModels, DelegationRecord, EvidenceRecord, Finding, ResultRecord, SessionState,
};
use anvil::state::store::StateStore;
use tempfile::tempdir;

#[test]
fn session_state_roundtrip_validates_and_persists() {
    let registry = RoleRegistry::load_builtin().expect("registry");
    let temp = tempdir().expect("tempdir");
    let store = StateStore::new(PathBuf::from(temp.path()));
    let session = sample_session();

    let path = store
        .save_session(&registry, &session)
        .expect("save session");
    let loaded = store
        .load_session(&registry, &session.session_id)
        .expect("load session");

    assert_eq!(loaded.session_id, session.session_id);
    assert_eq!(path, temp.path().join("sessions/session-123.json"));
}

#[test]
fn handoff_roundtrip_validates_and_persists() {
    let registry = RoleRegistry::load_builtin().expect("registry");
    let temp = tempdir().expect("tempdir");
    let store = StateStore::new(PathBuf::from(temp.path()));
    let session = sample_session();
    let handoff = HandoffFile::from_session(&session, "anvil-session-export");

    let path = store
        .save_handoff(&registry, &handoff)
        .expect("save handoff");
    let loaded = store.load_handoff(&registry, &path).expect("load handoff");

    assert_eq!(loaded.session_id, session.session_id);
    assert_eq!(loaded.pending_steps, session.pending_steps);
    assert_eq!(path, temp.path().join("handoffs/session-123.json"));
}

fn sample_session() -> SessionState {
    SessionState {
        session_id: "session-123".to_string(),
        pm_model: "qwen-coder-14b".to_string(),
        permission_mode: PermissionMode::WorkspaceWrite,
        network_policy: NetworkPolicy::LocalOnly,
        agent_models: AgentModels {
            reader: None,
            editor: Some("qwen-coder-14b".to_string()),
            tester: None,
            reviewer: Some("deepseek-coder-14b".to_string()),
        },
        objective: "Implement the MVP runtime permission layer".to_string(),
        working_summary: "Permission checks are partly stubbed and need wiring.".to_string(),
        user_preferences_summary: "Prefer small, reviewable changes.".to_string(),
        repository_summary: "Rust CLI with schema files under schemas/.".to_string(),
        active_constraints: vec!["No network by default".to_string()],
        open_questions: vec!["Should handoff import preserve delegations?".to_string()],
        completed_steps: vec!["Loaded the role registry".to_string()],
        pending_steps: vec!["Add permission policy tests".to_string()],
        relevant_files: vec!["src/runtime/permissions.rs".to_string()],
        recent_delegations: vec![DelegationRecord {
            id: "delegation-1".to_string(),
            role: "editor".to_string(),
            resolved_model: "qwen-coder-14b".to_string(),
            inherited_from_pm: false,
            task: "Implement permission-mode serialization.".to_string(),
            requested_permission: Some(PermissionMode::WorkspaceWrite),
            created_at: "2026-03-10T00:00:00Z".to_string(),
        }],
        recent_results: vec![ResultRecord {
            role: "reviewer".to_string(),
            model: "deepseek-coder-14b".to_string(),
            summary: "Noted that destructive command handling lacks tests.".to_string(),
            evidence: vec![EvidenceRecord {
                source_type: "repo-file".to_string(),
                value: "src/policy/command_classification.rs".to_string(),
            }],
            changed_files: vec!["src/policy/command_classification.rs".to_string()],
            commands_run: vec!["cargo test".to_string()],
            next_recommendation: Some("Add permission-policy coverage.".to_string()),
            findings: vec![Finding {
                severity: "medium".to_string(),
                message: "Networked and destructive commands are not distinguished.".to_string(),
                file: Some("src/policy/command_classification.rs".to_string()),
            }],
        }],
        pending_confirmation: None,
    }
}
