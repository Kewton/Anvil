pub mod agent;
pub mod cli;
pub mod config;
pub mod logging;
pub mod model_registry;
pub mod modes;
pub mod ollama;
pub mod safety;
pub mod session;
pub mod system_prompt;
pub mod tools;

use std::io::{self, IsTerminal, Read};
use std::path::{Path, PathBuf};

use agent::Agent;
use cli::CliArgs;
use config::Config;
use model_registry::{RuntimeModels, select_models};
use ollama::client::OllamaClient;
use session::store::SessionStore;

pub fn run_cli(args: CliArgs) -> Result<(), String> {
    // --debug deprecation is emitted here because the flag only exists on the
    // CLI; env/config deprecations are collected inside `Config::load`.
    if args.debug {
        eprintln!(
            "warning: --debug is deprecated, use --trace (or --verbose for medium verbosity)"
        );
    }

    let (config, warnings) = Config::load(args)?;
    for warning in &warnings {
        eprintln!("warning: {warning}");
    }

    let state_root = resolve_state_root(&config)?;
    let workspace_key = compute_workspace_key(&config.cwd);
    let session_id = resolve_session_id(&state_root, &workspace_key, config.fresh_session);

    ensure_state_dirs(&state_root, &session_id)?;

    let log_path = state_root
        .join("sessions")
        .join(&session_id)
        .join("logs")
        .join("llm-io.jsonl");
    logging::init_logging(config.log_level, &log_path)?;

    let _ = symlink_anvil_dirs(&config.cwd, &state_root, &session_id);

    let client = OllamaClient::new_with_timeout_and_options(
        config.ollama_host.clone(),
        config.chat_timeout_secs,
        config.context_budget,
        2_048,
    )?;
    let available_models = client.list_models()?;
    let models = select_models(
        config.requested_model.clone(),
        config.requested_sidecar_model.clone(),
        &available_models,
        model_registry::detect_total_memory_gib(),
    );

    let session_store = SessionStore::new(&state_root, &session_id, &workspace_key);
    let session = session_store.load_or_new(config.fresh_session)?;
    let mut agent = Agent::new(config, models, client, session_store, session);

    if let Some(prompt) = agent.initial_prompt_from_cli_or_stdin()? {
        let reply = agent.run_oneshot(&prompt)?;
        if !reply.is_empty() {
            println!("{reply}");
        }
        return Ok(());
    }

    agent.run_repl()
}

pub fn stdin_prompt() -> Result<Option<String>, String> {
    if io::stdin().is_terminal() {
        return Ok(None);
    }

    let mut input = String::new();
    io::stdin()
        .read_to_string(&mut input)
        .map_err(|err| format!("failed to read stdin: {err}"))?;

    let trimmed = input.trim();
    if trimmed.is_empty() {
        Ok(None)
    } else {
        Ok(Some(trimmed.to_string()))
    }
}

pub fn format_model_banner(models: &RuntimeModels) -> String {
    match &models.sidecar {
        Some(sidecar) => format!("main={} sidecar={sidecar}", models.main),
        None => format!("main={} sidecar=disabled", models.main),
    }
}

fn xdg_state_home() -> PathBuf {
    if let Ok(val) = std::env::var("XDG_STATE_HOME") {
        PathBuf::from(val)
    } else {
        let home = std::env::var("HOME").unwrap_or_else(|_| "/tmp".into());
        PathBuf::from(home).join(".local").join("state")
    }
}

pub fn resolve_state_root(config: &Config) -> Result<PathBuf, String> {
    if let Some(ref override_path) = config.state_dir_override {
        if !override_path.is_absolute() {
            return Err(format!(
                "state-dir must be absolute: {}",
                override_path.display()
            ));
        }
        if override_path
            .components()
            .any(|c| c == std::path::Component::ParentDir)
        {
            return Err(format!(
                "state-dir must not contain '..': {}",
                override_path.display()
            ));
        }
        Ok(override_path.clone())
    } else {
        Ok(xdg_state_home().join("anvil"))
    }
}

pub fn compute_workspace_key(cwd: &Path) -> String {
    use std::collections::hash_map::DefaultHasher;
    use std::hash::{Hash, Hasher};
    let canonical = cwd.canonicalize().unwrap_or(cwd.to_path_buf());
    let mut hasher = DefaultHasher::new();
    canonical.hash(&mut hasher);
    format!("{:016x}", hasher.finish())
}

