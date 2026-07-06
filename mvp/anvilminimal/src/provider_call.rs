use std::sync::mpsc;
use std::time::{Duration, Instant};

use anyhow::anyhow;
use serde_json::json;

use crate::config::Config;
use crate::eval_events;
use crate::providers::{AssistantReply, ChatClient};
use crate::state::ConversationMessage;
use crate::tools::registry::ToolSpec;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProviderCallScope {
    PlannerUltra,
    PlannerStep,
    Executor,
    Repair,
}

impl ProviderCallScope {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::PlannerUltra => "planner_ultra",
            Self::PlannerStep => "planner_step",
            Self::Executor => "executor",
            Self::Repair => "repair",
        }
    }

    pub fn timeout_kind(self) -> &'static str {
        match self {
            Self::PlannerUltra => "planner_ultra_timeout",
            Self::PlannerStep => "phase_step_planner_timeout",
            Self::Executor | Self::Repair => "provider_turn_timeout",
        }
    }

    fn is_planner(self) -> bool {
        matches!(self, Self::PlannerUltra | Self::PlannerStep)
    }
}

pub struct ProviderCallOutcome {
    pub result: anyhow::Result<AssistantReply>,
    pub elapsed: Duration,
    pub timed_out: bool,
}

struct ProviderTurnTelemetry<'a> {
    scope: ProviderCallScope,
    provider: &'a str,
    model: &'a str,
    tool_count: usize,
    native_tools_enabled: bool,
    elapsed: Duration,
    timed_out: bool,
    ok: bool,
}

pub fn chat(
    client: &mut dyn ChatClient,
    config: &Config,
    scope: ProviderCallScope,
    model: &str,
    messages: &[ConversationMessage],
    tools: &[ToolSpec],
    native_tools_enabled: bool,
) -> ProviderCallOutcome {
    let provider = client.label().to_string();
    let timeout = Duration::from_secs(config.chat_timeout_secs);
    let started = Instant::now();

    let Some(mut worker_client) = client.boxed_clone() else {
        let result = client.chat(model, messages, tools, native_tools_enabled);
        let elapsed = started.elapsed();
        emit_provider_turn_duration(
            config,
            ProviderTurnTelemetry {
                scope,
                provider: &provider,
                model,
                tool_count: tools.len(),
                native_tools_enabled,
                elapsed,
                timed_out: elapsed >= timeout,
                ok: result.is_ok(),
            },
        );
        return ProviderCallOutcome {
            result,
            elapsed,
            timed_out: elapsed >= timeout,
        };
    };

    let model = model.to_string();
    let tool_count = tools.len();
    let messages = messages.to_vec();
    let tools = tools.to_vec();
    let worker_model = model.clone();
    let (tx, rx) = mpsc::sync_channel(1);
    std::thread::spawn(move || {
        let result = worker_client.chat(&worker_model, &messages, &tools, native_tools_enabled);
        let _ = tx.send(result);
    });

    match rx.recv_timeout(timeout) {
        Ok(result) => {
            let elapsed = started.elapsed();
            emit_provider_turn_duration(
                config,
                ProviderTurnTelemetry {
                    scope,
                    provider: &provider,
                    model: &model,
                    tool_count,
                    native_tools_enabled,
                    elapsed,
                    timed_out: false,
                    ok: result.is_ok(),
                },
            );
            ProviderCallOutcome {
                result,
                elapsed,
                timed_out: false,
            }
        }
        Err(mpsc::RecvTimeoutError::Timeout) => {
            let elapsed = started.elapsed();
            emit_provider_turn_duration(
                config,
                ProviderTurnTelemetry {
                    scope,
                    provider: &provider,
                    model: &model,
                    tool_count,
                    native_tools_enabled,
                    elapsed,
                    timed_out: true,
                    ok: false,
                },
            );
            if scope.is_planner() {
                emit_provider_turn_timeout(config, scope, &provider, &model, elapsed);
            }
            ProviderCallOutcome {
                result: Err(anyhow!(
                    "{}: provider call exceeded configured deadline of {}s",
                    scope.timeout_kind(),
                    config.chat_timeout_secs
                )),
                elapsed,
                timed_out: true,
            }
        }
        Err(mpsc::RecvTimeoutError::Disconnected) => {
            let elapsed = started.elapsed();
            emit_provider_turn_duration(
                config,
                ProviderTurnTelemetry {
                    scope,
                    provider: &provider,
                    model: &model,
                    tool_count,
                    native_tools_enabled,
                    elapsed,
                    timed_out: false,
                    ok: false,
                },
            );
            ProviderCallOutcome {
                result: Err(anyhow!("provider call worker disconnected")),
                elapsed,
                timed_out: false,
            }
        }
    }
}

