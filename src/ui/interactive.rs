use std::io::{self, Write};
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};
use std::thread;
use std::time::Duration;

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
