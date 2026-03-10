use std::fs;

use anvil::agent::{Agent, OneShotRequest};
use anvil::config::AppConfig;
use anvil::policy::permissions::{
    ExecutionContext, InteractionMode, NonInteractiveBehavior, PermissionMode,
};
use tempfile::tempdir;

#[tokio::test]
async fn one_shot_requires_live_model_or_fails_cleanly() {
    if std::env::var("ANVIL_RUN_LIVE_OLLAMA_TESTS").ok().as_deref() != Some("1") {
        return;
    }

    let dir = tempdir().unwrap();
    let cfg = AppConfig {
        cwd: dir.path().to_path_buf(),
        model: "qwen3.5:35b".to_string(),
        ollama_host: "http://127.0.0.1:11434".to_string(),
        permission_mode: PermissionMode::Ask,
        execution_context: ExecutionContext {
            interaction_mode: InteractionMode::NonInteractive,
            non_interactive_ask: NonInteractiveBehavior::Deny,
            non_interactive_soft_confirm: NonInteractiveBehavior::Deny,
            non_interactive_hard_confirm: NonInteractiveBehavior::Deny,
        },
        state_dir: dir.path().join(".anvil/state"),
    };
    let agent = Agent::new(cfg).await.unwrap();
    let result = agent
        .run_one_shot(OneShotRequest {
            prompt: "Create a tiny HTML file named index.html".to_string(),
            target_dir: dir.path().to_path_buf(),
        })
        .await;

    if let Ok(ok) = result {
        assert!(!ok.written_files.is_empty());
        let content = fs::read_to_string(&ok.written_files[0]).unwrap();
        assert!(!content.is_empty());
    } else {
        let msg = format!("{}", result.unwrap_err());
        assert!(
            msg.contains("Connection") || msg.contains("model response") || msg.contains("error")
        );
    }
}
