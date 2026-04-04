/// Action recommended when exploration has continued for too long.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReadTransitionAction {
    /// No action needed.
    Continue,
    /// Inject a system message urging the model to transition to implementation.
    Inject(String),
}

/// Guards against over-exploration by tracking consecutive read/search calls.
///
/// Unlike `LoopDetector`, this policy intentionally triggers even when the
/// agent reads different files each time. The goal is to force the transition
/// from exploration to implementation once enough context has been gathered.
pub struct ReadTransitionGuard {
    consecutive_exploration_calls: usize,
    consecutive_file_reads: usize,
    transition_threshold: usize,
    reinject_interval: usize,
    last_injected_at: Option<usize>,
}

impl ReadTransitionGuard {
    pub fn new(transition_threshold: usize, reinject_interval: usize) -> Self {
        Self {
            consecutive_exploration_calls: 0,
            consecutive_file_reads: 0,
            transition_threshold,
            reinject_interval,
            last_injected_at: None,
        }
    }

    /// Override the transition_threshold with a runtime-effective value (Issue #263).
    ///
    /// Only `transition_threshold` is changed; `reinject_interval` remains
    /// at its configured baseline.
    pub fn set_effective_threshold(&mut self, threshold: usize) {
        self.transition_threshold = threshold.max(3);
    }

    pub fn reset(&mut self) {
        self.consecutive_exploration_calls = 0;
        self.consecutive_file_reads = 0;
        self.last_injected_at = None;
    }

    /// Record a tool call, optionally with the shell command string for
    /// `shell.exec` file-reading detection (Issue #265).
    ///
    /// When `shell_command` is `Some`, and the command is a file-reading
    /// operation (grep/sed/cat/…), it counts as an exploration call instead
    /// of resetting the guard.
    pub fn record_tool_call_ex(
        &mut self,
        tool_name: &str,
        success: bool,
        shell_command: Option<&str>,
    ) -> ReadTransitionAction {
        self.record_inner(tool_name, success, shell_command)
    }

    pub fn record_tool_call(&mut self, tool_name: &str, success: bool) -> ReadTransitionAction {
        self.record_inner(tool_name, success, None)
    }

