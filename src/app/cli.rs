//! CLI session loop and interactive input handling.
//!
//! Contains the `run_session_loop` and `run_interactive_loop` functions
//! that drive the interactive CLI experience.

use crate::config::{CliArgs, EffectiveConfig, PromptSource};
use crate::logging::{LogGuard, init_tracing};
use crate::provider::{
    ProviderClient, ProviderRuntimeContext, ProviderTurnError, build_local_provider_client,
};
use crate::session::SessionError;
use crate::tui::Tui;

use super::{App, AppError, SessionControl, cli_prompt, error_guidance};

use std::io::{self, BufRead, Read as _, Write};
use std::sync::Arc;
use std::sync::atomic::AtomicBool;

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

/// Initialize signal handlers for graceful shutdown.
///
/// Registers SIGTERM (always) and SIGINT (non-interactive mode only).
/// In interactive mode, SIGINT is handled by rustyline.
fn setup_shutdown_handler(interactive: bool) -> Arc<AtomicBool> {
    use signal_hook::consts::{SIGINT, SIGTERM};
    use signal_hook::flag;

    let shutdown_flag = Arc::new(AtomicBool::new(false));

    // SIGTERM is always registered
    if let Err(e) = flag::register(SIGTERM, Arc::clone(&shutdown_flag)) {
        eprintln!("Warning: failed to register SIGTERM handler: {e}");
    }

    // In non-interactive mode, also register SIGINT
    if !interactive && let Err(e) = flag::register(SIGINT, Arc::clone(&shutdown_flag)) {
        eprintln!("Warning: failed to register SIGINT handler: {e}");
    }

    shutdown_flag
}

/// Production entry point: parse pre-built CLI args into config, then run.
pub fn run_with_args(cli: &CliArgs) -> Result<(), AppError> {
    let config = EffectiveConfig::load_with_args(cli)?;
    run_with_config(config)
}

/// Test-compatible entry point (no CliArgs required).
///
/// Uses `EffectiveConfig::load()` which falls back gracefully when
/// `std::env::args()` contains test-harness arguments.
pub fn run() -> Result<(), AppError> {
    let config = EffectiveConfig::load()?;
    run_with_config(config)
}

/// If the provider is Ollama and the user did not explicitly set
/// `context_window`, query the model's actual context length via
/// `/api/show` and apply it.
fn auto_detect_and_apply_context_window(
    config: &mut EffectiveConfig,
    provider: &ProviderRuntimeContext,
) {
    use crate::provider::{ProviderBackend, fetch_context_length_from_ollama};

    if provider.backend != ProviderBackend::Ollama {
        return;
    }
    if config.runtime.context_window_explicitly_set {
        return;
    }

    if let Some(detected) =
        fetch_context_length_from_ollama(&config.runtime.provider_url, &config.runtime.model)
    {
        eprintln!(
            "Auto-detected context_window={detected} from Ollama model '{}'",
            config.runtime.model
        );
        config.runtime.context_window = detected;
        config.clamp_context_window();
        config.clamp_context_budget();
    }
}

