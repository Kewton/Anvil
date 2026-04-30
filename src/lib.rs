pub mod agent;
pub mod cli;
pub mod config;
pub mod logging;
pub mod model_registry;
pub mod modes;
pub mod ollama;
pub mod repo_graph;
pub mod safety;
pub mod session;
pub mod system_prompt;
pub mod tools;
pub mod tui;
pub mod util;

use std::io::{self, IsTerminal, Read};
use std::path::{Path, PathBuf};

use agent::Agent;
use agent::loop_run::FooterLease;
use agent::loop_run::commands::{
    print_startup_banner, print_startup_banner_stderr_oneshot, short_id,
};
use cli::{CliArgs, Command};
use config::Config;
use model_registry::{RuntimeModels, select_models};
use ollama::client::OllamaClient;
use session::compact::find_last_user_prompt;
use session::sessions_cli;
use session::store::{SessionStore, reconcile_resume_state};

pub fn run_cli(args: CliArgs) -> Result<(), String> {
    // --debug deprecation is emitted here because the flag only exists on the
    // CLI; env/config deprecations are collected inside `Config::load`.
    if args.debug {
        eprintln!(
            "warning: --debug is deprecated, use --trace (or --verbose for medium verbosity)"
        );
    }

    // CLI-level mutual-exclusion checks that clap cannot express declaratively.
    args.validate()?;

    // Short-circuit for `anvil sessions ...` BEFORE loading Ollama / Agent so
    // session inspection works offline and without an LLM running.
    if let Some(Command::Sessions { action }) = args.command.clone() {
        // We still need cwd-derived state_root + workspace_key to scope the
        // sessions view; log level / model / Ollama host are deliberately
        // not consulted.
        let cwd = match args.cwd.clone() {
            Some(path) => path,
            None => {
                std::env::current_dir().map_err(|err| format!("failed to resolve cwd: {err}"))?
            }
        };
        // CB-003 fix: canonicalize the workspace root once here and pass it
        // explicitly to dispatch so tmp-tests `promote` writes its output
        // relative to the user-declared workspace (`--cwd <path>`) rather
        // than the process's `current_dir()`. Falls back to the raw cwd if
        // canonicalize fails (e.g. non-existent path) so existing error
        // messages from downstream layers still surface.
        let workspace_root = std::fs::canonicalize(&cwd).unwrap_or_else(|_| cwd.clone());
        let state_root = resolve_state_root_from_parts(args.state_dir.as_deref())?;
        let workspace_key = compute_workspace_key(&cwd);
        return sessions_cli::dispatch(&state_root, &workspace_key, &workspace_root, action);
    }

    let (config, warnings) = Config::load(args)?;
    for warning in &warnings {
        eprintln!("warning: {warning}");
    }

    let state_root = resolve_state_root(&config)?;
    let workspace_key = compute_workspace_key(&config.cwd);

    // Explicit `--resume <ID>` takes a different code path that refuses
    // anything that is not a UUID v7 directory under state_root/sessions.
    let session_id = match config.resume.explicit_id() {
        Some(id) => {
            sessions_cli::validate_explicit_session_id(&state_root, id)?;
            id.to_string()
        }
        None => resolve_session_id(&state_root, &workspace_key, config.fresh_session),
    };

    ensure_state_dirs(&state_root, &session_id)?;

    let log_path = state_root
        .join("sessions")
        .join(&session_id)
        .join("logs")
        .join("llm-io.jsonl");
    logging::init_logging(config.log_level, &log_path)?;

    // Issue #471: structured eval log. Failure is warn-only (DR3-002).
    let eval_log_path = state_root
        .join("sessions")
        .join(&session_id)
        .join("logs")
        .join("eval.jsonl");
    if let Err(err) = session::eval_log::init_eval_log(&eval_log_path) {
        eprintln!("warning: {err}");
    }

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
    let mut session = session_store.load_or_new(config.fresh_session)?;

    // Explicit --resume <ID>: refuse foreign workspace sessions. We only
    // check here, after the snapshot is loaded, because workspace_key lives
    // inside the snapshot (not the path).
    if config.resume.explicit_id().is_some()
        && !session.workspace_key.is_empty()
        && session.workspace_key != workspace_key
    {
        return Err(format!(
            "session {session_id} belongs to a different workspace; --resume cannot cross workspace"
        ));
    }

    // Clean up broken references so the agent does not try to cd into a
    // missing active_root or resume a deleted plan.md.
    reconcile_resume_state(&mut session, &config.cwd);

    // session.json is user-editable; re-apply add_precaution canonicalization
    // (raw cap / mask / truncate / path normalize / id / unknown-status) to
    // any precautions deserialized from disk. Issue #451 design judgment #14.
    session
        .working_memory
        .sanitize_active_precautions_after_load(&config.cwd);

    let is_resume = config.resume.is_some();
    let is_oneshot = config.oneshot;
    let model_banner = format_model_banner(&models);
    let version = env!("CARGO_PKG_VERSION");
    let session_short = short_id(&session.id).to_string();
    let messages_count = session.messages.len();
    let work_root_for_banner = session
        .active_root
        .clone()
        .unwrap_or_else(|| config.cwd.clone());
    let mode_for_banner = session.mode_state.mode;
    let fresh = config.fresh_session;
    let log_level = config.log_level;

    // Resume path has to read the last user prompt before we hand the
    // snapshot to Agent::new (which consumes it by value).
    let resume_prompt: Option<String> = if is_resume {
        Some(find_last_user_prompt(&session.messages).ok_or_else(|| {
            "cannot resume: the last user message has already been collapsed into a \
             compaction summary. Start a new prompt instead."
                .to_string()
        })?)
    } else {
        None
    };

    // Acquire the fixed-footer lease before constructing `Agent` so the
    // handle can be plumbed into the agent. Phase A: `acquire` always
    // returns a disabled handle (cargo non-TTY harness short-circuits and
    // the install path is itself still skeleton-only), so the lease is
    // safe to take on every non-sessions path. AC9's strict zero-acquire
    // for the oneshot path lands in Phase C alongside the real worker
    // install (issue #430). The lease drops at the end of `run_cli`.
    // `_footer_lease` (underscore-prefixed but NOT bare `_`) keeps the lease
    // alive for the full `run_cli` scope; bare `_` would drop immediately.
    let _footer_lease = FooterLease::acquire(&config);
    let footer_handle = _footer_lease.handle_clone();

    let mut agent = Agent::new(
        config,
        models,
        client,
        session_store,
        session,
        footer_handle,
    );

    // Banner: REPL / resume get stdout; oneshot gets stderr so stdout stays
    // clean for script consumers.
    if is_oneshot {
        print_startup_banner_stderr_oneshot(&session_short, messages_count, fresh, is_resume);
    } else {
        print_startup_banner(
            version,
            &model_banner,
            &mode_for_banner,
            &work_root_for_banner,
            &session_short,
            messages_count,
            fresh,
            is_resume,
            log_level,
        );
    }

    if let Some(prompt) = resume_prompt {
        // Resume: replay the last user turn, then fall into the REPL loop
        // (banner has already been printed above).
        return agent.run_resume(&prompt);
    }

    if let Some(prompt) = agent.initial_prompt_from_cli_or_stdin()? {
        let reply = agent.run_oneshot(&prompt)?;
        if !reply.is_empty() {
            println!("{reply}");
        }
        return Ok(());
    }

    agent.run_repl_loop()
}