pub fn resolve_session_id(state_root: &Path, workspace_key: &str, fresh: bool) -> String {
    if fresh {
        return uuid::Uuid::now_v7().to_string();
    }

    let sessions_dir = state_root.join("sessions");
    let mut candidates: Vec<String> = Vec::new();

    if let Ok(entries) = std::fs::read_dir(&sessions_dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            // use directory name as session_id — never trust session.json content for path construction
            let dir_name = match entry.file_name().into_string() {
                Ok(n) => n,
                Err(_) => continue,
            };
            // validate directory name is a well-formed UUID
            if uuid::Uuid::parse_str(&dir_name).is_err() {
                continue;
            }
            // skip symlinks
            let Ok(meta) = path.symlink_metadata() else {
                continue;
            };
            if meta.file_type().is_symlink() || !meta.is_dir() {
                continue;
            }
            let session_json = path.join("session.json");
            if !session_json.exists() {
                continue;
            }
            if let Ok(meta) = std::fs::metadata(&session_json)
                && meta.len() > 10 * 1024 * 1024
            {
                continue;
            }
            let Ok(data) = std::fs::read_to_string(&session_json) else {
                continue;
            };
            let Ok(snap) = serde_json::from_str::<session::store::SessionSnapshot>(&data) else {
                continue;
            };
            if snap.workspace_key == workspace_key {
                candidates.push(dir_name);
            }
        }
    }

    // UUID v7 strings are lexicographically time-ordered; pick the most recent
    if let Some(latest) = candidates.into_iter().max() {
        return latest;
    }
    uuid::Uuid::now_v7().to_string()
}

pub fn ensure_state_dirs(state_root: &Path, session_id: &str) -> Result<(), String> {
    let sessions_root = state_root.join("sessions");
    let session_dir = sessions_root.join(session_id);
    let logs_dir = session_dir.join("logs");
    let plans_dir = session_dir.join("plans");

    std::fs::create_dir_all(&logs_dir)
        .map_err(|e| format!("failed to create logs dir {}: {e}", logs_dir.display()))?;
    std::fs::create_dir_all(&plans_dir)
        .map_err(|e| format!("failed to create plans dir {}: {e}", plans_dir.display()))?;

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode_700 = std::fs::Permissions::from_mode(0o700);
        // set all ancestor dirs created here to owner-only
        for dir in &[
            state_root,
            &sessions_root,
            &session_dir,
            &logs_dir,
            &plans_dir,
        ] {
            let _ = std::fs::set_permissions(dir, mode_700.clone());
        }
    }

    Ok(())
}

pub fn symlink_anvil_dirs(
    workdir: &Path,
    state_root: &Path,
    session_id: &str,
) -> Result<(), String> {
    let anvil_dir = workdir.join(".anvil");
    if !anvil_dir.exists() {
        let _ = std::fs::create_dir_all(&anvil_dir);
    }

    let session_root = state_root.join("sessions").join(session_id);
    let links = [
        ("logs", session_root.join("logs")),
        ("sessions", session_root.clone()),
        ("plans", session_root.join("plans")),
    ];

    for (name, target) in &links {
        let link = anvil_dir.join(name);
        create_symlink_best_effort(&link, target);
    }

    Ok(())
}

fn create_symlink_best_effort(link: &Path, target: &Path) {
    if let Ok(meta) = link.symlink_metadata() {
        if meta.file_type().is_symlink() {
            #[cfg(unix)]
            let _ = std::fs::remove_file(link);
            #[cfg(windows)]
            let _ = std::fs::remove_dir(link);
        } else if meta.is_dir() {
            eprintln!(
                "warn: {} is a real directory, skipping symlink creation",
                link.display()
            );
            return;
        } else {
            eprintln!(
                "warn: {} exists as a file, skipping symlink creation",
                link.display()
            );
            return;
        }
    }

    #[cfg(unix)]
    {
        if let Err(e) = std::os::unix::fs::symlink(target, link) {
            eprintln!(
                "warn: failed to create symlink {} -> {}: {e}",
                link.display(),
                target.display()
            );
        }
    }
    #[cfg(windows)]
    {
        if let Err(e) = std::os::windows::fs::symlink_dir(target, link) {
            eprintln!(
                "warn: failed to create symlink {} -> {}: {e}",
                link.display(),
                target.display()
            );
        }
    }
}
