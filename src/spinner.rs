//! Terminal spinner for visual feedback during blocking operations.

use std::io::Write;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread;
use std::time::Duration;

const FRAMES: &[char] = &['⠋', '⠙', '⠹', '⠸', '⠼', '⠴', '⠦', '⠧', '⠇', '⠏'];
const FRAME_MS: u64 = 80;

/// A terminal spinner that runs in a background thread.
///
/// Writes animated progress to stderr using carriage-return overwrite.
/// Automatically cleans up the line when stopped or dropped.
pub struct Spinner {
    running: Arc<AtomicBool>,
    handle: Option<thread::JoinHandle<()>>,
}

impl Spinner {
    /// Start a spinner with the given message.
    pub fn start(message: impl Into<String>) -> Self {
        let running = Arc::new(AtomicBool::new(true));
        let flag = running.clone();
        let message = message.into();

        let handle = thread::spawn(move || {
            let mut stderr = std::io::stderr();
            let mut i = 0usize;
            while flag.load(Ordering::Relaxed) {
                let frame = FRAMES[i % FRAMES.len()];
                let _ = write!(stderr, "\r{frame} {message}");
                let _ = stderr.flush();
                thread::sleep(Duration::from_millis(FRAME_MS));
                i += 1;
            }
            // Clear the spinner line
            let clear_len = message.len() + 4;
            let _ = write!(stderr, "\r{:width$}\r", "", width = clear_len);
            let _ = stderr.flush();
        });

        Self {
            running,
            handle: Some(handle),
        }
    }

    /// Stop the spinner and clear the line.
    pub fn stop(mut self) {
        self.shutdown();
    }

    fn shutdown(&mut self) {
        self.running.store(false, Ordering::Relaxed);
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
    }
}

impl Drop for Spinner {
    fn drop(&mut self) {
        self.shutdown();
    }
}
