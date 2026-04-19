use std::env;
use std::path::Path;
use std::process::Command;

use anvil::agent::Agent;
use anvil::agent::orchestration::{capture_repo_snapshot, verify_repo_progress};
use anvil::config::Config;
use anvil::model_registry::RuntimeModels;
use anvil::ollama::client::OllamaClient;
use anvil::session::store::SessionStore;
use anvil::{compute_workspace_key, ensure_state_dirs, resolve_session_id};
use tempfile::tempdir;

#[test]
#[ignore = "requires live Ollama"]
fn live_ollama_can_write_a_file() {
    let temp = tempdir().unwrap();
    let host =
        env::var("ANVIL_E2E_OLLAMA_HOST").unwrap_or_else(|_| "http://127.0.0.1:11434".to_string());
    let model = env::var("ANVIL_E2E_MODEL").unwrap_or_else(|_| "qwen3:8b".to_string());

    let Some(client) = available_client_or_skip(&host, &model).unwrap() else {
        return;
    };
    let mut agent = new_agent(temp.path(), &host, &model, client.clone(), 8);

    let prompt = "Use the available file tools to create a file named e2e-output.txt in the current project root. The file content must be exactly LOCAL_E2E_OK on a single line. After writing the file, reply with one short sentence.";
    run_with_retry(
        &mut agent,
        prompt,
        "Create ./e2e-output.txt now using a file tool. Do not explain first. Write exactly LOCAL_E2E_OK.",
    )
    .unwrap();

    let content = std::fs::read_to_string(temp.path().join("e2e-output.txt")).unwrap();
    assert_eq!(content.trim(), "LOCAL_E2E_OK");
    assert!(temp.path().join(".anvil-state/sessions").is_dir());
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

    let Some(client) = available_client_or_skip(&host, &model).unwrap() else {
        return;
    };

    for run in 0..runs {
        let temp = tempdir().unwrap();
        let mut agent = new_agent(temp.path(), &host, &model, client.clone(), 20);
        let prompt = format!(
            "Create a file at the relative path run-{run}.txt in the current workspace root. Do not use placeholders like /path/to/project. The file content must be exactly RUN_{run}_OK on one line."
        );
        let retry_prompt = format!(
            "Create ./run-{run}.txt now using a file tool. Do not explain first. Write exactly RUN_{run}_OK."
        );
        run_with_retry(&mut agent, &prompt, &retry_prompt).unwrap();
        let content = std::fs::read_to_string(temp.path().join(format!("run-{run}.txt"))).unwrap();
        assert_eq!(content.trim(), format!("RUN_{run}_OK"));
        assert!(temp.path().join(".anvil-state/sessions").is_dir());
    }
}

#[test]
#[ignore = "requires live Ollama, npm, and a working Next.js toolchain"]
fn live_ollama_can_semantically_edit_and_start_nextjs() {
    let host =
        env::var("ANVIL_E2E_OLLAMA_HOST").unwrap_or_else(|_| "http://127.0.0.1:11434".to_string());
    let model = env::var("ANVIL_E2E_MODEL").unwrap_or_else(|_| "qwen3:8b".to_string());
    let Some(client) = available_client_or_skip(&host, &model).unwrap() else {
        return;
    };

    let temp = tempdir().unwrap();
    let scaffold_status = Command::new("sh")
        .args([
            "-lc",
            "npx create-next-app@latest app --ts --eslint --app --src-dir --use-npm --no-tailwind --import-alias '@/*' --yes",
        ])
        .current_dir(temp.path())
        .status()
        .unwrap();
    assert!(scaffold_status.success());

    let cwd = temp.path().join("app");
    let mut agent = new_agent(&cwd, &host, &model, client, 12);
    let prompt = "Use the available tools to replace src/app/page.tsx with a simple page that renders the exact text LOCAL_E2E_WEB_OK. Finish the file edit before replying.";
    run_with_retry(
        &mut agent,
        prompt,
        &format!(
            "Edit the file {} now. Replace it with a page that renders exactly LOCAL_E2E_WEB_OK. Do not stop after inspection.",
            cwd.join("src/app/page.tsx").display()
        ),
    )
    .unwrap();

    let page_path = cwd.join("src/app/page.tsx");
    let page = std::fs::read_to_string(&page_path).unwrap();
    assert_semantic_page_contents(&page, "LOCAL_E2E_WEB_OK");
    assert!(cwd.join(".anvil-state/sessions").is_dir());

    let status = Command::new("sh")
        .args(["-lc", "npm run dev -- --port 3011 > /tmp/anvil-e2e-dev.log 2>&1 & pid=$!; sleep 8; kill $pid >/dev/null 2>&1; wait $pid >/dev/null 2>&1 || true"])
        .current_dir(&cwd)
        .status()
        .unwrap();
    assert!(status.success());
}

