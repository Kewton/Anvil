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

    pub fn record_tool_call(&mut self, tool_name: &str, success: bool) -> ReadTransitionAction {
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
             Do NOT call file.read again unless a previous edit failed and you need one \
             targeted verification read.",
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
        assert_eq!(
            guard.record_tool_call("shell.exec", true),
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
}
