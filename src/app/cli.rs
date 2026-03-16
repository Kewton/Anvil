//! CLI session loop and interactive input handling.
//!
//! Contains the `run_session_loop` and `run_interactive_loop` functions
//! that drive the interactive CLI experience.

use crate::config::EffectiveConfig;
use crate::logging::{LogGuard, init_tracing};
use crate::provider::{ProviderClient, ProviderRuntimeContext, build_local_provider_client};
use crate::session::SessionError;
use crate::tui::Tui;

use super::{App, AppError, SessionControl, cli_prompt};

use std::io::{self, BufRead, Write};

/// Drive the interactive CLI session loop.
///
/// Reads lines from `input`, dispatches them through [`App::handle_cli_line`],
/// and writes rendered frames to `output` until the user exits.
pub fn run_session_loop<C: ProviderClient, R: BufRead, W: Write>(
    app: &mut App,
    provider_client: &C,
    tui: &Tui,
    mut input: R,
    output: &mut W,
) -> Result<(), AppError> {
    loop {
        write!(output, "{}", cli_prompt())
            .map_err(|err| AppError::Session(SessionError::SessionWriteFailed(err)))?;
        output
            .flush()
            .map_err(|err| AppError::Session(SessionError::SessionWriteFailed(err)))?;

        let mut line = String::new();
        let read = input
            .read_line(&mut line)
            .map_err(|err| AppError::Session(SessionError::SessionReadFailed(err)))?;
        if read == 0 {
            break;
        }

        let turn = app.handle_cli_line(&line, provider_client, tui)?;
        for frame in turn.frames {
            writeln!(output, "{frame}")
                .map_err(|err| AppError::Session(SessionError::SessionWriteFailed(err)))?;
        }
        if turn.control == SessionControl::Exit {
            break;
        }
    }

    Ok(())
}

/// Application entry point.
///
/// Uses `rustyline` for interactive input, providing cursor movement,
/// line editing, and input history.
pub fn run() -> Result<(), AppError> {
    let config = EffectiveConfig::load()?;

    let _guard: Option<LogGuard> = init_tracing(
        config.mode.log_filter.as_deref(),
        config.mode.debug_logging,
        &config.paths.logs_dir,
        config.session_key(),
    );

    tracing::info!(
        provider = %config.runtime.provider,
        model = %config.runtime.model,
        context_window = config.runtime.context_window,
        debug_logging = config.mode.debug_logging,
        "anvil started with effective config"
    );

    let provider = ProviderRuntimeContext::bootstrap(&config)?;
    let provider_client = build_local_provider_client(&config)?;
    let mut app = App::new(config, provider)?;
    let tui = Tui::new();
    println!("{}", app.startup_console(&tui)?);

    if !app.config.mode.interactive {
        return Ok(());
    }

    run_interactive_loop(&mut app, &provider_client, &tui)
}

/// Interactive session loop powered by `rustyline`.
///
/// Supports arrow-key cursor movement, line editing, and input history.
/// Falls back to the `BufRead`-based [`run_session_loop`] for
/// non-interactive contexts (tests, piped input).
fn run_interactive_loop<C: ProviderClient>(
    app: &mut App,
    provider_client: &C,
    tui: &Tui,
) -> Result<(), AppError> {
    use rustyline::error::ReadlineError;

    let history_path = app.config.paths.state_dir.join("input-history.txt");
    let mut rl = rustyline::DefaultEditor::new().map_err(|err| {
        AppError::Session(SessionError::SessionReadFailed(io::Error::other(format!(
            "failed to initialize line editor: {err}"
        ))))
    })?;
    let _ = rl.load_history(&history_path);

    let prompt = cli_prompt();
    loop {
        match rl.readline(prompt) {
            Ok(line) => {
                if !line.trim().is_empty() {
                    let _ = rl.add_history_entry(&line);
                }
                let turn = app.handle_cli_line(&line, provider_client, tui)?;
                for frame in &turn.frames {
                    println!("{frame}");
                }
                if turn.control == SessionControl::Exit {
                    break;
                }
            }
            Err(ReadlineError::Interrupted | ReadlineError::Eof) => {
                break;
            }
            Err(err) => {
                return Err(AppError::Session(SessionError::SessionReadFailed(
                    io::Error::other(format!("readline error: {err}")),
                )));
            }
        }
    }

    if let Some(parent) = history_path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let _ = rl.save_history(&history_path);

    Ok(())
}
