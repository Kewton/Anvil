use super::slash_commands::{self, AnvilEditor, build_editor};
use super::summary::{ExitReason, format_run_summary};
use super::*;
use crate::config::LogLevel;

/// First 8 characters of a session id, or the full id if shorter. Used for
/// banner display so users get a stable short handle without exposing the
/// whole UUID.
pub fn short_id(id: &str) -> &str {
    id.get(..8).unwrap_or(id)
}

/// Minimum terminal width (columns) required before the ASCII-art banner is
/// shown. Narrow terminals fall back to the legacy 4-line plain-text banner.
const MIN_BANNER_WIDTH: u16 = 50;

/// 256-color ANSI color codes applied to each of the five ASCII-art lines.
/// Length is pinned to 5 so the compiler guarantees one color per art line
/// (see [`ANVIL_ASCII_ART`]).
const NEON_GRADIENT_256: [u8; 5] = [51, 39, 93, 201, 21];

/// 5-line static ASCII-art logo shown at the top of the neon / mono-art
/// banners. Width is kept at or below 42 columns so that `MIN_BANNER_WIDTH`
/// (50) always leaves horizontal padding.
const ANVIL_ASCII_ART: [&str; 5] = [
    "                                          ",
    "  ╔═╗ ╔╗╔ ╦  ╦ ╦ ╦                        ",
    "  ╠═╣ ║║║ ╚╗╔╝ ║ ║     local-first agent  ",
    "  ╩ ╩ ╝╚╝  ╚╝  ╩ ╩═╝                      ",
    "                                          ",
];

/// Which banner variant to render. The three styles are mutually exclusive and
/// picked by [`decide_banner_style`] at startup.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum BannerStyle {
    /// ASCII art + 256-color ANSI gradient (interactive main path).
    Neon,
    /// ASCII art without any ANSI escapes (TTY + `NO_COLOR` + wide enough).
    MonoArt,
    /// Original 4-line plain-text banner, byte-identical with the pre-#425
    /// output. Used for non-TTY, unknown size, or narrow terminals.
    Legacy4Line,
}

/// Pure branch-decision: which banner style to render for the given runtime
/// signals. `width = None` collapses both the non-TTY case and the
/// `crossterm::terminal::size()` error case into `Legacy4Line` — if a future
/// requirement needs to distinguish them, split `width` into an explicit
/// `TerminalWidth { NonTty, SizeErr, Known(u16) }` enum.
pub(crate) fn decide_banner_style(tty: bool, no_color: bool, width: Option<u16>) -> BannerStyle {
    if !tty {
        return BannerStyle::Legacy4Line;
    }
    match width {
        None => BannerStyle::Legacy4Line,
        Some(w) if w < MIN_BANNER_WIDTH => BannerStyle::Legacy4Line,
        Some(_) if no_color => BannerStyle::MonoArt,
        Some(_) => BannerStyle::Neon,
    }
}

/// Map the `(fresh, resumed)` flag pair to the `[state]` suffix shared between
/// the stdout banner and the oneshot stderr banner. Kept in a single place so
/// AC-6 (three fixed labels) stays in sync across both paths.
pub(crate) fn state_suffix(fresh: bool, resumed: bool) -> &'static str {
    if fresh {
        "fresh"
    } else if resumed {
        "resumed"
    } else {
        "continued"
    }
}

/// Returns true when the `NO_COLOR` environment variable is set to a non-empty
/// value (https://no-color.org/). Intentionally duplicated from
/// `turn::no_color_requested` so that `commands.rs` does not force a cross-
/// module dependency; a test verifies the two implementations agree under the
/// same environment.
fn banner_no_color_requested() -> bool {
    std::env::var_os("NO_COLOR").is_some_and(|v| !v.is_empty())
}

/// Replace control characters (C0, DEL, and C1) with spaces, then trim trailing
/// whitespace. Mirrors `turn::sanitize_for_progress` so that model-derived
/// text (model banner, cwd, log path) cannot inject newlines or ANSI escapes
/// into the startup banner output. C1 (`U+0080..U+009F`) is included because
/// some terminals interpret 8-bit CSI (`U+009B`) and OSC (`U+009D`) equivalently
/// to `ESC [` and `ESC ]`.
fn sanitize_banner_text(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for ch in s.chars() {
        let cp = ch as u32;
        if cp < 0x20 || cp == 0x7F || (0x80..=0x9F).contains(&cp) {
            out.push(' ');
        } else {
            out.push(ch);
        }
    }
    out.trim_end().to_string()
}

/// Inputs for the pure [`render_startup_banner`] function. The wrapper
/// `print_startup_banner` collects runtime state (TTY, width, NO_COLOR, log
/// level) and feeds this struct to the renderer. `llm_log_path` is already
/// gated by the wrapper: `Some` ⇒ emit the `llm log=` line, `None` ⇒ skip it.
pub(crate) struct BannerInputs<'a> {
    pub version: &'a str,
    pub model_banner: &'a str,
    pub mode: &'a crate::modes::plan_act::ExecutionMode,
    pub work_root: &'a std::path::Path,
    pub session_id_short: &'a str,
    pub message_count: usize,
    pub fresh: bool,
    pub resumed: bool,
    pub llm_log_path: Option<&'a std::path::Path>,
    pub style: BannerStyle,
}

