use std::env;
use std::path::Path;
use std::process::Command;

use anvil::agent::Agent;
use anvil::agent::orchestration::{capture_repo_snapshot, verify_repo_progress};
use anvil::config::{Config, LogLevel};
use anvil::model_registry::RuntimeModels;
use anvil::ollama::client::OllamaClient;
use anvil::session::store::SessionStore;
use anvil::{compute_workspace_key, ensure_state_dirs, resolve_session_id};
use serde_json::Value;
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
    let mut agent = new_agent_with_log_level(&cwd, &host, &model, client, 10, LogLevel::Trace);
    let prompt = "Use the available tools to inspect the current app and make one concrete implementation code change. Stop after the first successful file change.";
    let _ = agent.run_oneshot(prompt);

    let verification = verify_repo_progress(&before, &cwd);
    assert!(
        verification.made_any_progress(),
        "first-write assertion: no repository changes detected. changed_files={:?}",
        verification.changed_files
    );
}

#[test]
#[ignore = "requires live Ollama"]
fn live_ollama_edit_fallback_handles_drifted_old_string() {
    let temp = tempdir().unwrap();
    let host =
        env::var("ANVIL_E2E_OLLAMA_HOST").unwrap_or_else(|_| "http://127.0.0.1:11434".to_string());
    let model = env::var("ANVIL_E2E_MODEL").unwrap_or_else(|_| "qwen3:8b".to_string());

    let Some(client) = available_client_or_skip(&host, &model).unwrap() else {
        return;
    };

    std::fs::create_dir_all(temp.path().join("src")).unwrap();
    std::fs::write(
        temp.path().join("src/main.ts"),
        "export function main() {\n  const my_special_variable = compute_result(42);\n  return my_special_variable;\n}\n",
    )
    .unwrap();

    let mut agent = new_agent(temp.path(), &host, &model, client, 8);
    let prompt = "Do not call Read first. Use the Edit tool, not Write, on src/main.ts. Replace the line `const my_special_variable = compute_result(input_value);` with `const my_special_variable = compute_result(7);`. Keep the rest of the file unchanged.";
    let retry_prompt = "Use Edit, not Write, on src/main.ts right now. Do not read first. Replace `const my_special_variable = compute_result(input_value);` with `const my_special_variable = compute_result(7);` and keep the rest unchanged.";
    run_with_retry(&mut agent, prompt, retry_prompt).unwrap();

    let content = std::fs::read_to_string(temp.path().join("src/main.ts")).unwrap();
    assert!(content.contains("compute_result(7)"));

    let session_json = find_latest_session_json(&temp.path().join(".anvil-state"));
    let session: Value =
        serde_json::from_str(&std::fs::read_to_string(session_json).unwrap()).unwrap();
    let messages = session["messages"].as_array().unwrap();
    assert!(messages.iter().any(|message| {
        message["tool_calls"]
            .as_array()
            .is_some_and(|calls| calls.iter().any(|call| call["name"] == "Edit"))
    }));
    assert!(messages.iter().any(|message| {
        message["role"] == "tool"
            && message["content"]
                .as_str()
                .is_some_and(|content| content.contains("token-anchor fallback"))
    }));
}

#[test]
#[ignore = "requires live Ollama"]
fn live_ollama_persists_working_memory_after_write() {
    let temp = tempdir().unwrap();
    let host =
        env::var("ANVIL_E2E_OLLAMA_HOST").unwrap_or_else(|_| "http://127.0.0.1:11434".to_string());
    let model = env::var("ANVIL_E2E_MODEL").unwrap_or_else(|_| "qwen3:8b".to_string());

    let Some(client) = available_client_or_skip(&host, &model).unwrap() else {
        return;
    };
    let mut agent = new_agent(temp.path(), &host, &model, client, 8);
    let prompt = "Use the available file tools to create a file named e2e-memory.txt in the current project root. The file content must be exactly MEMORY_OK on a single line.";
    run_with_retry(
        &mut agent,
        prompt,
        "Create ./e2e-memory.txt now using a file tool. Write exactly MEMORY_OK.",
    )
    .unwrap();

    let session_json = find_latest_session_json(&temp.path().join(".anvil-state"));
    let session: Value =
        serde_json::from_str(&std::fs::read_to_string(session_json).unwrap()).unwrap();
    assert_eq!(
        session["working_memory"]["active_task"].as_str(),
        Some(prompt)
    );
    let touched = session["working_memory"]["touched_files"]
        .as_array()
        .unwrap();
    assert!(touched.iter().any(|value| value == "e2e-memory.txt"));
}