/// Lightweight state-root resolver for the `sessions` subcommand path, where
/// we don't construct a full `Config`. Mirrors `resolve_state_root` but takes
/// just the CLI override so sessions commands can run without Ollama/Agent.
fn resolve_state_root_from_parts(state_dir_override: Option<&Path>) -> Result<PathBuf, String> {
    if let Some(override_path) = state_dir_override {
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
        return Ok(override_path.to_path_buf());
    }
    if let Ok(env_dir) = std::env::var("ANVIL_STATE_DIR") {
        let p = PathBuf::from(env_dir);
        if !p.is_absolute() || p.components().any(|c| c == std::path::Component::ParentDir) {
            return Err("ANVIL_STATE_DIR must be an absolute path without '..'".to_string());
        }
        return Ok(p);
    }
    Ok(xdg_state_home().join("anvil"))
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

    // iter_session_dirs centralizes UUID/symlink/size/parse defenses;
    // here we only keep entries whose persisted workspace_key matches.
    let latest = session::discovery::iter_session_dirs(state_root)
        .into_iter()
        .filter(|entry| entry.snapshot.workspace_key == workspace_key)
        .map(|entry| entry.id)
        // UUID v7 strings are lexicographically time-ordered; take the max.
        .max();

    latest.unwrap_or_else(|| uuid::Uuid::now_v7().to_string())
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
    if !anvil_dir.is_dir() {
        return Ok(());
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

#[cfg(test)]
mod tests {
    use super::{ensure_state_dirs, symlink_anvil_dirs};
    use tempfile::tempdir;

    #[test]
    fn symlink_anvil_dirs_does_not_create_anvil_dir_implicitly() {
        let workdir = tempdir().unwrap();
        let state_root = tempdir().unwrap();
        let session_id = "019dbb24-uat";
        ensure_state_dirs(state_root.path(), session_id).unwrap();

        symlink_anvil_dirs(workdir.path(), state_root.path(), session_id).unwrap();

        assert!(!workdir.path().join(".anvil").exists());
    }

    #[test]
    fn symlink_anvil_dirs_populates_existing_legacy_anvil_dir() {
        let workdir = tempdir().unwrap();
        let state_root = tempdir().unwrap();
        let session_id = "019dbb24-uat";
        ensure_state_dirs(state_root.path(), session_id).unwrap();
        std::fs::create_dir_all(workdir.path().join(".anvil")).unwrap();

        symlink_anvil_dirs(workdir.path(), state_root.path(), session_id).unwrap();

        let anvil_dir = workdir.path().join(".anvil");
        assert!(anvil_dir.join("logs").exists());
        assert!(anvil_dir.join("plans").exists());
        assert!(anvil_dir.join("sessions").exists());
    }
}
