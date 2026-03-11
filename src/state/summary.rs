use std::time::Duration;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SummaryInput {
    pub tokens: usize,
    pub turns: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SummaryPolicy {
    pub summary_trigger_tokens: usize,
    pub summary_trigger_turns: usize,
    pub subagent_trigger_tokens: usize,
    pub subagent_trigger_turns: usize,
}

impl Default for SummaryPolicy {
    fn default() -> Self {
        Self {
            summary_trigger_tokens: 48_000,
            summary_trigger_turns: 20,
            subagent_trigger_tokens: 96_000,
            subagent_trigger_turns: 28,
        }
    }
}

impl SummaryPolicy {
    pub fn should_summarize(&self, input: SummaryInput) -> bool {
        input.tokens >= self.summary_trigger_tokens
            || input.turns >= self.summary_trigger_turns
            || input.tokens >= self.subagent_trigger_tokens
    }
}

#[derive(Debug, Clone)]
pub struct SummaryController {
    policy: SummaryPolicy,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LatencyBudget {
    pub summary_max: Duration,
    pub subagent_max: Duration,
}

impl Default for LatencyBudget {
    fn default() -> Self {
        Self {
            summary_max: Duration::from_millis(800),
            subagent_max: Duration::from_secs(5),
        }
    }
}

impl SummaryController {
    pub fn new(policy: SummaryPolicy) -> Self {
        Self { policy }
    }

    pub fn should_summarize(&self, input: SummaryInput) -> bool {
        self.policy.should_summarize(input)
    }

    pub fn summarize_history(&self, history: &[String]) -> String {
        let mut preview = history
            .iter()
            .take(6)
            .map(|entry| truncate(entry, 100))
            .collect::<Vec<_>>()
            .join(" | ");
        if preview.len() > 760 {
            preview = truncate(&preview, 760);
        }
        format!("Rolling summary: {preview}")
    }

    pub fn truncate_tool_output(&self, text: &str, max_chars: usize) -> String {
        if text.chars().count() <= max_chars {
            return text.to_string();
        }
        format!(
            "{} ... [truncated {} chars]",
            truncate(text, max_chars),
            text.chars().count() - max_chars
        )
    }

    pub fn should_spawn_subagent(&self, tokens: usize, turns: usize) -> bool {
        tokens >= self.policy.subagent_trigger_tokens || turns >= self.policy.subagent_trigger_turns
    }

    pub fn within_summary_budget(&self, elapsed: Duration, budget: LatencyBudget) -> bool {
        elapsed <= budget.summary_max
    }

    pub fn within_subagent_budget(&self, elapsed: Duration, budget: LatencyBudget) -> bool {
        elapsed <= budget.subagent_max
    }
}

fn truncate(text: &str, max_chars: usize) -> String {
    text.chars().take(max_chars).collect()
}
