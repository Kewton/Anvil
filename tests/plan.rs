use anvil::agent::plan::{AgentMode, PlanDocument, PlanState};
use tempfile::tempdir;

#[test]
fn plan_file_can_be_created_loaded_and_activated() {
    let dir = tempdir().unwrap();
    let created = PlanState::create_plan(
        dir.path(),
        "ship-mvp",
        "1. inspect code\n2. update tests\n3. implement",
    )
    .unwrap();
    let loaded = PlanDocument::load(&created.path).unwrap();
    let state = PlanState::activate(loaded.clone());

    assert_eq!(state.mode, AgentMode::Act);
    assert_eq!(
        loaded.body,
        "1. inspect code\n2. update tests\n3. implement"
    );
    assert!(state.injection().unwrap().contains("Active plan summary"));
}

#[test]
fn plan_injection_uses_summary_not_full_body() {
    let long_body = (0..80)
        .map(|i| format!("step {i}: something very detailed"))
        .collect::<Vec<_>>()
        .join("\n");
    let doc = PlanDocument::new(
        "/tmp/plan.md".into(),
        long_body.clone(),
        Some("short summary".to_string()),
    );
    let state = PlanState::activate(doc);
    let injected = state.injection().unwrap();

    assert!(injected.contains("short summary"));
    assert!(!injected.contains(&long_body));
}
