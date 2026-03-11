use std::io::{self, Write};
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};
use std::thread;
use std::time::Duration;

use anyhow::Context;
use rustyline::DefaultEditor;
use rustyline::error::ReadlineError;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UiEvent {
    UserInput(String),
    AgentText(String),
    ToolCall(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FooterState {
    pub mode: String,
    pub pending_hint: String,
    pub token_status: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InteractiveFrame {
    pub title: String,
    pub provider: String,
    pub model: String,
    pub cwd: String,
    pub transcript: Vec<UiEvent>,
    pub footer: FooterState,
}

pub struct SpinnerHandle {
    done: Arc<AtomicBool>,
    join: Option<thread::JoinHandle<()>>,
}

pub struct LineEditor {
    editor: DefaultEditor,
}

impl SpinnerHandle {
    pub fn start(label: impl Into<String>) -> Self {
        let done = Arc::new(AtomicBool::new(false));
        let thread_done = Arc::clone(&done);
        let label = label.into();
        let join = thread::spawn(move || {
            let frames = ["|", "/", "-", "\\"];
            let mut idx = 0usize;
            while !thread_done.load(Ordering::Relaxed) {
                let _ = write!(
                    io::stdout(),
                    "\r\x1b[2K\x1b[36m{}\x1b[0m {} processing...",
                    frames[idx % frames.len()],
                    label
                );
                let _ = io::stdout().flush();
                idx += 1;
                thread::sleep(Duration::from_millis(90));
            }
        });
        Self {
            done,
            join: Some(join),
        }
    }

    pub fn stop(mut self, final_label: &str) {
        self.done.store(true, Ordering::Relaxed);
        if let Some(join) = self.join.take() {
            let _ = join.join();
        }
        let _ = write!(io::stdout(), "\r\x1b[2K\x1b[32m*\x1b[0m {final_label}\n");
        let _ = io::stdout().flush();
    }
}

impl LineEditor {
    pub fn new() -> anyhow::Result<Self> {
        let editor = DefaultEditor::new().context("failed to initialize line editor")?;
        Ok(Self { editor })
    }

    pub fn read_command(&mut self, prompt: &str) -> anyhow::Result<String> {
        let first_line = self.read_line(prompt)?;
        if first_line.trim() != "\"\"\"" {
            if !first_line.trim().is_empty() {
                let _ = self.editor.add_history_entry(first_line.as_str());
            }
            return Ok(first_line.trim().to_string());
        }

        let mut lines = Vec::new();
        loop {
            let line = self.read_line("... ")?;
            if line.trim() == "\"\"\"" {
                let joined = lines.join("\n").trim().to_string();
                if !joined.is_empty() {
                    let _ = self.editor.add_history_entry(joined.as_str());
                }
                return Ok(joined);
            }
            lines.push(line);
        }
    }

    fn read_line(&mut self, prompt: &str) -> anyhow::Result<String> {
        match self.editor.readline(prompt) {
            Ok(line) => Ok(line),
            Err(ReadlineError::Interrupted) | Err(ReadlineError::Eof) => Ok("/exit".to_string()),
            Err(err) => Err(anyhow::anyhow!(err)).context("failed to read interactive input"),
        }
    }
}
