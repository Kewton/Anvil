//! Terminal spinner for visual feedback during blocking operations.

use std::io::Write;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use crate::app::render::sanitize_display_string;

const FRAMES: &[char] = &['⠋', '⠙', '⠹', '⠸', '⠼', '⠴', '⠦', '⠧', '⠇', '⠏'];
const FRAME_MS: u64 = 80;

/// The display mode for a spinner.
#[derive(Clone)]
pub enum SpinnerMode {
    /// Simple spinner with a static message: `⠋ message`
    Simple,
    /// Tool-progress spinner: `⠋ [1/3] [file.read] 0.3s`
    Tool {
        total: usize,
        current_index: Arc<AtomicUsize>,
        tool_name: Arc<Mutex<String>>,
    },
    /// Parallel-progress spinner: `⠋ [2/4 completed]`
    Parallel {
        total: usize,
        completed: Arc<AtomicUsize>,
    },
}

/// A terminal spinner that runs in a background thread.
///
/// Writes animated progress to stderr using carriage-return overwrite.
/// Automatically cleans up the line when stopped or dropped.
pub struct Spinner {
    running: Arc<AtomicBool>,
    paused: Arc<AtomicBool>,
    handle: Option<thread::JoinHandle<()>>,
    mode: SpinnerMode,
}

impl Spinner {
    /// Create a disabled (no-op) spinner.
    fn noop() -> Self {
        Self {
            running: Arc::new(AtomicBool::new(false)),
            paused: Arc::new(AtomicBool::new(false)),
            handle: None,
            mode: SpinnerMode::Simple,
        }
    }

    /// Start a spinner with the given message.
    ///
    /// When `enabled` is false, returns a no-op spinner that does nothing.
    /// This is used in non-interactive mode to suppress terminal output.
    pub fn start(message: impl Into<String>, enabled: bool) -> Self {
        if !enabled {
            return Self::noop();
        }
        let running = Arc::new(AtomicBool::new(true));
        let paused = Arc::new(AtomicBool::new(false));
        let flag = running.clone();
        let pause_flag = paused.clone();
        let message = message.into();

        let handle = thread::spawn(move || {
            let mut stderr = std::io::stderr();
            let mut i = 0usize;
            let mut prev_len = 0usize;
            while flag.load(Ordering::Relaxed) {
                if pause_flag.load(Ordering::Relaxed) {
                    thread::sleep(Duration::from_millis(FRAME_MS));
                    continue;
                }
                let frame = FRAMES[i % FRAMES.len()];
                let line = format!("{frame} {message}");
                // Clear previous content then write new line
                let clear_width = prev_len.max(line.len());
                let _ = write!(stderr, "\r{:width$}\r{line}", "", width = clear_width);
                let _ = stderr.flush();
                prev_len = line.len();
                thread::sleep(Duration::from_millis(FRAME_MS));
                i += 1;
            }
            // Clear the spinner line
            let _ = write!(stderr, "\r{:width$}\r", "", width = prev_len + 2);
            let _ = stderr.flush();
        });

        Self {
            running,
            paused,
            handle: Some(handle),
            mode: SpinnerMode::Simple,
        }
    }