/// Pure banner renderer. Returns the complete newline-terminated banner
/// string. ANSI escapes are only ever emitted for the fixed ASCII-art lines
/// when `style == Neon`; all dynamic text is run through `sanitize_banner_text`
/// first to prevent terminal injection.
pub(crate) fn render_startup_banner(inputs: &BannerInputs<'_>) -> String {
    let mut out = String::new();

    match inputs.style {
        BannerStyle::Neon => {
            for (i, line) in ANVIL_ASCII_ART.iter().enumerate() {
                let code = NEON_GRADIENT_256[i];
                out.push_str(&format!("\x1b[38;5;{code}m{line}\x1b[0m\n"));
            }
        }
        BannerStyle::MonoArt => {
            for line in ANVIL_ASCII_ART.iter() {
                out.push_str(line);
                out.push('\n');
            }
        }
        BannerStyle::Legacy4Line => {}
    }

    let safe_model = sanitize_banner_text(inputs.model_banner);
    let safe_cwd = sanitize_banner_text(&inputs.work_root.display().to_string());
    let state = state_suffix(inputs.fresh, inputs.resumed);

    out.push_str(&format!("anvil {}\n", inputs.version));
    out.push_str(&format!("{safe_model}\n"));
    out.push_str(&format!("mode={:?} cwd={}\n", inputs.mode, safe_cwd));
    out.push_str(&format!(
        "session={} messages={} [{}]\n",
        inputs.session_id_short, inputs.message_count, state
    ));
    if let Some(path) = inputs.llm_log_path {
        let safe_path = sanitize_banner_text(&path.display().to_string());
        out.push_str(&format!("llm log={safe_path}\n"));
    }

    out
}

/// Print the REPL / resume startup banner to stdout. The `fresh` and
/// `resumed` flags drive the `[state]` suffix so users can tell at a glance
/// which session they are in. This is a thin I/O wrapper around the pure
/// [`render_startup_banner`] renderer: it collects TTY / NO_COLOR / width
/// signals, gates the optional `llm log=` line on `log_level >= Verbose`, and
/// writes the result in a single `print!` call.
#[allow(clippy::too_many_arguments)]
pub fn print_startup_banner(
    version: &str,
    model_banner: &str,
    mode: &crate::modes::plan_act::ExecutionMode,
    work_root: &std::path::Path,
    session_id_short: &str,
    message_count: usize,
    fresh: bool,
    resumed: bool,
    log_level: LogLevel,
) {
    use std::io::IsTerminal;
    let tty = std::io::stdout().is_terminal();
    let no_color = banner_no_color_requested();
    // Skip the ioctl when we know we aren't on a TTY; the result would collapse
    // to `Legacy4Line` either way.
    let width = if tty {
        crossterm::terminal::size().ok().map(|(w, _)| w)
    } else {
        None
    };
    let style = decide_banner_style(tty, no_color, width);
    let llm_log_path: Option<&std::path::Path> = if log_level >= LogLevel::Verbose {
        crate::logging::llm_io_log_path()
    } else {
        None
    };
    let inputs = BannerInputs {
        version,
        model_banner,
        mode,
        work_root,
        session_id_short,
        message_count,
        fresh,
        resumed,
        llm_log_path,
        style,
    };
    print!("{}", render_startup_banner(&inputs));
}

/// Oneshot startup banner: single stderr line so script output on stdout stays
/// clean.
pub fn print_startup_banner_stderr_oneshot(
    session_id_short: &str,
    message_count: usize,
    fresh: bool,
    resumed: bool,
) {
    let state = state_suffix(fresh, resumed);
    eprintln!("anvil session={session_id_short} messages={message_count} [{state}]");
}

impl Agent {
    pub fn initial_prompt_from_cli_or_stdin(&self) -> Result<Option<String>, String> {
        if let Some(prompt) = &self.config.prompt {
            return Ok(Some(prompt.clone()));
        }
        if self.config.oneshot {
            return stdin_prompt();
        }
        stdin_prompt()
    }

    pub fn run_oneshot(&mut self, prompt: &str) -> Result<String, String> {
        match self.process_line(prompt, false)? {
            AgentEvent::Continue(Some(message)) => Ok(message),
            AgentEvent::Continue(None) => Ok(String::new()),
            AgentEvent::Exit => Ok(String::new()),
        }
    }

    /// Replay the last user turn and then continue in the REPL loop.
    /// Used by `--resume` / `--resume <ID>` after the session has been loaded
    /// and reconciled. The caller is responsible for printing the startup
    /// banner; this method does not re-print it.
    pub fn run_resume(&mut self, replay_prompt: &str) -> Result<(), String> {
        match self.process_line(replay_prompt, self.config.stream)? {
            AgentEvent::Continue(Some(message)) => {
                if !self.config.stream {
                    println!("{message}");
                }
            }
            AgentEvent::Continue(None) => {}
            AgentEvent::Exit => return Ok(()),
        }
        self.run_repl_loop()
    }

    /// REPL body without the startup banner. Separated from `run_repl` so that
    /// `run_cli` can print the banner once (in a single location) and have
    /// both the fresh-REPL and resumed-REPL paths share the same loop.
    ///
    /// Delegates to either the rustyline-powered path (when stdin/stdout are
    /// both TTYs) or the plain `read_line` fallback (for pipes / CI / redirect).
    ///
    /// # Preconditions
    ///
    /// This function assumes it is called via `run_cli`, which has already
    /// invoked `crate::ensure_state_dirs` to materialize `state_root` with the
    /// correct 0o700 permissions. `prepare_editor()` still re-creates the
    /// directory defensively (`fs::create_dir_all(state_root)`) so direct
    /// callers that forgot to run `ensure_state_dirs` don't silently lose
    /// history persistence, but they also won't get the SSOT permission
    /// guarantees. Prefer `run_cli` for all new entry points.
    pub fn run_repl_loop(&mut self) -> Result<(), String> {
        let is_tty = io::stdin().is_terminal() && io::stdout().is_terminal();
        if !is_tty {
            return self.run_repl_loop_fallback();
        }
        self.run_repl_loop_rustyline()
    }

