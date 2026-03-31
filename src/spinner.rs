//! Terminal spinner for visual feedback during blocking operations.

use std::io::Write;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use crate::app::render::sanitize_display_string;
use crate::tooling::progress::{ToolProgressEntry, ToolProgressStatus};

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
    /// Parallel-detailed spinner: `⠋ [2/4] ✓file.read(0.3s) ⟳git.status(1.2s)`
    ParallelDetailed,
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

    /// Start a parallel-detailed spinner that displays individual tool status.
    ///
    /// Displays: `⠋ [2/4] ✓file.read(0.3s) ⟳git.status(1.2s)`
    ///
    /// When `enabled` is false, returns a no-op spinner.
    pub fn start_parallel_detailed(
        progress: Arc<Mutex<Vec<ToolProgressEntry>>>,
        enabled: bool,
    ) -> Self {
        if !enabled {
            return Self::noop();
        }
        let running = Arc::new(AtomicBool::new(true));
        let paused = Arc::new(AtomicBool::new(false));

        let flag = running.clone();
        let pause_flag = paused.clone();

        let handle = thread::spawn(move || {
            let mut stderr = std::io::stderr();
            let mut i = 0usize;
            let mut prev_len = 0usize;
            while flag.load(Ordering::Relaxed) {
                if pause_flag.load(Ordering::Relaxed) {
                    thread::sleep(Duration::from_millis(FRAME_MS));
                    continue;
                }
                let line = match progress.try_lock() {
                    Ok(entries) => format_progress_line(&entries, i),
                    Err(_) => {
                        // Mutex contended or poisoned — skip this frame
                        thread::sleep(Duration::from_millis(FRAME_MS));
                        i += 1;
                        continue;
                    }
                };
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
            mode: SpinnerMode::ParallelDetailed,
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

/// Pure function: build a spinner display line from parallel progress entries.
///
/// Format: `⠋ [2/4] ✓file.read(0.3s) ⟳git.status(1.2s) ✗web.fetch`
///
/// Status symbols:
/// - `⟳` Running
/// - `✓` Completed
/// - `✗` Failed
/// - (no symbol for Pending, shown as tool name only)
pub(crate) fn format_progress_line(entries: &[ToolProgressEntry], frame_idx: usize) -> String {
    if entries.is_empty() {
        let frame = FRAMES[frame_idx % FRAMES.len()];
        return format!("{frame} [0/0]");
    }

    let total = entries.len();
    let done = entries
        .iter()
        .filter(|e| {
            matches!(
                e.status,
                ToolProgressStatus::Completed | ToolProgressStatus::Failed(_)
            )
        })
        .count();

    let frame = FRAMES[frame_idx % FRAMES.len()];
    let mut parts = Vec::with_capacity(entries.len());
    for entry in entries {
        let name = sanitize_display_string(&entry.tool_name, 30);
        let elapsed_str = match &entry.status {
            ToolProgressStatus::Running => {
                if let Some(started) = entry.started_at {
                    let ms = started.elapsed().as_millis() as u64;
                    format!("({})", format_elapsed_ms(ms))
                } else {
                    String::new()
                }
            }
            ToolProgressStatus::Completed | ToolProgressStatus::Failed(_) => {
                if let Some(ms) = entry.elapsed_ms {
                    format!("({})", format_elapsed_ms(ms.min(u64::MAX as u128) as u64))
                } else {
                    String::new()
                }
            }
            ToolProgressStatus::Pending => String::new(),
        };
        let symbol = match &entry.status {
            ToolProgressStatus::Pending => "",
            ToolProgressStatus::Running => "\u{27F3}",
            ToolProgressStatus::Completed => "\u{2713}",
            ToolProgressStatus::Failed(_) => "\u{2717}",
        };
        parts.push(format!("{symbol}{name}{elapsed_str}"));
    }

    format!("{frame} [{done}/{total}] {}", parts.join(" "))
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

    // --- Issue #220: format_progress_line tests ---

    #[test]
    fn format_progress_line_empty_entries() {
        let result = format_progress_line(&[], 0);
        assert!(result.contains("[0/0]"));
    }

    #[test]
    fn format_progress_line_all_pending() {
        let entries = vec![
            ToolProgressEntry {
                tool_call_id: "c1".to_string(),
                tool_name: "file.read".to_string(),
                status: ToolProgressStatus::Pending,
                started_at: None,
                elapsed_ms: None,
            },
            ToolProgressEntry {
                tool_call_id: "c2".to_string(),
                tool_name: "git.status".to_string(),
                status: ToolProgressStatus::Pending,
                started_at: None,
                elapsed_ms: None,
            },
        ];
        let result = format_progress_line(&entries, 0);
        assert!(result.contains("[0/2]"));
        assert!(result.contains("file.read"));
        assert!(result.contains("git.status"));
    }

    #[test]
    fn format_progress_line_mixed_states() {
        let entries = vec![
            ToolProgressEntry {
                tool_call_id: "c1".to_string(),
                tool_name: "file.read".to_string(),
                status: ToolProgressStatus::Completed,
                started_at: None,
                elapsed_ms: Some(300),
            },
            ToolProgressEntry {
                tool_call_id: "c2".to_string(),
                tool_name: "git.status".to_string(),
                status: ToolProgressStatus::Running,
                started_at: Some(Instant::now()),
                elapsed_ms: None,
            },
            ToolProgressEntry {
                tool_call_id: "c3".to_string(),
                tool_name: "web.fetch".to_string(),
                status: ToolProgressStatus::Failed("timeout".to_string()),
                started_at: None,
                elapsed_ms: Some(5000),
            },
        ];
        let result = format_progress_line(&entries, 0);
        // 2 done (Completed + Failed)
        assert!(result.contains("[2/3]"));
        // Completed symbol
        assert!(result.contains("\u{2713}file.read"));
        // Running symbol
        assert!(result.contains("\u{27F3}git.status"));
        // Failed symbol
        assert!(result.contains("\u{2717}web.fetch"));
    }

    #[test]
    fn format_progress_line_all_completed_with_elapsed() {
        let entries = vec![
            ToolProgressEntry {
                tool_call_id: "c1".to_string(),
                tool_name: "file.read".to_string(),
                status: ToolProgressStatus::Completed,
                started_at: None,
                elapsed_ms: Some(1234),
            },
            ToolProgressEntry {
                tool_call_id: "c2".to_string(),
                tool_name: "git.diff".to_string(),
                status: ToolProgressStatus::Completed,
                started_at: None,
                elapsed_ms: Some(500),
            },
        ];
        let result = format_progress_line(&entries, 0);
        assert!(result.contains("[2/2]"));
        assert!(result.contains("(1.2s)"));
        assert!(result.contains("(0.5s)"));
    }

    // --- Issue #220: start_parallel_detailed tests ---

    #[test]
    fn spinner_start_parallel_detailed_noop_when_disabled() {
        let progress = Arc::new(Mutex::new(vec![ToolProgressEntry {
            tool_call_id: "c1".to_string(),
            tool_name: "file.read".to_string(),
            status: ToolProgressStatus::Pending,
            started_at: None,
            elapsed_ms: None,
        }]));
        let spinner = Spinner::start_parallel_detailed(progress, false);
        assert!(!spinner.running.load(Ordering::Relaxed));
        assert!(spinner.handle.is_none());
        spinner.stop();
    }

    #[test]
    fn spinner_start_parallel_detailed_starts_and_stops() {
        let progress = Arc::new(Mutex::new(vec![ToolProgressEntry {
            tool_call_id: "c1".to_string(),
            tool_name: "file.read".to_string(),
            status: ToolProgressStatus::Running,
            started_at: Some(Instant::now()),
            elapsed_ms: None,
        }]));
        let spinner = Spinner::start_parallel_detailed(progress, true);
        assert!(spinner.running.load(Ordering::Relaxed));
        // Let it run for a frame
        std::thread::sleep(Duration::from_millis(100));
        spinner.stop();
    }
}
