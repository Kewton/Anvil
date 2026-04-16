use std::env;

use anvil::agent::Agent;
use anvil::config::Config;
use anvil::model_registry::RuntimeModels;
use anvil::ollama::client::OllamaClient;
use anvil::session::store::SessionStore;
use tempfile::tempdir;

#[test]
#[ignore = "requires live Ollama"]
fn live_ollama_can_write_a_file() {
    let temp = tempdir().unwrap();
    let host =
        env::var("ANVIL_E2E_OLLAMA_HOST").unwrap_or_else(|_| "http://127.0.0.1:11434".to_string());
    let model = env::var("ANVIL_E2E_MODEL").unwrap_or_else(|_| "qwen3:8b".to_string());

    let client = OllamaClient::new(host.clone()).unwrap();
    let available = client.list_models().unwrap();
    if !available.iter().any(|candidate| candidate == &model) {
        eprintln!("skip live e2e: model {model} is not installed at {host}");
        return;
    }

    let config = Config {
        cwd: temp.path().to_path_buf(),
        requested_model: Some(model.clone()),
        requested_sidecar_model: None,
        ollama_host: host,
        context_budget: 24_000,
        max_iterations: 8,
        debug: false,
        yes_mode: true,
        fresh_session: true,
        oneshot: true,
        prompt: None,
    };
    config.ensure_state_dirs().unwrap();

    let mut agent = Agent::new(
        config,
        RuntimeModels {
            main: model,
            sidecar: None,
        },
        client,
        SessionStore::new(temp.path().join(".anvil/sessions/session.json")),
        Default::default(),
    );

    let prompt = "Use the available file tools to create a file named e2e-output.txt in the current project root. The file content must be exactly LOCAL_E2E_OK on a single line. After writing the file, reply with one short sentence.";
    agent.run_oneshot(prompt).unwrap();

    let content = std::fs::read_to_string(temp.path().join("e2e-output.txt")).unwrap();
    assert_eq!(content.trim(), "LOCAL_E2E_OK");
}