    /// Plain `read_line` REPL kept for non-TTY contexts (pipe input, CI,
    /// redirected stdout). Behaviorally identical to the pre-#427 loop so
    /// `echo foo | anvil` and friends keep working unchanged.
    fn run_repl_loop_fallback(&mut self) -> Result<(), String> {
        let mut line = String::new();
        loop {
            print!("anvil> ");
            io::stdout()
                .flush()
                .map_err(|err| format!("failed to flush stdout: {err}"))?;
            line.clear();
            let bytes = io::stdin()
                .read_line(&mut line)
                .map_err(|err| format!("failed to read line: {err}"))?;
            if bytes == 0 {
                break;
            }
            match self.process_line(line.trim(), self.config.stream)? {
                AgentEvent::Continue(Some(message)) => println!("{message}"),
                AgentEvent::Continue(None) => {}
                AgentEvent::Exit => break,
            }
        }
        Ok(())
    }

    /// Rustyline-powered REPL: persistent history + tab-completed slash
    /// commands + Ctrl+A/E/K/U etc. Thin: builds an editor, runs the loop,
    /// then best-effort appends history and tightens file perms on unix.
    fn run_repl_loop_rustyline(&mut self) -> Result<(), String> {
        let (mut editor, history_path) = self.prepare_editor()?;
        let outcome = self.repl_loop_body(&mut editor);
        if let Err(err) = editor.append_history(&history_path) {
            tracing::warn!(
                "readline: failed to append history ({}): {err}",
                history_path.display()
            );
        }
        #[cfg(unix)]
        {
            tighten_history_perms(&history_path);
        }
        outcome
    }

    /// Resolve state_root, build the editor, and load any existing history
    /// file. The parent `state_root` is normally created by `ensure_state_dirs`
    /// before we get here (SSOT, via `run_cli`). We additionally call
    /// `create_dir_all(state_root)` here as a defensive fallback for direct
    /// callers of `run_repl` / `run_repl_loop` (see `run_repl_loop` preconditions):
    /// without it, `append_history` would silently fail on REPL teardown and we
    /// would lose history persistence — a latent regression the wrapper can
    /// prevent cheaply (a single `mkdir -p`).
    fn prepare_editor(&self) -> Result<(AnvilEditor, std::path::PathBuf), String> {
        let state_root = crate::resolve_state_root(&self.config)?;
        if let Err(err) = std::fs::create_dir_all(&state_root) {
            tracing::warn!(
                "readline: failed to ensure state_root exists ({}): {err}",
                state_root.display()
            );
        }
        let history_path = state_root.join("history");

        let mut editor = build_editor().map_err(|err| {
            format!("readline: failed to init editor (Editor::with_config): {err}")
        })?;

        if let Err(err) = editor.load_history(&history_path)
            && !is_not_found(&err)
        {
            tracing::warn!(
                "readline: failed to load history ({}): {err}",
                history_path.display()
            );
        }

        Ok((editor, history_path))
    }

    /// The readline loop body itself — no history I/O, no permission work.
    /// Ctrl+C discards the current line and keeps going, Ctrl+D (on empty
    /// line) exits cleanly, other errors are logged and cause a graceful exit.
    fn repl_loop_body(&mut self, editor: &mut AnvilEditor) -> Result<(), String> {
        use rustyline::error::ReadlineError;

        loop {
            // Issue #430 Phase D: pause footer redraw for the rustyline
            // prompt. The footer line stays painted (DR1-006 #6: maintain
            // DECSTBM), only the daemon worker stops re-emitting ANSI so
            // rustyline owns stdout / cursor positioning. Guard drops once
            // readline returns and the worker resumes within the next tick.
            let _footer_freeze = self.footer.freeze_for_prompt();
            match editor.readline("anvil> ") {
                Ok(line) => {
                    let trimmed = line.trim();
                    if trimmed.is_empty() {
                        continue;
                    }
                    match self.process_line(trimmed, self.config.stream) {
                        Ok(AgentEvent::Continue(Some(msg))) => println!("{msg}"),
                        Ok(AgentEvent::Continue(None)) => {}
                        Ok(AgentEvent::Exit) => break Ok(()),
                        Err(err) => break Err(err),
                    }
                }
                Err(ReadlineError::Interrupted) => continue,
                Err(ReadlineError::Eof) => break Ok(()),
                Err(other) => {
                    tracing::error!("readline: failed to read input: {other}");
                    break Ok(());
                }
            }
        }
    }

    /// Backwards-compatible wrapper that prints the startup banner and then
    /// runs the REPL loop. Kept for consumers that still call `run_repl`
    /// directly; `run_cli` no longer uses it because the banner is printed
    /// one level up for consistency across REPL / oneshot / resume.
    ///
    /// # Preconditions
    ///
    /// Same as `run_repl_loop`: the caller is expected to have invoked
    /// `crate::ensure_state_dirs` (via `run_cli`) so `state_root` exists with
    /// 0o700 permissions. `prepare_editor` will still `create_dir_all` the
    /// state root as a fallback, but direct callers do not get the
    /// permission-hardening SSOT — prefer `run_cli` for new entry points.
    pub fn run_repl(&mut self) -> Result<(), String> {
        print_startup_banner(
            env!("CARGO_PKG_VERSION"),
            &format_model_banner(&self.models),
            &self.session.mode_state.mode,
            &self.work_root,
            short_id(&self.session.id),
            self.session.messages.len(),
            self.config.fresh_session,
            false,
            self.config.log_level,
        );
        self.run_repl_loop()
    }