pub fn is_scoped_timeout(scope: ProviderCallScope, error: &str) -> bool {
    error.contains(scope.timeout_kind())
}

fn emit_provider_turn_duration(config: &Config, telemetry: ProviderTurnTelemetry<'_>) {
    eval_events::emit(
        config.eval_events_path.as_deref(),
        json!({
            "event": "provider_turn_duration",
            "caller_scope": telemetry.scope.as_str(),
            "provider": telemetry.provider,
            "model": eval_events::body_snippet(telemetry.model),
            "duration_ms": telemetry.elapsed.as_millis().min(u128::from(u64::MAX)) as u64,
            "timeout_ms": Duration::from_secs(config.chat_timeout_secs).as_millis().min(u128::from(u64::MAX)) as u64,
            "timeout_secs": config.chat_timeout_secs,
            "timeout_source": config.chat_timeout_source,
            "timed_out": telemetry.timed_out,
            "ok": telemetry.ok,
            "tools": telemetry.tool_count,
            "native_tools_enabled": telemetry.native_tools_enabled,
        }),
    );
}

fn emit_provider_turn_timeout(
    config: &Config,
    scope: ProviderCallScope,
    provider: &str,
    model: &str,
    elapsed: Duration,
) {
    eval_events::emit(
        config.eval_events_path.as_deref(),
        json!({
            "event": "provider_turn_timeout",
            "caller_scope": scope.as_str(),
            "provider": provider,
            "model": eval_events::body_snippet(model),
            "classification": scope.timeout_kind(),
            "duration_ms": elapsed.as_millis().min(u128::from(u64::MAX)) as u64,
            "timeout_secs": config.chat_timeout_secs,
            "timeout_source": config.chat_timeout_source,
        }),
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{Action, Provider};
    use std::path::PathBuf;

    #[derive(Clone)]
    struct HangingClient {
        delay: Duration,
    }

    impl ChatClient for HangingClient {
        fn label(&self) -> &str {
            "hanging"
        }

        fn boxed_clone(&self) -> Option<Box<dyn ChatClient>> {
            Some(Box::new(self.clone()))
        }

        fn chat(
            &mut self,
            _model: &str,
            _messages: &[ConversationMessage],
            _tools: &[ToolSpec],
            _native_tools_enabled: bool,
        ) -> anyhow::Result<AssistantReply> {
            std::thread::sleep(self.delay);
            Ok(AssistantReply::text("late"))
        }
    }

    #[test]
    fn planner_scope_timeout_kinds_are_stable() {
        assert_eq!(
            ProviderCallScope::PlannerStep.timeout_kind(),
            "phase_step_planner_timeout"
        );
        assert_eq!(
            ProviderCallScope::PlannerUltra.timeout_kind(),
            "planner_ultra_timeout"
        );
    }

    #[test]
    fn cloned_provider_call_times_out_without_waiting_for_worker() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let events_path = tmp.path().join("events.jsonl");
        let config = Config {
            workspace_root: tmp.path().to_path_buf(),
            state_dir: PathBuf::from("state"),
            yes: true,
            offline: false,
            context_budget: 1000,
            model: "m".to_string(),
            provider: Provider::Ollama,
            planner_model: "m".to_string(),
            planner_provider: Provider::Ollama,
            ollama_host: "http://localhost:11434".to_string(),
            num_predict: 100,
            max_iterations: 1,
            chat_timeout_secs: 1,
            chat_timeout_source: "override:test".to_string(),
            chat_retries: 1,
            eval_events_path: Some(events_path.clone()),
            completion_contract_path: None,
            resume: None,
            fresh_session: false,
            no_footer: false,
            profile: "generic".to_string(),
            profile_explicit: false,
            profile_inference: None,
            style: "default".to_string(),
            action: Action::Repl,
        };
        let mut client = HangingClient {
            delay: Duration::from_secs(5),
        };

        let started = Instant::now();
        let outcome = chat(
            &mut client,
            &config,
            ProviderCallScope::PlannerStep,
            "m",
            &[ConversationMessage::user("plan".to_string())],
            &[],
            false,
        );

        assert!(outcome.timed_out);
        assert!(started.elapsed() < Duration::from_secs(3));
        let error = outcome.result.unwrap_err().to_string();
        assert!(
            error.contains("phase_step_planner_timeout"),
            "unexpected error: {error}"
        );
        let events = std::fs::read_to_string(events_path).expect("events");
        assert!(events.contains("\"event\":\"provider_turn_duration\""));
        assert!(events.contains("\"caller_scope\":\"planner_step\""));
        assert!(events.contains("\"timeout_source\":\"override:test\""));
        assert!(events.contains("\"classification\":\"phase_step_planner_timeout\""));
    }
}
