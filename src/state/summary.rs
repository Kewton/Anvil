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
    pub keep_recent_events: usize,
}

impl Default for SummaryPolicy {
    fn default() -> Self {
        Self {
            summary_trigger_tokens: 48_000,
            summary_trigger_turns: 20,
            subagent_trigger_tokens: 96_000,
            subagent_trigger_turns: 28,
            keep_recent_events: 8,
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

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CarryoverState {
    pub rolling_summary: Option<String>,
    pub summarized_events: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompactionOutcome {
    pub rolling_summary: String,
    pub retained_events: usize,
    pub summarized_events: usize,
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

    pub fn policy(&self) -> SummaryPolicy {
        self.policy
    }

    pub fn should_summarize(&self, input: SummaryInput) -> bool {
        self.policy.should_summarize(input)
    }

    pub fn estimate_tokens(&self, history: &[String], existing_summary: Option<&str>) -> usize {
        let history_chars = history
            .iter()
            .map(|entry| entry.chars().count())
            .sum::<usize>();
        let summary_chars = existing_summary
            .map(|text| text.chars().count())
            .unwrap_or(0);
        (history_chars + summary_chars) / 4
    }

    pub fn summarize_history(&self, history: &[String]) -> String {
        self.summarize_for_carryover(None, history)
    }

    pub fn summarize_for_carryover(
        &self,
        existing_summary: Option<&str>,
        history: &[String],
    ) -> String {
        let user_goals = collect_recent(history, &["You", "User", "TASK"], 2);
        let accepted_facts = collect_recent(history, &["Tool", "TOOL_RESULT"], 3);
        let pending_tasks = collect_recent(history, &["Anvil", "Agent", "TOOL_ERROR"], 2);
        let changed_files = collect_paths(history, 4);

        let mut lines = Vec::new();
        lines.push("Rolling summary".to_string());
        if let Some(existing) = existing_summary.filter(|text| !text.trim().is_empty()) {
            lines.push(format!("Prior summary: {}", truncate(existing, 220)));
        }
        if !user_goals.is_empty() {
            lines.push(format!("User goals: {}", user_goals.join(" | ")));
        }
        if !accepted_facts.is_empty() {
            lines.push(format!("Accepted facts: {}", accepted_facts.join(" | ")));
        }
        if !pending_tasks.is_empty() {
            lines.push(format!("Pending tasks: {}", pending_tasks.join(" | ")));
        }
        if !changed_files.is_empty() {
            lines.push(format!("Changed files: {}", changed_files.join(", ")));
        }
        truncate(&lines.join("\n"), 900)
    }

    pub fn compact_history(
        &self,
        existing_summary: Option<&str>,
        history: &[String],
    ) -> Option<CompactionOutcome> {
        if history.len() <= self.policy.keep_recent_events {
            return None;
        }
        let retained_events = self.policy.keep_recent_events;
        let summarized_events = history.len().saturating_sub(retained_events);
        Some(CompactionOutcome {
            rolling_summary: self.summarize_for_carryover(existing_summary, history),
            retained_events,
            summarized_events,
        })
    }

    pub fn prompt_prefix(&self, carryover: &CarryoverState) -> Option<String> {
        let summary = carryover.rolling_summary.as_deref()?.trim();
        if summary.is_empty() {
            return None;
        }
        Some(format!(
            "Session carryover summary:\n{}\nSummarized prior events: {}\n",
            summary, carryover.summarized_events
        ))
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

fn collect_recent(history: &[String], markers: &[&str], max_items: usize) -> Vec<String> {
    history
        .iter()
        .rev()
        .filter(|entry| markers.iter().any(|marker| entry.contains(marker)))
        .take(max_items)
        .map(|entry| truncate(entry, 120))
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect()
}

fn collect_paths(history: &[String], max_items: usize) -> Vec<String> {
    let mut paths = Vec::new();
    for entry in history.iter().rev() {
        for token in entry.split_whitespace() {
            if token.starts_with("./") || token.starts_with('/') {
                let trimmed = token.trim_matches(|ch: char| {
                    matches!(ch, '`' | '"' | '\'' | ',' | '.' | ')' | '(' | ':' | ';')
                });
                if !paths.iter().any(|existing| existing == trimmed) {
                    paths.push(trimmed.to_string());
                    if paths.len() >= max_items {
                        return paths.into_iter().rev().collect();
                    }
                }
            }
        }
    }
    paths.into_iter().rev().collect()
}

fn truncate(text: &str, max_chars: usize) -> String {
    if text.chars().count() <= max_chars {
        return text.to_string();
    }
    text.chars().take(max_chars).collect()
}
