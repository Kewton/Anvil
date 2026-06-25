use std::io::{self, IsTerminal};

use anyhow::bail;
use rustyline::error::ReadlineError;

use crate::config::Config;
use crate::tui::markdown::TerminalMarkdownRenderer;
use crate::tui::{OutputRenderer, TerminalUi};

pub fn run(config: Config) -> anyhow::Result<()> {
    if !io::stdin().is_terminal() {
        bail!("stdin is not a TTY; pass --prompt or an action flag");
    }

    crate::tui::banner::print_startup_banner(&config)?;
    let ui = TerminalUi::new(&config);
    let renderer = TerminalMarkdownRenderer::for_stdout();
    let mut execution = crate::providers::client_from_config(&config, false)?;
    let mut planner = crate::providers::client_from_config(&config, true)?;
    let mut editor = rustyline::DefaultEditor::new()?;
    let history_path = config.state_dir.join("history.txt");
    if let Some(parent) = history_path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let _ = editor.load_history(&history_path);

    loop {
        let line = {
            let _prompt_guard = ui.pause_for_prompt();
            match editor.readline("anvil> ") {
                Ok(line) => line,
                Err(ReadlineError::Interrupted) => continue,
                Err(ReadlineError::Eof) => break,
                Err(err) => return Err(err.into()),
            }
        };
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        if matches!(line, "/exit" | "/quit") {
            break;
        }
        let _ = editor.add_history_entry(line);
        match crate::tui::slash::handle_command(line, &config, &mut *planner, &mut *execution, &ui)
        {
            Ok(output) => renderer.render_assistant(&output)?,
            Err(err) => eprintln!("error: {err:#}"),
        }
        ui.reset_interrupt();
    }
    let _ = editor.save_history(&history_path);
    Ok(())
}
