use std::path::PathBuf;

use anvil::config::{AppConfig, ProviderKind};
use anvil::policy::permissions::{
    ExecutionContext, InteractionMode, NonInteractiveBehavior, PermissionMode,
};

#[test]
fn app_config_holds_runtime_settings() {
    let cfg = AppConfig {
        cwd: PathBuf::from("/tmp/project"),
        provider: ProviderKind::Ollama,
        model: "qwen3.5:35b".to_string(),
        host: "http://127.0.0.1:11434".to_string(),
        permission_mode: PermissionMode::Ask,
        execution_context: ExecutionContext {
            interaction_mode: InteractionMode::Interactive,
            non_interactive_ask: NonInteractiveBehavior::Deny,
            non_interactive_soft_confirm: NonInteractiveBehavior::Deny,
            non_interactive_hard_confirm: NonInteractiveBehavior::Deny,
        },
        state_dir: PathBuf::from("/tmp/project/.anvil/state"),
    };

    assert_eq!(cfg.model, "qwen3.5:35b");
    assert_eq!(cfg.host, "http://127.0.0.1:11434");
    assert_eq!(cfg.state_dir, PathBuf::from("/tmp/project/.anvil/state"));
}