#[test]
#[ignore = "requires live Ollama, npm, and a working Next.js toolchain"]
fn live_ollama_reaches_first_write_on_scaffolded_nextjs() {
    let host =
        env::var("ANVIL_E2E_OLLAMA_HOST").unwrap_or_else(|_| "http://127.0.0.1:11434".to_string());
    let model = env::var("ANVIL_E2E_MODEL").unwrap_or_else(|_| "qwen3:8b".to_string());
    let Some(client) = available_client_or_skip(&host, &model).unwrap() else {
        return;
    };

    let temp = tempdir().unwrap();
    let scaffold_status = Command::new("sh")
        .args([
            "-lc",
            "npx create-next-app@latest app --ts --eslint --app --src-dir --use-npm --no-tailwind --import-alias '@/*' --yes",
        ])
        .current_dir(temp.path())
        .status()
        .unwrap();
    assert!(scaffold_status.success());

    let cwd = temp.path().join("app");
    let before = capture_repo_snapshot(&cwd);
    let mut agent = new_agent_with_debug(&cwd, &host, &model, client, 10, true);
    let prompt = "Use the available tools to inspect the current app and make one concrete implementation code change. Stop after the first successful file change.";
    let _ = agent.run_oneshot(prompt);

    let verification = verify_repo_progress(&before, &cwd);
    assert!(
        verification.made_any_progress(),
        "first-write assertion: no repository changes detected. changed_files={:?}",
        verification.changed_files
    );
}

fn available_client_or_skip(host: &str, model: &str) -> Result<Option<OllamaClient>, String> {
    let client = OllamaClient::new(host.to_string())?;
    let available = client.list_models()?;
    if available.iter().any(|candidate| candidate == model) {
        Ok(Some(client))
    } else {
        eprintln!("skip live e2e: model {model} is not installed at {host}");
        Ok(None)
    }
}

fn new_agent(
    cwd: &Path,
    host: &str,
    model: &str,
    client: OllamaClient,
    max_iterations: usize,
) -> Agent {
    new_agent_with_debug(cwd, host, model, client, max_iterations, false)
}

fn new_agent_with_debug(
    cwd: &Path,
    host: &str,
    model: &str,
    client: OllamaClient,
    max_iterations: usize,
    debug: bool,
) -> Agent {
    let state_root = cwd.join(".anvil-state");
    let workspace_key = compute_workspace_key(cwd);
    let session_id = resolve_session_id(&state_root, &workspace_key, true);
    ensure_state_dirs(&state_root, &session_id).unwrap();
    let config = Config {
        cwd: cwd.to_path_buf(),
        requested_model: Some(model.to_string()),
        requested_sidecar_model: None,
        ollama_host: host.to_string(),
        context_budget: 24_000,
        max_iterations,
        chat_timeout_secs: 300,
        chat_retries: 1,
        debug,
        stream: false,
        yes_mode: true,
        fresh_session: true,
        oneshot: true,
        prompt: None,
        state_dir_override: Some(state_root.clone()),
    };

    Agent::new(
        config,
        RuntimeModels {
            main: model.to_string(),
            sidecar: None,
        },
        client,
        SessionStore::new(&state_root, &session_id, &workspace_key),
        Default::default(),
    )
}

fn run_with_retry(agent: &mut Agent, prompt: &str, retry_prompt: &str) -> Result<(), String> {
    match agent.run_oneshot(prompt) {
        Ok(_) => Ok(()),
        Err(_) => agent.run_oneshot(retry_prompt).map(|_| ()),
    }
}

fn assert_semantic_page_contents(page: &str, marker: &str) {
    assert!(page.contains(marker));
    assert!(!page.contains("Get started by editing"));
    assert!(!page.contains("next/font/google"));
}
