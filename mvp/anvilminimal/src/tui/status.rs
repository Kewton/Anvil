use crate::config::{Config, Provider};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UiStatus {
    pub mode: String,
    pub provider: String,
    pub model: String,
    pub context_budget: usize,
    pub yes: bool,
    pub prompt_tokens: Option<u64>,
    pub completion_tokens: Option<u64>,
}

impl UiStatus {
    pub fn from_config(config: &Config) -> Self {
        Self {
            mode: "act".to_string(),
            provider: provider_label(config.provider).to_string(),
            model: config.model.clone(),
            context_budget: config.context_budget,
            yes: config.yes,
            prompt_tokens: None,
            completion_tokens: None,
        }
    }

    pub fn for_model_reply(
        config: &Config,
        model: &str,
        provider: &str,
        prompt: Option<u64>,
        completion: Option<u64>,
    ) -> Self {
        Self {
            mode: "act".to_string(),
            provider: provider.to_string(),
            model: model.to_string(),
            context_budget: config.context_budget,
            yes: config.yes,
            prompt_tokens: prompt,
            completion_tokens: completion,
        }
    }

    pub fn token_total(&self) -> Option<u64> {
        match (self.prompt_tokens, self.completion_tokens) {
            (None, None) => None,
            (prompt, completion) => Some(prompt.unwrap_or(0) + completion.unwrap_or(0)),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TaskFailureBlock {
    pub task_status: String,
    pub failed_phase: String,
    pub completed_phases: Option<usize>,
    pub total_phases: Option<usize>,
    pub primary_stop_reason: String,
    pub recovery_prompt_command: String,
    pub recovery_ultra_plan_command: String,
    pub summary_path: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TaskIncompleteNotice {
    pub status: String,
    pub task_status: String,
    pub summary_path: String,
}

pub fn render_task_failure_block(block: &TaskFailureBlock) -> String {
    [
        "================ TASK FAILED (process exited normally) ================".to_string(),
        format!("Task status: {}", display_or_missing(&block.task_status)),
        format!("Failed phase: {}", display_or_missing(&block.failed_phase)),
        format!("Phases completed: {}", phase_progress(block)),
        format!(
            "Primary stop reason: {}",
            display_or_missing(&block.primary_stop_reason)
        ),
        format!(
            "Recovery prompt command: {}",
            display_or_missing(&block.recovery_prompt_command)
        ),
        format!(
            "Recovery UltraPlan command: {}",
            display_or_missing(&block.recovery_ultra_plan_command)
        ),
        format!("Run summary: {}", display_or_missing(&block.summary_path)),
        "=======================================================================".to_string(),
    ]
    .join("\n")
}

pub fn render_task_incomplete_notice(notice: &TaskIncompleteNotice) -> String {
    format!(
        "TASK INCOMPLETE (command returned; partial artifacts may exist): task_status={}; status={}; summary={}",
        display_or_missing(&notice.task_status),
        display_or_missing(&notice.status),
        display_or_missing(&notice.summary_path),
    )
}

fn phase_progress(block: &TaskFailureBlock) -> String {
    match (block.completed_phases, block.total_phases) {
        (Some(completed), Some(total)) => format!("{completed}/{total}"),
        (Some(completed), None) => format!("{completed}/unknown"),
        (None, Some(total)) => format!("unknown/{total}"),
        (None, None) => "unknown".to_string(),
    }
}

fn display_or_missing(value: &str) -> &str {
    if value.trim().is_empty() {
        "missing"
    } else {
        value
    }
}

fn provider_label(provider: Provider) -> &'static str {
    match provider {
        Provider::Ollama => "ollama",
        Provider::Openai => "openai",
        Provider::Gemini => "gemini",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn status_token_total_unknown_stays_none() {
        let status = UiStatus {
            mode: "act".to_string(),
            provider: "gemini".to_string(),
            model: "m".to_string(),
            context_budget: 1000,
            yes: true,
            prompt_tokens: None,
            completion_tokens: None,
        };
        assert_eq!(status.token_total(), None);
    }

    #[test]
    fn status_token_total_uses_known_values_only() {
        let status = UiStatus {
            mode: "act".to_string(),
            provider: "ollama".to_string(),
            model: "m".to_string(),
            context_budget: 1000,
            yes: true,
            prompt_tokens: Some(10),
            completion_tokens: None,
        };
        assert_eq!(status.token_total(), Some(10));
    }

    #[test]
    fn task_failure_block_contains_phase_reason_recovery_commands_and_summary() {
        let block = TaskFailureBlock {
            task_status: "failed".to_string(),
            failed_phase: "project-setup".to_string(),
            completed_phases: Some(1),
            total_phases: Some(3),
            primary_stop_reason: "dependency_setup_authority_required".to_string(),
            recovery_prompt_command:
                "/ultra-plan-run --profile nextjs \"$(cat .anvil/repairs/repair-project-setup.md)\""
                    .to_string(),
            recovery_ultra_plan_command:
                "/run-ultra-plan .anvil/plans/recovery-ultra-plan-project-setup.yaml".to_string(),
            summary_path: ".anvil/runs/123/summary.md".to_string(),
        };

        let rendered = render_task_failure_block(&block);

        assert!(rendered.contains("TASK FAILED (process exited normally)"));
        assert!(rendered.contains("Task status: failed"));
        assert!(rendered.contains("Failed phase: project-setup"));
        assert!(rendered.contains("Phases completed: 1/3"));
        assert!(rendered.contains("Primary stop reason: dependency_setup_authority_required"));
        assert!(rendered.contains("Recovery prompt command: /ultra-plan-run --profile nextjs"));
        assert!(rendered.contains(
            "Recovery UltraPlan command: /run-ultra-plan .anvil/plans/recovery-ultra-plan-project-setup.yaml"
        ));
        assert!(rendered.contains("Run summary: .anvil/runs/123/summary.md"));
    }

    #[test]
    fn incomplete_notice_mentions_partial_artifacts_and_summary() {
        let notice = TaskIncompleteNotice {
            status: "complete_with_partial_release_gate".to_string(),
            task_status: "partial".to_string(),
            summary_path: ".anvil/runs/123/summary.md".to_string(),
        };

        let rendered = render_task_incomplete_notice(&notice);

        assert!(rendered.contains("TASK INCOMPLETE (command returned"));
        assert!(rendered.contains("task_status=partial"));
        assert!(rendered.contains("status=complete_with_partial_release_gate"));
        assert!(rendered.contains("summary=.anvil/runs/123/summary.md"));
    }
}
