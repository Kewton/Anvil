use std::io::{self, BufRead, Write};
use std::path::PathBuf;

use anyhow::{bail, Context};

use crate::agents::pm::PmAgent;
use crate::cli::app::{Cli, Command, HandoffAction};
use crate::cli::flags::{NetworkPolicyArg, PermissionModeArg};
use crate::cli::output::{render_session_history, render_session_snapshot, render_startup_summary};
use crate::config::repo_instructions::RepoInstructions;
use crate::roles::{EffectiveModels, RoleRegistry};
use crate::runtime::engine::RuntimeEngine;
use crate::runtime::loop_state::RuntimeLoop;
use crate::runtime::sandbox::SandboxPolicy;
use crate::runtime::{NetworkPolicy, PermissionMode};
use crate::state::handoff::HandoffFile;
use crate::state::session::{ResultRecord, SessionState};
use crate::state::store::StateStore;
use crate::tools::registry::ToolRegistry;
use crate::util::clock::session_id;
use crate::util::json::pretty;

pub fn execute(cli: Cli) -> anyhow::Result<()> {
    let registry = RoleRegistry::load_builtin().context("failed to load builtin role registry")?;
    let store = StateStore::default();

    if cli.prompt.is_some() && matches!(cli.command, Some(Command::Handoff { .. })) {
        bail!("--prompt cannot be used together with handoff commands");
    }

    match &cli.command {
        Some(Command::Resume { session_id }) => {
            let mut session = store.load_session(&registry, session_id)?;
            let models = EffectiveModels::from_session(&cli, &registry, &session)?;
            let permission_mode = cli
                .permission_mode
                .map(PermissionMode::from)
                .unwrap_or(session.permission_mode);
            let network_policy = cli
                .network_policy
                .map(NetworkPolicy::from)
                .unwrap_or(session.network_policy);
            println!("resuming session: {session_id}");
            println!("objective: {}", session.objective);
            if let Some(prompt) = &cli.prompt {
                let response = execute_prompt_turn(
                    &store,
                    &registry,
                    &mut session,
                    &models,
                    permission_mode,
                    network_policy,
                    prompt,
                )?;
                println!("prompt: {prompt}");
                println!("response: {response}");
            }
            let snapshot = render_session_snapshot(&session);
            if !snapshot.is_empty() {
                println!("{snapshot}");
            }
            println!(
                "{}",
                render_startup_summary(&models, permission_mode, network_policy)
            );
            if cli.prompt.is_none() {
                run_interactive_loop(
                    &store,
                    &registry,
                    &mut session,
                    &models,
                    permission_mode,
                    network_policy,
                )?;
            }
        }
        Some(Command::Handoff { action }) => match action {
            HandoffAction::Export { session_id } => {
                let session = store.load_session(&registry, session_id)?;
                let handoff = HandoffFile::from_session(&session, "anvil-session-export");
                println!("{}", pretty(&handoff)?);
            }
            HandoffAction::Import { file } => {
                let handoff = store.load_handoff(&registry, &PathBuf::from(file))?;
                let session = SessionState {
                    session_id: handoff.session_id.clone(),
                    pm_model: handoff.pm_model.clone(),
                    permission_mode: handoff.permission_mode,
                    network_policy: handoff.network_policy,
                    agent_models: handoff.agent_models.clone(),
                    objective: handoff.objective.clone(),
                    working_summary: handoff.working_summary.clone(),
                    user_preferences_summary: String::new(),
                    repository_summary: handoff.repository_summary.unwrap_or_default(),
                    active_constraints: handoff.active_constraints.clone(),
                    open_questions: handoff.open_questions.clone(),
                    completed_steps: handoff.completed_steps.clone(),
                    pending_steps: handoff.pending_steps.clone(),
                    relevant_files: handoff.relevant_files.clone(),
                    recent_delegations: Vec::new(),
                    recent_results: handoff
                        .recent_results
                        .into_iter()
                        .map(|result| ResultRecord {
                            role: result.role,
                            model: result.model,
                            summary: result.summary,
                            evidence: result.evidence,
                            changed_files: result.changed_files,
                            commands_run: result.commands_run,
                            next_recommendation: result.next_recommendation,
                            findings: Vec::new(),
                        })
                        .collect(),
                };
                let path = store.save_session(&registry, &session)?;
                println!("imported handoff into {}", path.display());
            }
        },
        None => {
            let models = EffectiveModels::from_cli(&cli, &registry)?;
            let permission_mode = cli
                .permission_mode
                .unwrap_or(PermissionModeArg::ReadOnly)
                .into();
            let network_policy = cli
                .network_policy
                .unwrap_or(NetworkPolicyArg::Disabled)
                .into();
            let mut session = build_session_state(&cli, &models, permission_mode, network_policy);

            if let Some(prompt) = &cli.prompt {
                let response = execute_prompt_turn(
                    &store,
                    &registry,
                    &mut session,
                    &models,
                    permission_mode,
                    network_policy,
                    prompt,
                )?;
                println!("prompt mode");
                println!("prompt: {prompt}");
                println!("response: {response}");
                println!("session: {}", session.session_id);
                let snapshot = render_session_snapshot(&session);
                if !snapshot.is_empty() {
                    println!("{snapshot}");
                }
                println!(
                    "{}",
                    render_startup_summary(&models, permission_mode, network_policy)
                );
                println!(
                    "state: {}",
                    store.session_path(&session.session_id).display()
                );
            } else {
                let path = store.save_session(&registry, &session)?;
                println!("interactive mode");
                println!("session: {}", session.session_id);
                let snapshot = render_session_snapshot(&session);
                if !snapshot.is_empty() {
                    println!("{snapshot}");
                }
                println!(
                    "{}",
                    render_startup_summary(&models, permission_mode, network_policy)
                );
                println!("state: {}", path.display());
                run_interactive_loop(
                    &store,
                    &registry,
                    &mut session,
                    &models,
                    permission_mode,
                    network_policy,
                )?;
            }
        }
    }

    Ok(())
}