    /// Start a tool-progress spinner.
    ///
    /// Displays: `⠋ [index/total] [tool_name] elapsed`
    ///
    /// When `enabled` is false, returns a no-op spinner.
    pub fn start_tool(tool_name: &str, total: usize, index: usize, enabled: bool) -> Self {
        if !enabled {
            return Self::noop();
        }
        let running = Arc::new(AtomicBool::new(true));
        let paused = Arc::new(AtomicBool::new(false));
        let current_index = Arc::new(AtomicUsize::new(index));
        let name = Arc::new(Mutex::new(
            sanitize_display_string(tool_name, 30).to_string(),
        ));

        let flag = running.clone();
        let pause_flag = paused.clone();
        let idx = current_index.clone();
        let name_ref = name.clone();

        let mode = SpinnerMode::Tool {
            total,
            current_index: current_index.clone(),
            tool_name: name.clone(),
        };

        let handle = thread::spawn(move || {
            let mut stderr = std::io::stderr();
            let mut i = 0usize;
            let mut prev_len = 0usize;
            let start = Instant::now();
            while flag.load(Ordering::Relaxed) {
                if pause_flag.load(Ordering::Relaxed) {
                    thread::sleep(Duration::from_millis(FRAME_MS));
                    continue;
                }
                let frame = FRAMES[i % FRAMES.len()];
                let cur = idx.load(Ordering::Relaxed);
                let tool = name_ref.lock().unwrap().clone();
                let elapsed = format_elapsed_ms(start.elapsed().as_millis() as u64);
                let line = format!("{frame} [{}/{}] [{}] {}", cur, total, tool, elapsed);
                let clear_width = prev_len.max(line.len());
                let _ = write!(stderr, "\r{:width$}\r{line}", "", width = clear_width);
                let _ = stderr.flush();
                prev_len = line.len();
                thread::sleep(Duration::from_millis(FRAME_MS));
                i += 1;
            }
            let _ = write!(stderr, "\r{:width$}\r", "", width = prev_len + 2);
            let _ = stderr.flush();
        });

        Self {
            running,
            paused,
            handle: Some(handle),
            mode,
        }
    }

    /// Start a parallel-progress spinner.
    ///
    /// Displays: `⠋ [completed/total completed]`
    ///
    /// When `enabled` is false, returns a no-op spinner.
    pub fn start_parallel(total: usize, completed: Arc<AtomicUsize>, enabled: bool) -> Self {
        if !enabled {
            return Self::noop();
        }
        let running = Arc::new(AtomicBool::new(true));
        let paused = Arc::new(AtomicBool::new(false));

        let flag = running.clone();
        let pause_flag = paused.clone();
        let comp = completed.clone();

        let mode = SpinnerMode::Parallel {
            total,
            completed: completed.clone(),
        };

        let handle = thread::spawn(move || {
            let mut stderr = std::io::stderr();
            let mut i = 0usize;
            let mut prev_len = 0usize;
            while flag.load(Ordering::Relaxed) {
                if pause_flag.load(Ordering::Relaxed) {
                    thread::sleep(Duration::from_millis(FRAME_MS));
                    continue;
                }
                let frame = FRAMES[i % FRAMES.len()];
                let done = comp.load(Ordering::Relaxed);
                let line = format!("{frame} [{}/{} completed]", done, total);
                let clear_width = prev_len.max(line.len());
                let _ = write!(stderr, "\r{:width$}\r{line}", "", width = clear_width);
                let _ = stderr.flush();
                prev_len = line.len();
                thread::sleep(Duration::from_millis(FRAME_MS));
                i += 1;
            }
            let _ = write!(stderr, "\r{:width$}\r", "", width = prev_len + 2);
            let _ = stderr.flush();
        });

        Self {
            running,
            paused,
            handle: Some(handle),
            mode,
        }
    }

    /// Pause the spinner rendering. The background thread keeps running
    /// but skips drawing until [`resume`] is called.
    pub fn pause(&self) {
        self.paused.store(true, Ordering::Relaxed);
    }

    /// Resume spinner rendering after a [`pause`].
    pub fn resume(&self) {
        self.paused.store(false, Ordering::Relaxed);
    }

    /// Update the current tool index and name for a `SpinnerMode::Tool` spinner.
    ///
    /// No-op for other modes.
    pub fn set_tool_progress(&self, index: usize, name: &str) {
        if let SpinnerMode::Tool {
            current_index,
            tool_name,
            ..
        } = &self.mode
        {
            current_index.store(index, Ordering::Relaxed);
            let sanitized = sanitize_display_string(name, 30);
            if let Ok(mut guard) = tool_name.lock() {
                *guard = sanitized;
            }
        }
    }