/// Common startup logic shared by `run_with_args` and `run`.
fn run_with_config(mut config: EffectiveConfig) -> Result<(), AppError> {
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

    // Setup shutdown handler before config is moved
    let shutdown_flag = setup_shutdown_handler(config.mode.interactive);

    let provider = ProviderRuntimeContext::bootstrap(&config)?;

    // Auto-detect context_window from Ollama if not explicitly set.
    auto_detect_and_apply_context_window(&mut config, &provider);

    let provider_client = build_local_provider_client(&config, Arc::clone(&shutdown_flag))?;

    // Health check: staged error handling based on error type.
    match provider_client.health_check() {
        Ok(()) => {}
        Err(ref err @ ProviderTurnError::ConnectionRefused(_))
        | Err(ref err @ ProviderTurnError::DnsFailure(_)) => {
            eprintln!("Error: {err}");
            eprintln!("{}", error_guidance(&AppError::ProviderTurn(err.clone())));
            return Err(AppError::ProviderTurn(err.clone()));
        }
        Err(ref err @ ProviderTurnError::AuthenticationFailed { .. }) => {
            if config.runtime.api_key.is_some() {
                // api_key configured -> authentication problem -> error exit
                eprintln!("Error: {err}");
                eprintln!("Hint: Your API key appears to be invalid. Verify the key.");
                return Err(AppError::ProviderTurn(err.clone()));
            } else {
                // api_key not configured -> LM Studio etc. possibility -> warn and continue
                eprintln!("Warning: {err}");
                eprintln!("Hint: If this server requires an API key, set ANVIL_API_KEY.");
                eprintln!("      If using LM Studio or similar, verify the server URL.");
            }
        }
        Err(ref err) => {
            // Timeout, ServerError etc. -> warn and continue
            eprintln!("Warning: {err}");
        }
    }

    let mut app = App::new(config, provider, Arc::clone(&shutdown_flag))?;

    match app.config.mode.prompt_source {
        PromptSource::Interactive => {
            let tui = Tui::new();
            println!("{}", app.startup_console(&tui)?);
            run_interactive_loop(&mut app, &provider_client, &tui)
        }
        ref source => {
            let source = source.clone();
            run_non_interactive(&mut app, &provider_client, &source)
        }
    }
}

/// Read all of stdin into a string.
fn read_stdin() -> Result<String, AppError> {
    let mut buf = String::new();
    io::stdin().lock().read_to_string(&mut buf).map_err(|e| {
        AppError::Config(crate::config::ConfigError::ValidationError(format!(
            "failed to read stdin: {e}"
        )))
    })?;
    Ok(buf)
}

/// Non-interactive execution path for --exec, --exec-file, and --oneshot modes.
fn run_non_interactive<C: ProviderClient>(
    app: &mut App,
    provider_client: &C,
    source: &PromptSource,
) -> Result<(), AppError> {
    // 1. Get prompt from the appropriate source
    let prompt = match source {
        PromptSource::Stdin => read_stdin()?,
        PromptSource::Exec(s) => s.clone(),
        PromptSource::ExecFile(path) => std::fs::read_to_string(path).map_err(|e| {
            AppError::Config(crate::config::ConfigError::ValidationError(format!(
                "failed to read exec file: {e}"
            )))
        })?,
        PromptSource::Interactive => unreachable!(),
    };

    // 2. Execute the prompt via run_live_turn
    let tui = Tui::new();
    match app.run_live_turn(&prompt, provider_client, &tui) {
        Ok(_frames) => {
            // 3. Output the last assistant message to stdout
            if let Some(response) = app.session().last_assistant_message() {
                print!("{}", response);
            }

            // Run PostSession hook (DR3-004: after run_live_turn success)
            app.run_post_session_hook();

            // 4. Check for provider errors (e.g. ModelNotFound) that
            //    run_live_turn converted to AgentEvent::Failed
            if let Some(record) = app.last_provider_error() {
                return Err(AppError::ProviderTurn(
                    ProviderTurnError::from_error_record(record),
                ));
            }

            // 5. Check for tool execution failures
            if app.has_tool_execution_failure() {
                return Err(AppError::ToolExecution("tool execution failed".to_string()));
            }

            Ok(())
        }
        Err(err) => {
            // DR3-004: Run PostSession hook on error paths after run_live_turn
            app.run_post_session_hook();
            Err(err)
        }
    }
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
        // Check shutdown flag before readline
        if app.is_shutdown_requested() {
            break;
        }

        match rl.readline(prompt) {
            Ok(line) => {
                // Check shutdown flag after readline
                if app.is_shutdown_requested() {
                    break;
                }
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

    // Run PostSession hook before saving
    app.run_post_session_hook();

    // Save session on exit
    app.save_session_on_exit();

    if let Some(parent) = history_path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let _ = rl.save_history(&history_path);

    Ok(())
}