#[test]
#[ignore = "requires live Ollama"]
fn live_ollama_repo_context_guides_targeted_edit() {
    let temp = tempdir().unwrap();
    let host =
        env::var("ANVIL_E2E_OLLAMA_HOST").unwrap_or_else(|_| "http://127.0.0.1:11434".to_string());
    let model = env::var("ANVIL_E2E_MODEL").unwrap_or_else(|_| "qwen3:8b".to_string());

    let Some(client) = available_client_or_skip(&host, &model).unwrap() else {
        return;
    };

    std::fs::create_dir_all(temp.path().join("src/billing")).unwrap();
    std::fs::create_dir_all(temp.path().join("src/ui")).unwrap();
    std::fs::create_dir_all(temp.path().join("docs")).unwrap();
    std::fs::write(
        temp.path().join("src/billing/retry_policy.ts"),
        "export const paymentRetryDelayMs = 3000;\nexport const paymentRetryLimit = 4;\n",
    )
    .unwrap();
    std::fs::write(
        temp.path().join("src/ui/retry_banner.ts"),
        "export const retryBannerDelayMs = 1200;\n",
    )
    .unwrap();
    std::fs::write(
        temp.path().join("docs/payment-retries.md"),
        "Payment retries are handled by the billing retry policy.\n",
    )
    .unwrap();

    let mut agent = new_agent(temp.path(), &host, &model, client, 8);
    let prompt = "Update the billing retry policy so `paymentRetryDelayMs` becomes 7000. Keep the change minimal and stop after the edit.";
    let retry_prompt = "Edit src/billing/retry_policy.ts now so `paymentRetryDelayMs` becomes 7000. Keep the rest unchanged.";
    run_with_retry(&mut agent, prompt, retry_prompt).unwrap();

    let content = std::fs::read_to_string(temp.path().join("src/billing/retry_policy.ts")).unwrap();
    assert!(content.contains("paymentRetryDelayMs = 7000"));

    let session_json = find_latest_session_json(&temp.path().join(".anvil-state"));
    let session: Value =
        serde_json::from_str(&std::fs::read_to_string(session_json).unwrap()).unwrap();
    let messages = session["messages"].as_array().unwrap();
    assert!(messages.iter().any(|message| {
        message["tool_calls"].as_array().is_some_and(|calls| {
            calls.iter().any(|call| {
                call["arguments"]["path"]
                    .as_str()
                    .is_some_and(|path| path.ends_with("src/billing/retry_policy.ts"))
            })
        })
    }));
}

#[test]
#[ignore = "requires live Ollama"]
fn live_ollama_offline_mode_still_writes_files() {
    let temp = tempdir().unwrap();
    let host =
        env::var("ANVIL_E2E_OLLAMA_HOST").unwrap_or_else(|_| "http://127.0.0.1:11434".to_string());
    let model = env::var("ANVIL_E2E_MODEL").unwrap_or_else(|_| "qwen3:8b".to_string());

    let Some(client) = available_client_or_skip(&host, &model).unwrap() else {
        return;
    };
    let mut agent = new_offline_agent(temp.path(), &host, &model, client, 8);

    let prompt = "Offline mode is enabled. Use available file tools to create offline-output.txt in the project root. The file content must be exactly OFFLINE_OK.";
    run_with_retry(
        &mut agent,
        prompt,
        "Create ./offline-output.txt now using a file tool. Write exactly OFFLINE_OK.",
    )
    .unwrap();

    let content = std::fs::read_to_string(temp.path().join("offline-output.txt")).unwrap();
    assert_eq!(content.trim(), "OFFLINE_OK");
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
    new_agent_with_options(
        cwd,
        host,
        model,
        client,
        max_iterations,
        LogLevel::Info,
        false,
    )
}

fn new_offline_agent(
    cwd: &Path,
    host: &str,
    model: &str,
    client: OllamaClient,
    max_iterations: usize,
) -> Agent {
    new_agent_with_options(
        cwd,
        host,
        model,
        client,
        max_iterations,
        LogLevel::Info,
        true,
    )
}

fn new_agent_with_log_level(
    cwd: &Path,
    host: &str,
    model: &str,
    client: OllamaClient,
    max_iterations: usize,
    log_level: LogLevel,
) -> Agent {
    new_agent_with_options(cwd, host, model, client, max_iterations, log_level, false)
}

fn new_agent_with_options(
    cwd: &Path,
    host: &str,
    model: &str,
    client: OllamaClient,
    max_iterations: usize,
    log_level: LogLevel,
    offline: bool,
) -> Agent {
    let state_root = cwd.join(".anvil-state");
    let workspace_key = compute_workspace_key(cwd);
    let session_id = resolve_session_id(&state_root, &workspace_key, true);
    ensure_state_dirs(&state_root, &session_id).unwrap();
    let mut config = Config::default();
    config.cwd = cwd.to_path_buf();
    config.requested_model = Some(model.to_string());
    config.ollama_host = host.to_string();
    config.context_budget = 24_000;
    config.max_iterations = max_iterations;
    config.chat_timeout_secs = 300;
    config.chat_retries = 1;
    config.log_level = log_level;
    config.yes_mode = true;
    config.fresh_session = true;
    config.oneshot = true;
    config.offline = offline;
    config.state_dir_override = Some(state_root.clone());

    Agent::new(
        config,
        RuntimeModels {
            main: model.to_string(),
            sidecar: None,
        },
        client,
        SessionStore::new(&state_root, &session_id, &workspace_key),
        Default::default(),
        anvil::agent::loop_run::FooterHandle::disabled(),
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

fn find_latest_session_json(state_root: &Path) -> std::path::PathBuf {
    let sessions_dir = state_root.join("sessions");
    let mut candidates = std::fs::read_dir(&sessions_dir)
        .unwrap()
        .filter_map(Result::ok)
        .map(|entry| entry.path().join("session.json"))
        .filter(|path| path.exists())
        .collect::<Vec<_>>();
    candidates.sort();
    candidates.pop().unwrap()
}
