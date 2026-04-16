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
        chat_timeout_secs: 300,
        chat_retries: 1,
        debug: false,
        stream: false,
        tui: false,
        watch: false,
        auto_test_command: None,
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
    run_with_retry(
        &mut agent,
        prompt,
        "Create ./e2e-output.txt now using a file tool. Do not explain first. Write exactly LOCAL_E2E_OK.",
    )
    .unwrap();

    let content = std::fs::read_to_string(temp.path().join("e2e-output.txt")).unwrap();
    assert_eq!(content.trim(), "LOCAL_E2E_OK");
}

#[test]
#[ignore = "requires live Ollama and optional frontend toolchain"]
fn live_ollama_multi_run_file_write_stability() {
    let host =
        env::var("ANVIL_E2E_OLLAMA_HOST").unwrap_or_else(|_| "http://127.0.0.1:11434".to_string());
    let model = env::var("ANVIL_E2E_MODEL").unwrap_or_else(|_| "qwen3:8b".to_string());
    let runs = env::var("ANVIL_E2E_RUNS")
        .ok()
        .and_then(|value| value.parse::<usize>().ok())
        .unwrap_or(3);

    let client = OllamaClient::new(host.clone()).unwrap();
    let available = client.list_models().unwrap();
    if !available.iter().any(|candidate| candidate == &model) {
        eprintln!("skip live e2e: model {model} is not installed at {host}");
        return;
    }

    for run in 0..runs {
        let temp = tempdir().unwrap();
        let config = Config {
            cwd: temp.path().to_path_buf(),
            requested_model: Some(model.clone()),
            requested_sidecar_model: None,
            ollama_host: host.clone(),
            context_budget: 24_000,
            max_iterations: 20,
            chat_timeout_secs: 300,
            chat_retries: 1,
            debug: false,
            stream: false,
            tui: false,
            watch: false,
            auto_test_command: None,
            yes_mode: true,
            fresh_session: true,
            oneshot: true,
            prompt: None,
        };
        config.ensure_state_dirs().unwrap();
        let mut agent = Agent::new(
            config,
            RuntimeModels {
                main: model.clone(),
                sidecar: None,
            },
            client.clone(),
            SessionStore::new(temp.path().join(".anvil/sessions/session.json")),
            Default::default(),
        );
        let prompt = format!(
            "Create a file at the relative path run-{run}.txt in the current workspace root. Do not use placeholders like /path/to/project. The file content must be exactly RUN_{run}_OK on one line."
        );
        let retry_prompt = format!(
            "Create ./run-{run}.txt now using a file tool. Do not explain first. Write exactly RUN_{run}_OK."
        );
        run_with_retry(&mut agent, &prompt, &retry_prompt).unwrap();
        let content = std::fs::read_to_string(temp.path().join(format!("run-{run}.txt"))).unwrap();
        assert_eq!(content.trim(), format!("RUN_{run}_OK"));
    }
}

#[test]
#[ignore = "requires live Ollama, npm, and a working Next.js toolchain"]
fn live_ollama_can_scaffold_and_start_dev_server() {
    let host =
        env::var("ANVIL_E2E_OLLAMA_HOST").unwrap_or_else(|_| "http://127.0.0.1:11434".to_string());
    let model = env::var("ANVIL_E2E_MODEL").unwrap_or_else(|_| "qwen3:8b".to_string());
    let client = OllamaClient::new(host.clone()).unwrap();
    let available = client.list_models().unwrap();
    if !available.iter().any(|candidate| candidate == &model) {
        eprintln!("skip live e2e: model {model} is not installed at {host}");
        return;
    }

    let temp = tempdir().unwrap();
    std::process::Command::new("sh")
        .args([
            "-lc",
            "npx create-next-app@latest app --ts --eslint --app --src-dir --use-npm --no-tailwind --import-alias '@/*' --yes",
        ])
        .current_dir(temp.path())
        .status()
        .unwrap();

    let cwd = temp.path().join("app");
    let config = Config {
        cwd: cwd.clone(),
        requested_model: Some(model.clone()),
        requested_sidecar_model: None,
        ollama_host: host,
        context_budget: 24_000,
        max_iterations: 10,
        chat_timeout_secs: 300,
        chat_retries: 1,
        debug: false,
        stream: false,
        tui: false,
        watch: false,
        auto_test_command: None,
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
        SessionStore::new(cwd.join(".anvil/sessions/session.json")),
        Default::default(),
    );

    let prompt = "Use the available tools to replace src/app/page.tsx with a simple page that renders the exact text LOCAL_E2E_WEB_OK. Then reply with a short sentence.";
    run_with_retry(
        &mut agent,
        prompt,
        "Edit ./src/app/page.tsx now. Replace it with a page that renders exactly LOCAL_E2E_WEB_OK.",
    )
    .unwrap();
    let page = std::fs::read_to_string(cwd.join("src/app/page.tsx")).unwrap();
    assert!(page.contains("LOCAL_E2E_WEB_OK"));

    let status = std::process::Command::new("sh")
        .args(["-lc", "npm run dev -- --port 3011 > /tmp/anvil-e2e-dev.log 2>&1 & pid=$!; sleep 8; kill $pid >/dev/null 2>&1; wait $pid >/dev/null 2>&1 || true"])
        .current_dir(&cwd)
        .status()
        .unwrap();
    assert!(status.success());
}

fn run_with_retry(agent: &mut Agent, prompt: &str, retry_prompt: &str) -> Result<(), String> {
    match agent.run_oneshot(prompt) {
        Ok(_) => Ok(()),
        Err(_) => agent.run_oneshot(retry_prompt).map(|_| ()),
    }
}
