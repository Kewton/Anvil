use std::path::Path;

use anvil::agent::Agent;
use anvil::agent::loop_run::{AgentEvent, FooterHandle, select_precautions_for_prompt};
use anvil::config::Config;
use anvil::model_registry::RuntimeModels;
use anvil::modes::plan_act::ExecutionMode;
use anvil::ollama::client::OllamaClient;
use anvil::session::precaution::PrecautionStatus;
use anvil::session::store::{SessionSnapshot, SessionStore};
use tempfile::tempdir;

fn build_agent(
    cwd: &Path,
    state_root: &Path,
    session_id: &str,
    workspace_key: &str,
    session: SessionSnapshot,
) -> Agent {
    let mut config = Config::default();
    config.cwd = cwd.to_path_buf();
    config.requested_model = Some("test-model".to_string());
    config.ollama_host = "http://127.0.0.1:11434".to_string();
    config.state_dir_override = Some(state_root.to_path_buf());

    Agent::new(
        config,
        RuntimeModels {
            main: "test-model".to_string(),
            sidecar: None,
        },
        OllamaClient::new("http://127.0.0.1:11434".to_string()).unwrap(),
        SessionStore::new(state_root, session_id, workspace_key),
        session,
        FooterHandle::disabled(),
    )
}

fn continue_message(event: AgentEvent) -> String {
    match event {
        AgentEvent::Continue(Some(message)) => message,
        AgentEvent::Continue(None) => panic!("expected Continue(Some), got Continue(None)"),
        AgentEvent::Exit => panic!("expected Continue, got Exit"),
    }
}

#[test]
fn precautions_add_resume_list_prompt_and_retire_via_repl() {
    let temp = tempdir().unwrap();
    let state_root = temp.path().join(".anvil-state");
    let session_id = "0199fe00-0000-7000-8000-000000000454";
    let workspace_key = "test-workspace";
    let store = SessionStore::new(&state_root, session_id, workspace_key);
    let session = SessionSnapshot {
        id: session_id.to_string(),
        workspace_key: workspace_key.to_string(),
        ..SessionSnapshot::default()
    };
    let mut agent = build_agent(temp.path(), &state_root, session_id, workspace_key, session);

    let add = continue_message(
        agent
            .process_line("/precautions add Do not modify generated files", false)
            .unwrap(),
    );
    assert!(add.contains("added precaution"), "{add}");

    let mut loaded = store.load_or_new(false).unwrap();
    loaded
        .working_memory
        .sanitize_active_precautions_after_load(temp.path());
    let precaution = loaded
        .working_memory
        .active_precautions
        .iter()
        .find(|precaution| precaution.status == PrecautionStatus::Active)
        .expect("active precaution persisted");
    let id12 = precaution.id[..12].to_string();
    assert_eq!(precaution.text, "Do not modify generated files");

    let mut resumed = build_agent(
        temp.path(),
        &state_root,
        session_id,
        workspace_key,
        loaded.clone(),
    );
    let list = continue_message(resumed.process_line("/precautions", false).unwrap());
    assert!(list.contains(&id12), "{list}");
    assert!(list.contains("Do not modify generated files"), "{list}");

    let selected = select_precautions_for_prompt(
        &loaded.working_memory.active_precautions,
        ExecutionMode::Act,
        &loaded.working_memory.touched_files,
        None,
    );
    let prompt = loaded
        .working_memory
        .format_for_prompt_with_precautions(&selected)
        .expect("active precaution should render");
    assert!(prompt.contains("Active Precautions:"), "{prompt}");
    assert!(
        prompt.contains("- [medium] Do not modify generated files"),
        "{prompt}"
    );

    let retire = continue_message(
        resumed
            .process_line(&format!("/precautions retire {id12}"), false)
            .unwrap(),
    );
    assert_eq!(retire, format!("retired: {id12}"));

    let mut retired = store.load_or_new(false).unwrap();
    retired
        .working_memory
        .sanitize_active_precautions_after_load(temp.path());
    let selected_after = select_precautions_for_prompt(
        &retired.working_memory.active_precautions,
        ExecutionMode::Act,
        &retired.working_memory.touched_files,
        None,
    );
    let prompt_after = retired
        .working_memory
        .format_for_prompt_with_precautions(&selected_after);
    assert!(
        prompt_after
            .as_deref()
            .is_none_or(|body| !body.contains("Active Precautions:")),
        "{prompt_after:?}"
    );
}