    pub fn process_line(&mut self, input: &str, stream_output: bool) -> Result<AgentEvent, String> {
        if input.trim().is_empty() {
            return Ok(AgentEvent::Continue(None));
        }

        let outcome = if input.starts_with('/') {
            self.handle_command(input)
        } else {
            match self.handle_user_message(input, stream_output) {
                Ok((prose, stats)) => {
                    if !stream_output {
                        // Issue #431: non-streaming assistant prose also goes
                        // through the markdown renderer. `prose` here is the
                        // raw LLM text (session has already stored this raw
                        // content in `run_actor_loop`). Feed the raw text
                        // directly into a one-shot renderer — its own
                        // `<think>`-stripping matches the streaming path and
                        // does not trim leading/trailing whitespace (unlike
                        // `xml_fallback::strip_think_tags`). Skip entirely
                        // when `ANVIL_NO_MARKDOWN` disables markdown.
                        if crate::tui::markdown::markdown_fully_disabled() {
                            println!("{prose}");
                        } else {
                            let color = crate::tui::markdown::color_enabled_for_markdown();
                            let utf8 = crate::tui::markdown::markdown_unicode_enabled();
                            let mut r = crate::tui::markdown::MarkdownRenderer::new(color, utf8);
                            let mut body = r.push_chunk(&prose);
                            body.push_str(&r.flush());
                            // Preserve the trailing newline that `println!`
                            // used to add.
                            if !body.ends_with('\n') {
                                body.push('\n');
                            }
                            let _ = std::io::stdout().write_all(body.as_bytes());
                            let _ = std::io::stdout().flush();
                        }
                    }
                    let summary = format_run_summary(ExitReason::Done, &stats);
                    println!();
                    println!("{summary}");
                    Ok(AgentEvent::Continue(None))
                }
                Err((reason, error_text, stats)) => {
                    let summary = format_run_summary(reason, &stats);
                    println!();
                    println!("{summary}");
                    // ESC-initiated exits are a user-intended pause, not a run
                    // failure — stay in the REPL and let the persist_session
                    // below save a resumable snapshot (AC-1 / S3-001 / S5-002).
                    if matches!(reason, ExitReason::Interrupted) {
                        Ok(AgentEvent::Continue(None))
                    } else {
                        Err(error_text)
                    }
                }
            }
        };

        let persist_error = self.persist_session().err();
        match (outcome, persist_error) {
            (Ok(event), None) => Ok(event),
            (Ok(_), Some(err)) => Err(err),
            (Err(err), None) => Err(err),
            (Err(err), Some(persist_err)) => Err(format!(
                "{err} (also failed to persist session: {persist_err})"
            )),
        }
    }

    fn handle_command(&mut self, input: &str) -> Result<AgentEvent, String> {
        let (command, _) = input.split_once(' ').unwrap_or((input, ""));
        match command {
            "/help" => Ok(AgentEvent::Continue(Some(slash_commands::help_line()))),
            "/status" => Ok(AgentEvent::Continue(Some(format!(
                "mode={:?} auto_approve={} native_tools={} cwd={} session={} plan={} approx_tokens={} log_level={} core_only=true",
                self.session.mode_state.mode,
                self.config.yes_mode,
                self.native_tools_enabled,
                self.work_root.display(),
                self.session_store.path().display(),
                self.session
                    .mode_state
                    .active_plan_path
                    .as_ref()
                    .map(|path| path.display().to_string())
                    .unwrap_or_else(|| "-".to_string()),
                approximate_token_count(&self.session.messages),
                self.config.log_level,
            )))),
            "/model" => Ok(AgentEvent::Continue(Some(format_model_banner(
                &self.models,
            )))),
            "/yes" => {
                self.config.yes_mode = true;
                // Issue #430: republish flags so the footer reflects the new
                // yes-mode bit on the next render tick.
                self.footer.publish_flags(
                    self.session.mode_state.mode,
                    self.config.log_level,
                    self.config.yes_mode,
                );
                Ok(AgentEvent::Continue(Some(
                    "auto-approve enabled".to_string(),
                )))
            }
            "/no" => {
                self.config.yes_mode = false;
                self.footer.publish_flags(
                    self.session.mode_state.mode,
                    self.config.log_level,
                    self.config.yes_mode,
                );
                Ok(AgentEvent::Continue(Some(
                    "auto-approve disabled".to_string(),
                )))
            }
            "/plan" => {
                if self.session.mode_state.mode == ExecutionMode::Plan {
                    let current = self
                        .session
                        .mode_state
                        .active_plan_path
                        .as_ref()
                        .map(|path| path.display().to_string())
                        .unwrap_or_else(|| "-".to_string());
                    return Ok(AgentEvent::Continue(Some(format!(
                        "already in plan mode: {current}"
                    ))));
                }
                let plan_path = self
                    .session
                    .mode_state
                    .enter_plan(self.session_store.plan_dir())?;
                self.ensure_plan_file(&plan_path)?;
                self.push_system_note(format!(
                    "[Plan Mode] Explore with Read, Glob, and Grep. Write the plan to {}. Wait for /approve before making code changes.",
                    plan_path.display()
                ));
                // Republish flags after entering plan mode (issue #430).
                self.footer.publish_flags(
                    self.session.mode_state.mode,
                    self.config.log_level,
                    self.config.yes_mode,
                );
                Ok(AgentEvent::Continue(Some(format!(
                    "plan mode: {}",
                    plan_path.display()
                ))))
            }
            "/approve" | "/act" => {
                if self.session.mode_state.mode != ExecutionMode::Plan {
                    return Err("approve is only available from plan mode".to_string());
                }
                let plan_contents = self
                    .current_plan_contents()?
                    .ok_or_else(|| "plan file is missing".to_string())?;
                if !lifecycle::plan_is_substantive(&plan_contents) {
                    return Err("plan file is empty or still template-only".to_string());
                }
                self.session.mode_state.approve();
                self.push_system_note(format!(
                    "[Act Mode] Implement the following plan step by step.\n\n{}",
                    plan_contents
                ));
                // Republish flags after switching to act mode (issue #430).
                self.footer.publish_flags(
                    self.session.mode_state.mode,
                    self.config.log_level,
                    self.config.yes_mode,
                );
                Ok(AgentEvent::Continue(Some("act mode".to_string())))
            }
            "/compact" => {
                let changed = self.maybe_compact_session(20);
                Ok(AgentEvent::Continue(Some(if changed {
                    "session compacted".to_string()
                } else {
                    "session already compact".to_string()
                })))
            }
            "/logs" => {
                let args: Vec<&str> = input
                    .trim_start_matches("/logs")
                    .split_whitespace()
                    .collect();
                match args.as_slice() {
                    ["path"] => {
                        let path = self.session_store.log_dir().display().to_string();
                        Ok(AgentEvent::Continue(Some(format!("log dir: {path}"))))
                    }
                    ["path", session_id] => match uuid::Uuid::parse_str(session_id) {
                        Err(_) => Ok(AgentEvent::Continue(Some(format!(
                            "invalid session id: {session_id}"
                        )))),
                        Ok(_) => match self.session_store.log_dir_for(session_id) {
                            Some(path) => Ok(AgentEvent::Continue(Some(format!(
                                "log dir: {}",
                                path.display()
                            )))),
                            None => Ok(AgentEvent::Continue(Some(format!(
                                "session not found: {session_id}"
                            )))),
                        },
                    },
                    _ => Ok(AgentEvent::Continue(Some(
                        "/logs path [<session_id>] — show log dir".to_string(),
                    ))),
                }
            }
            "/checkpoint" | "/rollback" | "/watch" | "/autotest" | "/skills" | "/skill"
            | "/mcp" | "/parallel" => Ok(AgentEvent::Continue(Some(format!(
                "{command} is unavailable in the v0.1.0 core rebuild"
            )))),
            "/exit" | "/quit" => Ok(AgentEvent::Exit),
            _ => Ok(AgentEvent::Continue(Some(format!(
                "unknown command: {command}"
            )))),
        }
    }
}