    fn record_inner(
        &mut self,
        tool_name: &str,
        success: bool,
        shell_command: Option<&str>,
    ) -> ReadTransitionAction {
        if !success {
            return ReadTransitionAction::Continue;
        }

        match tool_name {
            "file.read" => {
                self.consecutive_exploration_calls += 1;
                self.consecutive_file_reads += 1;
            }
            "file.search" | "web.fetch" => {
                self.consecutive_exploration_calls += 1;
            }
            "file.edit" | "file.edit_anchor" | "file.write" => {
                self.reset();
                return ReadTransitionAction::Continue;
            }
            "shell.exec"
                if shell_command
                    .is_some_and(crate::tooling::shell_policy::is_file_read_shell_command) =>
            {
                // Issue #265: grep/sed/cat etc. count as exploration, not a reset
                self.consecutive_exploration_calls += 1;
            }
            _ => {
                self.consecutive_exploration_calls = 0;
                self.consecutive_file_reads = 0;
                self.last_injected_at = None;
                return ReadTransitionAction::Continue;
            }
        }

        if self.consecutive_exploration_calls < self.transition_threshold {
            return ReadTransitionAction::Continue;
        }

        let should_inject = match self.last_injected_at {
            None => true,
            Some(last) => {
                self.consecutive_exploration_calls.saturating_sub(last) >= self.reinject_interval
            }
        };
        if !should_inject {
            return ReadTransitionAction::Continue;
        }

        self.last_injected_at = Some(self.consecutive_exploration_calls);
        ReadTransitionAction::Inject(format!(
            "[System] You have already spent {} consecutive tool calls exploring \
             (including {} file.read calls). You have enough context. \
             Start implementing now using file.edit, file.edit_anchor, or file.write. \
             Do NOT call file.read or use shell.exec with grep/sed/cat to read files. \
             Proceed to implementation immediately.",
            self.consecutive_exploration_calls, self.consecutive_file_reads
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn injects_at_threshold() {
        let mut guard = ReadTransitionGuard::new(4, 2);
        for _ in 0..3 {
            assert_eq!(
                guard.record_tool_call("file.read", true),
                ReadTransitionAction::Continue
            );
        }
        let action = guard.record_tool_call("file.read", true);
        assert!(matches!(action, ReadTransitionAction::Inject(_)));
    }

    #[test]
    fn reinjects_on_interval() {
        let mut guard = ReadTransitionGuard::new(4, 2);
        for _ in 0..4 {
            guard.record_tool_call("file.read", true);
        }
        assert_eq!(
            guard.record_tool_call("file.read", true),
            ReadTransitionAction::Continue
        );
        let action = guard.record_tool_call("file.search", true);
        assert!(matches!(action, ReadTransitionAction::Inject(_)));
    }

    #[test]
    fn successful_write_resets_guard() {
        let mut guard = ReadTransitionGuard::new(3, 2);
        for _ in 0..3 {
            guard.record_tool_call("file.read", true);
        }
        assert_eq!(
            guard.record_tool_call("file.edit", true),
            ReadTransitionAction::Continue
        );
        assert_eq!(
            guard.record_tool_call("file.read", true),
            ReadTransitionAction::Continue
        );
    }

    #[test]
    fn non_exploration_tools_clear_streak() {
        let mut guard = ReadTransitionGuard::new(3, 2);
        guard.record_tool_call("file.read", true);
        guard.record_tool_call("file.search", true);
        // Non-file-reading shell.exec resets the streak
        assert_eq!(
            guard.record_tool_call_ex("shell.exec", true, Some("cargo test")),
            ReadTransitionAction::Continue
        );
        assert_eq!(
            guard.record_tool_call("file.read", true),
            ReadTransitionAction::Continue
        );
    }

    #[test]
    fn failed_calls_do_not_advance_guard() {
        let mut guard = ReadTransitionGuard::new(2, 1);
        assert_eq!(
            guard.record_tool_call("file.read", false),
            ReadTransitionAction::Continue
        );
        assert_eq!(
            guard.record_tool_call("file.read", true),
            ReadTransitionAction::Continue
        );
    }

    // --- Issue #265: shell.exec file-reading detection ---

    #[test]
    fn shell_exec_grep_counts_as_exploration() {
        let mut guard = ReadTransitionGuard::new(3, 2);
        guard.record_tool_call("file.read", true);
        guard.record_tool_call("file.read", true);
        // grep should count as exploration, not reset
        let action =
            guard.record_tool_call_ex("shell.exec", true, Some("grep -n pattern src/main.rs"));
        assert!(matches!(action, ReadTransitionAction::Inject(_)));
    }

    #[test]
    fn shell_exec_sed_counts_as_exploration() {
        let mut guard = ReadTransitionGuard::new(3, 2);
        guard.record_tool_call("file.read", true);
        guard.record_tool_call("file.read", true);
        let action =
            guard.record_tool_call_ex("shell.exec", true, Some("sed -n '10,20p' src/main.rs"));
        assert!(matches!(action, ReadTransitionAction::Inject(_)));
    }

    #[test]
    fn shell_exec_cat_counts_as_exploration() {
        let mut guard = ReadTransitionGuard::new(3, 2);
        guard.record_tool_call("file.read", true);
        guard.record_tool_call("file.read", true);
        let action = guard.record_tool_call_ex("shell.exec", true, Some("cat src/main.rs"));
        assert!(matches!(action, ReadTransitionAction::Inject(_)));
    }

    #[test]
    fn shell_exec_non_read_resets_streak() {
        let mut guard = ReadTransitionGuard::new(3, 2);
        guard.record_tool_call("file.read", true);
        guard.record_tool_call("file.read", true);
        // cargo build is not a file-reading command → resets
        assert_eq!(
            guard.record_tool_call_ex("shell.exec", true, Some("cargo build")),
            ReadTransitionAction::Continue
        );
        // After reset, need 3 more to trigger
        assert_eq!(
            guard.record_tool_call("file.read", true),
            ReadTransitionAction::Continue
        );
    }

    #[test]
    fn shell_exec_without_command_resets_streak() {
        let mut guard = ReadTransitionGuard::new(3, 2);
        guard.record_tool_call("file.read", true);
        guard.record_tool_call("file.read", true);
        // No command provided → falls through to default reset
        assert_eq!(
            guard.record_tool_call_ex("shell.exec", true, None),
            ReadTransitionAction::Continue
        );
    }

    #[test]
    fn mixed_file_read_and_shell_grep_triggers_guard() {
        let mut guard = ReadTransitionGuard::new(4, 2);
        guard.record_tool_call("file.read", true);
        guard.record_tool_call_ex("shell.exec", true, Some("grep -rn TODO src/"));
        guard.record_tool_call("file.read", true);
        let action =
            guard.record_tool_call_ex("shell.exec", true, Some("sed -n '1,50p' src/lib.rs"));
        assert!(matches!(action, ReadTransitionAction::Inject(_)));
    }
}