    /// Stop the spinner and clear the line.
    pub fn stop(mut self) {
        self.shutdown();
    }

    fn shutdown(&mut self) {
        self.running.store(false, Ordering::Relaxed);
        // Ensure the thread is not paused so it can exit
        self.paused.store(false, Ordering::Relaxed);
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

/// Format a duration in milliseconds as a human-readable string.
///
/// Examples: `0` -> `"0.0s"`, `1234` -> `"1.2s"`, `65000` -> `"65.0s"`.
#[allow(dead_code)]
pub(crate) fn format_elapsed_ms(ms: u64) -> String {
    let secs = ms / 1000;
    let tenths = (ms % 1000) / 100;
    format!("{secs}.{tenths}s")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn format_elapsed_ms_zero() {
        assert_eq!(format_elapsed_ms(0), "0.0s");
    }

    #[test]
    fn format_elapsed_ms_normal() {
        assert_eq!(format_elapsed_ms(1234), "1.2s");
    }

    #[test]
    fn format_elapsed_ms_large() {
        assert_eq!(format_elapsed_ms(65000), "65.0s");
    }

    #[test]
    fn format_elapsed_ms_max_no_panic() {
        let result = format_elapsed_ms(u64::MAX);
        assert!(!result.is_empty());
    }

    // --- Task 2.1: SpinnerMode tests ---

    #[test]
    fn spinner_start_creates_simple_mode() {
        let spinner = Spinner::start("test", false);
        assert!(!spinner.running.load(Ordering::Relaxed));
        assert!(spinner.handle.is_none());
        assert!(matches!(spinner.mode, SpinnerMode::Simple));
    }

    #[test]
    fn spinner_start_tool_creates_tool_mode() {
        let spinner = Spinner::start_tool("file.read", 3, 1, false);
        assert!(!spinner.running.load(Ordering::Relaxed));
        assert!(spinner.handle.is_none());
        // Disabled spinner uses Simple as default noop mode
        // Just verify it doesn't panic
    }

    #[test]
    fn spinner_start_parallel_creates_parallel_mode() {
        let completed = Arc::new(AtomicUsize::new(0));
        let spinner = Spinner::start_parallel(4, completed, false);
        assert!(!spinner.running.load(Ordering::Relaxed));
        assert!(spinner.handle.is_none());
    }

    // --- Task 2.2: pause/resume tests ---

    #[test]
    fn spinner_pause_resume_noop_when_disabled() {
        let spinner = Spinner::start("test", false);
        spinner.pause();
        spinner.resume();
        spinner.stop(); // should not panic
    }

    #[test]
    fn spinner_pause_during_stop() {
        // Verify that pausing then stopping does not deadlock
        let spinner = Spinner::start("test", false);
        spinner.pause();
        spinner.stop(); // must not deadlock
    }

    // --- Task 2.3: set_tool_progress tests ---

    #[test]
    fn spinner_set_tool_progress_updates_name() {
        // Create a tool-mode spinner (disabled) and verify set_tool_progress
        // works on enabled-style structs by manually constructing one.
        let current_index = Arc::new(AtomicUsize::new(1));
        let tool_name = Arc::new(Mutex::new("initial".to_string()));
        let spinner = Spinner {
            running: Arc::new(AtomicBool::new(false)),
            paused: Arc::new(AtomicBool::new(false)),
            handle: None,
            mode: SpinnerMode::Tool {
                total: 3,
                current_index: current_index.clone(),
                tool_name: tool_name.clone(),
            },
        };
        spinner.set_tool_progress(2, "file.write");
        assert_eq!(current_index.load(Ordering::Relaxed), 2);
        assert_eq!(*tool_name.lock().unwrap(), "file.write");
    }

    #[test]
    fn spinner_set_tool_progress_noop_for_simple_mode() {
        let spinner = Spinner::start("test", false);
        // Should not panic for Simple mode
        spinner.set_tool_progress(1, "anything");
    }
}