fn run_interactive_loop(
    store: &StateStore,
    registry: &RoleRegistry,
    session: &mut SessionState,
    models: &EffectiveModels,
    permission_mode: PermissionMode,
    network_policy: NetworkPolicy,
) -> anyhow::Result<()> {
    println!("interactive commands: enter a prompt, or `exit` to finish");
    let session_path = store.session_path(&session.session_id);
    let stdin = io::stdin();
    let mut stdout = io::stdout();

    for line in stdin.lock().lines() {
        let prompt = line.context("failed to read interactive prompt from stdin")?;
        let trimmed = prompt.trim();
        if trimmed.is_empty() {
            continue;
        }
        if matches!(trimmed, "exit" | "quit" | "/exit" | "/quit") {
            println!("interactive mode ended");
            break;
        }
        if matches!(trimmed, "help" | ":help" | "/help") {
            println!("{}", render_interactive_help());
            writeln!(stdout, "awaiting next prompt").ok();
            stdout.flush().ok();
            continue;
        }
        if matches!(
            trimmed,
            "status" | ":status" | "/status" | "snapshot" | ":snapshot" | "/snapshot"
        ) {
            let status = render_interactive_status(session, &session_path);
            println!("{status}");
            writeln!(stdout, "awaiting next prompt").ok();
            stdout.flush().ok();
            continue;
        }
        if matches!(trimmed, "models" | ":models" | "/models") {
            println!(
                "{}",
                render_startup_summary(models, permission_mode, network_policy)
            );
            writeln!(stdout, "awaiting next prompt").ok();
            stdout.flush().ok();
            continue;
        }
        if matches!(trimmed, "history" | ":history" | "/history") {
            println!("{}", render_session_history(session));
            writeln!(stdout, "awaiting next prompt").ok();
            stdout.flush().ok();
            continue;
        }

        let response = execute_prompt_turn(
            store,
            registry,
            session,
            models,
            permission_mode,
            network_policy,
            trimmed,
        )?;
        println!("prompt: {trimmed}");
        println!("response: {response}");
        let snapshot = render_session_snapshot(session);
        if !snapshot.is_empty() {
            println!("{snapshot}");
        }
        writeln!(stdout, "awaiting next prompt").ok();
        stdout.flush().ok();
    }

    Ok(())
}

fn render_interactive_help() -> &'static str {
    "interactive commands: `/help`, `/status`, `/snapshot`, `/models`, `/history`, `/exit`"
}

fn render_interactive_status(session: &SessionState, session_path: &PathBuf) -> String {
    let mut lines = vec![
        format!("Session: {}", session.session_id),
        format!("Objective: {}", session.objective),
        format!("State: {}", session_path.display()),
    ];
    let snapshot = render_session_snapshot(session);
    if snapshot.is_empty() {
        lines.push("Session snapshot is empty".to_string());
    } else {
        lines.push(snapshot);
    }
    lines.join("\n")
}

fn execute_prompt_turn(
    store: &StateStore,
    registry: &RoleRegistry,
    session: &mut SessionState,
    models: &EffectiveModels,
    permission_mode: PermissionMode,
    network_policy: NetworkPolicy,
    prompt: &str,
) -> anyhow::Result<String> {
    let workspace_root =
        std::env::current_dir().context("failed to determine current directory")?;
    let repo_instructions = RepoInstructions::load(&workspace_root)?;
    let sandbox = SandboxPolicy::new(permission_mode, network_policy, workspace_root, vec![]);
    let engine = RuntimeEngine::new(sandbox, ToolRegistry::default(), repo_instructions);
    let context = engine.build_context(prompt, Vec::new());
    let response = RuntimeLoop::run_prompt(
        session,
        models,
        &PmAgent::default(),
        &engine,
        &context,
        prompt,
    )?;
    store.save_session(registry, session)?;
    Ok(response)
}

fn build_session_state(
    cli: &Cli,
    models: &EffectiveModels,
    permission_mode: PermissionMode,
    network_policy: NetworkPolicy,
) -> SessionState {
    let objective = cli
        .prompt
        .clone()
        .unwrap_or_else(|| "interactive session".to_string());

    SessionState {
        session_id: session_id(),
        pm_model: models.pm_model.clone(),
        permission_mode,
        network_policy,
        agent_models: models.agent_models(),
        objective: objective.clone(),
        working_summary: objective,
        user_preferences_summary: String::new(),
        repository_summary: String::new(),
        active_constraints: Vec::new(),
        open_questions: Vec::new(),
        completed_steps: Vec::new(),
        pending_steps: Vec::new(),
        relevant_files: Vec::new(),
        recent_delegations: Vec::new(),
        recent_results: Vec::new(),
    }
}