/// NotFound (or PermissionDenied on some CI/sandbox environments) on
/// `load_history` means "no history yet" — swallow silently so REPL starts
/// cleanly on a fresh environment.
fn is_not_found(err: &rustyline::error::ReadlineError) -> bool {
    matches!(
        err,
        rustyline::error::ReadlineError::Io(io_err)
            if io_err.kind() == std::io::ErrorKind::NotFound
                || io_err.kind() == std::io::ErrorKind::PermissionDenied
    )
}

/// Defense-in-depth: rustyline 14 already chmods 0o600 on save, but in case
/// an external tool widened the file, we re-apply 0o600 after append. Best
/// effort; the path-not-found case is silent (common on first run before
/// `append_history` has created the file).
///
/// Hardening against CB-002: we use `symlink_metadata()` + `FileType::is_file()`
/// so a symlink-swap or non-regular-file substitution at `state_root/history`
/// does NOT cause us to chmod an unintended target. A symlink here already
/// implies an adversarial / misconfigured state (state_root is owner-only
/// 0o700 on Unix) so we log a warning and skip instead of following the link.
#[cfg(unix)]
fn tighten_history_perms(path: &Path) {
    use std::os::unix::fs::PermissionsExt;
    let meta = match std::fs::symlink_metadata(path) {
        Ok(m) => m,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return,
        Err(err) => {
            tracing::warn!(
                "readline: failed to stat history for chmod ({}): {err}",
                path.display()
            );
            return;
        }
    };
    if !meta.file_type().is_file() {
        tracing::warn!(
            "readline: refusing to chmod non-regular history path ({}): file_type={:?}",
            path.display(),
            meta.file_type()
        );
        return;
    }
    if let Err(err) = std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600)) {
        tracing::warn!(
            "readline: failed to chmod history ({}): {err}",
            path.display()
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::modes::plan_act::ExecutionMode;
    use std::path::{Path, PathBuf};
    use std::sync::Mutex;

    /// Serialize env-mutating tests within this module so `cargo test`'s
    /// default parallel runner cannot race on `NO_COLOR`.
    static ENV_MUTEX: Mutex<()> = Mutex::new(());

    /// RAII guard that snapshots `NO_COLOR` on construction and restores it on
    /// drop. All env mutation is confined to `#[cfg(test)]` per CLAUDE.md.
    struct NoColorGuard {
        prior: Option<std::ffi::OsString>,
    }

    impl NoColorGuard {
        fn capture() -> Self {
            Self {
                prior: std::env::var_os("NO_COLOR"),
            }
        }

        fn set(&self, value: &str) {
            // SAFETY: test-only env mutation, serialized by `ENV_MUTEX`.
            unsafe {
                std::env::set_var("NO_COLOR", value);
            }
        }

        fn unset(&self) {
            // SAFETY: test-only env mutation, serialized by `ENV_MUTEX`.
            unsafe {
                std::env::remove_var("NO_COLOR");
            }
        }
    }

    impl Drop for NoColorGuard {
        fn drop(&mut self) {
            match &self.prior {
                Some(value) => {
                    // SAFETY: test-only env mutation, serialized by `ENV_MUTEX`.
                    unsafe {
                        std::env::set_var("NO_COLOR", value);
                    }
                }
                None => {
                    // SAFETY: test-only env mutation, serialized by `ENV_MUTEX`.
                    unsafe {
                        std::env::remove_var("NO_COLOR");
                    }
                }
            }
        }
    }

    fn sample_mode() -> ExecutionMode {
        ExecutionMode::Act
    }

    fn build_inputs<'a>(
        model_banner: &'a str,
        mode: &'a ExecutionMode,
        work_root: &'a Path,
        llm_log_path: Option<&'a Path>,
        fresh: bool,
        resumed: bool,
        style: BannerStyle,
    ) -> BannerInputs<'a> {
        BannerInputs {
            version: "9.9.9",
            model_banner,
            mode,
            work_root,
            session_id_short: "abcd1234",
            message_count: 3,
            fresh,
            resumed,
            llm_log_path,
            style,
        }
    }

    // --- decide_banner_style truth table -----------------------------------

    #[test]
    fn decide_banner_style_non_tty_returns_legacy() {
        assert_eq!(
            decide_banner_style(false, false, Some(200)),
            BannerStyle::Legacy4Line
        );
        assert_eq!(
            decide_banner_style(false, true, Some(200)),
            BannerStyle::Legacy4Line
        );
        assert_eq!(
            decide_banner_style(false, false, None),
            BannerStyle::Legacy4Line
        );
    }

    #[test]
    fn decide_banner_style_tty_size_err_returns_legacy() {
        assert_eq!(
            decide_banner_style(true, false, None),
            BannerStyle::Legacy4Line
        );
    }

    #[test]
    fn decide_banner_style_tty_narrow_returns_legacy() {
        assert_eq!(
            decide_banner_style(true, false, Some(MIN_BANNER_WIDTH - 1)),
            BannerStyle::Legacy4Line
        );
        assert_eq!(
            decide_banner_style(true, true, Some(10)),
            BannerStyle::Legacy4Line
        );
    }

    #[test]
    fn decide_banner_style_tty_wide_no_color_returns_mono_art() {
        assert_eq!(
            decide_banner_style(true, true, Some(MIN_BANNER_WIDTH)),
            BannerStyle::MonoArt
        );
        assert_eq!(
            decide_banner_style(true, true, Some(200)),
            BannerStyle::MonoArt
        );
    }

    #[test]
    fn decide_banner_style_tty_wide_color_returns_neon() {
        assert_eq!(
            decide_banner_style(true, false, Some(MIN_BANNER_WIDTH)),
            BannerStyle::Neon
        );
        assert_eq!(
            decide_banner_style(true, false, Some(200)),
            BannerStyle::Neon
        );
    }

    // --- state_suffix ------------------------------------------------------

    #[test]
    fn state_suffix_fresh() {
        assert_eq!(state_suffix(true, false), "fresh");
    }

    #[test]
    fn state_suffix_resumed() {
        assert_eq!(state_suffix(false, true), "resumed");
    }

    #[test]
    fn state_suffix_continued() {
        assert_eq!(state_suffix(false, false), "continued");
    }

    #[test]
    fn state_suffix_fresh_beats_resumed() {
        // Defensive: `fresh` dominates so a caller that mistakenly sets both
        // flags still gets the intended `fresh` label.
        assert_eq!(state_suffix(true, true), "fresh");
    }

    // --- banner_no_color_requested vs turn::no_color_requested -------------

    #[test]
    fn banner_no_color_requested_unset_is_false() {
        let _lock = ENV_MUTEX.lock().unwrap();
        let guard = NoColorGuard::capture();
        guard.unset();
        assert!(!banner_no_color_requested());
        assert!(!super::super::turn::no_color_requested());
    }

    #[test]
    fn banner_no_color_requested_empty_is_false() {
        let _lock = ENV_MUTEX.lock().unwrap();
        let guard = NoColorGuard::capture();
        guard.set("");
        assert!(!banner_no_color_requested());
        assert!(!super::super::turn::no_color_requested());
    }

    #[test]
    fn banner_no_color_requested_nonempty_is_true() {
        let _lock = ENV_MUTEX.lock().unwrap();
        let guard = NoColorGuard::capture();
        guard.set("1");
        assert!(banner_no_color_requested());
        assert!(super::super::turn::no_color_requested());
    }

    #[test]
    fn banner_no_color_requested_matches_turn_across_values() {
        let _lock = ENV_MUTEX.lock().unwrap();
        let guard = NoColorGuard::capture();
        for value in ["1", "yes", "0", "false", ""] {
            guard.set(value);
            assert_eq!(
                banner_no_color_requested(),
                super::super::turn::no_color_requested(),
                "drift detected for NO_COLOR={value:?}"
            );
        }
        guard.unset();
        assert_eq!(
            banner_no_color_requested(),
            super::super::turn::no_color_requested()
        );
    }

    // --- sanitize_banner_text ---------------------------------------------

    #[test]
    fn sanitize_banner_text_replaces_esc() {
        assert_eq!(sanitize_banner_text("\x1b[31mred"), " [31mred");
    }

    #[test]
    fn sanitize_banner_text_replaces_all_c0_and_del() {
        let hostile = "a\nb\rc\x07d\x1bE\x7f";
        let cleaned = sanitize_banner_text(hostile);
        assert!(!cleaned.contains('\x1b'));
        assert!(!cleaned.contains('\n'));
        assert!(!cleaned.contains('\r'));
        assert!(!cleaned.contains('\x07'));
        assert!(!cleaned.contains('\x7f'));
    }

    #[test]
    fn sanitize_banner_text_replaces_c1_controls_including_8bit_csi_osc() {
        // Some terminals interpret U+009B (8-bit CSI) and U+009D (8-bit OSC)
        // equivalently to `ESC [` and `ESC ]`, so they must be sanitized too.
        let hostile = "a\u{009B}31mred\u{009D}8;;bad.example\u{009C}tail";
        let cleaned = sanitize_banner_text(hostile);
        assert!(!cleaned.contains('\u{009B}'));
        assert!(!cleaned.contains('\u{009D}'));
        assert!(!cleaned.contains('\u{009C}'));
        // A valid 2-byte UTF-8 (e.g. \u{00C0}..=\u{00FF}) outside C1 must survive.
        assert_eq!(sanitize_banner_text("caf\u{00E9}"), "caf\u{00E9}");
    }

    #[test]
    fn sanitize_banner_text_trims_trailing_whitespace() {
        assert_eq!(sanitize_banner_text("ok   "), "ok");
        assert_eq!(sanitize_banner_text("ok\n\n"), "ok");
    }

    #[test]
    fn sanitize_banner_text_matches_turn_sanitize_spec() {
        // Ensure our sanitizer agrees with the progress-line sanitizer spec:
        // C0, DEL, and C1 become spaces, then trailing whitespace is trimmed.
        for input in [
            "plain",
            "hello\x1b[0m",
            "line1\nline2",
            "bell\x07tail   ",
            "  mid  space  ",
            "csi8\u{009B}31mred",
            "osc8\u{009D}8;;evil.example",
        ] {
            // Re-implement the expected algorithm inline so drift in either
            // implementation is caught by the assertion.
            let mut expected = String::with_capacity(input.len());
            for ch in input.chars() {
                let cp = ch as u32;
                if cp < 0x20 || cp == 0x7F || (0x80..=0x9F).contains(&cp) {
                    expected.push(' ');
                } else {
                    expected.push(ch);
                }
            }
            let expected = expected.trim_end().to_string();
            assert_eq!(sanitize_banner_text(input), expected);
        }
    }

    // --- render_startup_banner: Legacy4Line byte-identical -----------------

    #[test]
    fn render_legacy4line_bytes_fresh_no_log() {
        let mode = sample_mode();
        let cwd = PathBuf::from("/tmp/anvil-test");
        let inputs = build_inputs(
            "llama3.2 model-banner",
            &mode,
            &cwd,
            None,
            true,
            false,
            BannerStyle::Legacy4Line,
        );
        let out = render_startup_banner(&inputs);
        assert_eq!(
            out,
            "anvil 9.9.9\n\
             llama3.2 model-banner\n\
             mode=Act cwd=/tmp/anvil-test\n\
             session=abcd1234 messages=3 [fresh]\n"
        );
    }

    #[test]
    fn render_legacy4line_bytes_resumed_no_log() {
        let mode = sample_mode();
        let cwd = PathBuf::from("/tmp/anvil-test");
        let inputs = build_inputs(
            "llama3.2 model-banner",
            &mode,
            &cwd,
            None,
            false,
            true,
            BannerStyle::Legacy4Line,
        );
        let out = render_startup_banner(&inputs);
        assert!(out.ends_with("[resumed]\n"));
        assert!(!out.contains("\x1b["));
    }

    #[test]
    fn render_legacy4line_bytes_continued_no_log() {
        let mode = sample_mode();
        let cwd = PathBuf::from("/tmp/anvil-test");
        let inputs = build_inputs(
            "m",
            &mode,
            &cwd,
            None,
            false,
            false,
            BannerStyle::Legacy4Line,
        );
        let out = render_startup_banner(&inputs);
        assert!(out.ends_with("[continued]\n"));
    }

    #[test]
    fn render_legacy4line_bytes_with_llm_log() {
        let mode = sample_mode();
        let cwd = PathBuf::from("/tmp/anvil-test");
        let log = PathBuf::from("/tmp/anvil-test/llm-io.jsonl");
        let inputs = build_inputs(
            "llama3.2 model-banner",
            &mode,
            &cwd,
            Some(log.as_path()),
            true,
            false,
            BannerStyle::Legacy4Line,
        );
        let out = render_startup_banner(&inputs);
        assert_eq!(
            out,
            "anvil 9.9.9\n\
             llama3.2 model-banner\n\
             mode=Act cwd=/tmp/anvil-test\n\
             session=abcd1234 messages=3 [fresh]\n\
             llm log=/tmp/anvil-test/llm-io.jsonl\n"
        );
    }

    // --- render_startup_banner: MonoArt -----------------------------------

    #[test]
    fn render_mono_art_has_no_ansi() {
        let mode = sample_mode();
        let cwd = PathBuf::from("/tmp/anvil-test");
        let inputs = build_inputs(
            "llama3.2",
            &mode,
            &cwd,
            None,
            true,
            false,
            BannerStyle::MonoArt,
        );
        let out = render_startup_banner(&inputs);
        assert!(
            !out.contains("\x1b["),
            "mono art must not contain ANSI escape: {out:?}"
        );
        // Sanity check: output contains the dynamic lines.
        assert!(out.contains("anvil 9.9.9\n"));
        assert!(out.contains("session=abcd1234 messages=3 [fresh]\n"));
    }

    #[test]
    fn render_mono_art_includes_ascii_art_lines() {
        let mode = sample_mode();
        let cwd = PathBuf::from("/tmp/a");
        let inputs = build_inputs("m", &mode, &cwd, None, false, true, BannerStyle::MonoArt);
        let out = render_startup_banner(&inputs);
        for line in ANVIL_ASCII_ART.iter() {
            assert!(out.contains(line), "missing art line: {line:?}");
        }
        assert!(out.contains("[resumed]\n"));
    }

    #[test]
    fn render_mono_art_with_log_path() {
        let mode = sample_mode();
        let cwd = PathBuf::from("/tmp/a");
        let log = PathBuf::from("/tmp/llm.jsonl");
        let inputs = build_inputs(
            "m",
            &mode,
            &cwd,
            Some(log.as_path()),
            false,
            false,
            BannerStyle::MonoArt,
        );
        let out = render_startup_banner(&inputs);
        assert!(out.contains("llm log=/tmp/llm.jsonl\n"));
        assert!(out.contains("[continued]\n"));
        assert!(!out.contains("\x1b["));
    }

    // --- render_startup_banner: Neon --------------------------------------

    #[test]
    fn render_neon_contains_expected_ansi_codes() {
        let mode = sample_mode();
        let cwd = PathBuf::from("/tmp/anvil-test");
        let inputs = build_inputs(
            "llama3.2",
            &mode,
            &cwd,
            None,
            true,
            false,
            BannerStyle::Neon,
        );
        let out = render_startup_banner(&inputs);
        // First gradient color is 51.
        assert!(
            out.contains("\x1b[38;5;51m"),
            "expected first gradient code: {out:?}"
        );
        // Every ASCII-art line terminates with a reset.
        assert!(out.contains("\x1b[0m"));
        // All five gradient codes should appear.
        for code in NEON_GRADIENT_256 {
            let needle = format!("\x1b[38;5;{code}m");
            assert!(
                out.contains(&needle),
                "missing gradient escape {needle:?} in {out:?}"
            );
        }
    }

    #[test]
    fn render_neon_resumed_and_log_combinations() {
        let mode = sample_mode();
        let cwd = PathBuf::from("/tmp/anvil-test");
        let log = PathBuf::from("/tmp/anvil-test/llm.jsonl");
        let inputs = build_inputs(
            "m",
            &mode,
            &cwd,
            Some(log.as_path()),
            false,
            true,
            BannerStyle::Neon,
        );
        let out = render_startup_banner(&inputs);
        assert!(out.contains("[resumed]\n"));
        assert!(out.contains("llm log=/tmp/anvil-test/llm.jsonl\n"));
        assert!(out.contains("\x1b[38;5;51m"));
    }

    #[test]
    fn render_neon_continued_no_log() {
        let mode = sample_mode();
        let cwd = PathBuf::from("/tmp/a");
        let inputs = build_inputs("m", &mode, &cwd, None, false, false, BannerStyle::Neon);
        let out = render_startup_banner(&inputs);
        assert!(out.contains("[continued]\n"));
        assert!(!out.contains("llm log="));
    }

    // --- Hostile-input regression -----------------------------------------

    #[test]
    fn render_sanitizes_hostile_model_banner() {
        let mode = sample_mode();
        let cwd = PathBuf::from("/tmp/anvil-test");
        let inputs = build_inputs(
            "bad\x1b[31m!\nnewline",
            &mode,
            &cwd,
            None,
            true,
            false,
            BannerStyle::Legacy4Line,
        );
        let out = render_startup_banner(&inputs);
        // Dynamic lines must never carry raw ESC / bare newlines from user input.
        assert!(
            !out.contains("\x1b["),
            "raw ESC leaked into Legacy4Line: {out:?}"
        );
        // Sanitized placeholder `!` should still be present.
        assert!(out.contains(" [31m!"));
    }

    #[test]
    fn render_sanitizes_hostile_cwd_and_log_path() {
        let mode = sample_mode();
        let cwd = PathBuf::from("/tmp/with\x1b[31mansi");
        let log = PathBuf::from("/tmp/l\nog.jsonl");
        let inputs = build_inputs(
            "m",
            &mode,
            &cwd,
            Some(log.as_path()),
            true,
            false,
            BannerStyle::Legacy4Line,
        );
        let out = render_startup_banner(&inputs);
        assert!(!out.contains('\x07'));
        assert!(!out.contains("\x1b["));
        // The log path's embedded newline must be neutralized, so the output
        // should still have exactly as many lines as the renderer emitted.
        let line_count = out.split_terminator('\n').count();
        assert_eq!(line_count, 5, "unexpected extra lines: {out:?}");
    }

    #[test]
    fn render_neon_hostile_inputs_contain_no_extra_escapes() {
        let mode = sample_mode();
        let cwd = PathBuf::from("/tmp/with\x1b[31mansi");
        let inputs = build_inputs(
            "m\x1b]8;;https://x\x07",
            &mode,
            &cwd,
            None,
            true,
            false,
            BannerStyle::Neon,
        );
        let out = render_startup_banner(&inputs);
        // Every ESC must belong to a banner-owned SGR escape (38;5;N or 0).
        for (i, byte) in out.as_bytes().iter().enumerate() {
            if *byte == 0x1b {
                // Next bytes must be `[38;5;` or `[0m`.
                let tail = &out.as_bytes()[i..];
                assert!(
                    tail.starts_with(b"\x1b[38;5;") || tail.starts_with(b"\x1b[0m"),
                    "unexpected ESC sequence at {i}: {:?}",
                    &out[i..i + 8.min(out.len() - i)]
                );
            }
        }
    }
}
