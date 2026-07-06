use std::sync::mpsc;
use std::time::{Duration, Instant};

use anyhow::anyhow;
use serde_json::json;

use crate::config::Config;
use crate::eval_events;
use crate::providers::{AssistantReply, ChatClient};
use crate::state::ConversationMessage;
use crate::tools::registry::ToolSpec;

pub const PROVIDER_WAIT_SLICE: Duration = Duration::from_millis(250);
const ABORTED_BY_USER_ERROR: &str = "aborted_by_user: interrupted by user";

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
    pub aborted_by_user: bool,
}

pub struct ProviderChatRequest<'a> {
    pub scope: ProviderCallScope,
    pub model: &'a str,
    pub messages: &'a [ConversationMessage],
    pub tools: &'a [ToolSpec],
    pub native_tools_enabled: bool,
}

struct ProviderTurnTelemetry<'a> {
    scope: ProviderCallScope,
    provider: &'a str,
    model: &'a str,
    tool_count: usize,
    native_tools_enabled: bool,
    elapsed: Duration,
    timed_out: bool,
    aborted_by_user: bool,
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
    chat_with_cancel(
        client,
        config,
        ProviderChatRequest {
            scope,
            model,
            messages,
            tools,
            native_tools_enabled,
        },
        || false,
    )
}

pub fn chat_with_cancel<F>(
    client: &mut dyn ChatClient,
    config: &Config,
    request: ProviderChatRequest<'_>,
    is_cancelled: F,
) -> ProviderCallOutcome
where
    F: Fn() -> bool,
{
    let scope = request.scope;
    let model = request.model;
    let messages = request.messages;
    let tools = request.tools;
    let native_tools_enabled = request.native_tools_enabled;
    let provider = client.label().to_string();
    let timeout = Duration::from_secs(config.chat_timeout_secs);
    crate::tui::status_bus::publish_provider_started(scope.as_str(), config.chat_timeout_secs);
    let started = Instant::now();

    if is_cancelled() {
        return provider_aborted_by_user(
            config,
            scope,
            &provider,
            model,
            tools.len(),
            native_tools_enabled,
            started,
        );
    }

    let Some(mut worker_client) = client.boxed_clone() else {
        let result = client.chat(model, messages, tools, native_tools_enabled);
        let elapsed = started.elapsed();
        crate::tui::status_bus::publish_provider_finished(elapsed);
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
                aborted_by_user: false,
                ok: result.is_ok(),
            },
        );
        return ProviderCallOutcome {
            result,
            elapsed,
            timed_out: elapsed >= timeout,
            aborted_by_user: false,
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

    loop {
        if is_cancelled() {
            return provider_aborted_by_user(
                config,
                scope,
                &provider,
                &model,
                tool_count,
                native_tools_enabled,
                started,
            );
        }
        let elapsed = started.elapsed();
        if elapsed >= timeout {
            let elapsed = started.elapsed();
            crate::tui::status_bus::publish_provider_finished(elapsed);
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
                    aborted_by_user: false,
                    ok: false,
                },
            );
            if scope.is_planner() {
                emit_provider_turn_timeout(config, scope, &provider, &model, elapsed);
            }
            return ProviderCallOutcome {
                result: Err(anyhow!(
                    "{}: provider call exceeded configured deadline of {}s",
                    scope.timeout_kind(),
                    config.chat_timeout_secs
                )),
                elapsed,
                timed_out: true,
                aborted_by_user: false,
            };
        }
        let slice = PROVIDER_WAIT_SLICE.min(timeout.saturating_sub(elapsed));
        match rx.recv_timeout(slice) {
            Ok(result) => {
                let elapsed = started.elapsed();
                crate::tui::status_bus::publish_provider_finished(elapsed);
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
                        aborted_by_user: false,
                        ok: result.is_ok(),
                    },
                );
                return ProviderCallOutcome {
                    result,
                    elapsed,
                    timed_out: false,
                    aborted_by_user: false,
                };
            }
            Err(mpsc::RecvTimeoutError::Timeout) => continue,
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                let elapsed = started.elapsed();
                crate::tui::status_bus::publish_provider_finished(elapsed);
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
                        aborted_by_user: false,
                        ok: false,
                    },
                );
                return ProviderCallOutcome {
                    result: Err(anyhow!("provider call worker disconnected")),
                    elapsed,
                    timed_out: false,
                    aborted_by_user: false,
                };
            }
        }
    }
}

pub fn is_aborted_by_user(error: &str) -> bool {
    error.contains("aborted_by_user") || error.contains("interrupted by user")
}

pub fn is_scoped_timeout(scope: ProviderCallScope, error: &str) -> bool {
    error.contains(scope.timeout_kind())
}

fn provider_aborted_by_user(
    config: &Config,
    scope: ProviderCallScope,
    provider: &str,
    model: &str,
    tool_count: usize,
    native_tools_enabled: bool,
    started: Instant,
) -> ProviderCallOutcome {
    let elapsed = started.elapsed();
    crate::tui::status_bus::publish_provider_finished(elapsed);
    emit_provider_turn_duration(
        config,
        ProviderTurnTelemetry {
            scope,
            provider,
            model,
            tool_count,
            native_tools_enabled,
            elapsed,
            timed_out: false,
            aborted_by_user: true,
            ok: false,
        },
    );
    emit_provider_turn_aborted_by_user(config, scope, provider, model, elapsed);
    ProviderCallOutcome {
        result: Err(anyhow!(ABORTED_BY_USER_ERROR)),
        elapsed,
        timed_out: false,
        aborted_by_user: true,
    }
}

fn emit_provider_turn_duration(config: &Config, telemetry: ProviderTurnTelemetry<'_>) {
    let mut value = json!({
        "event": "provider_turn_duration",
        "caller_scope": telemetry.scope.as_str(),
        "provider": telemetry.provider,
        "model": eval_events::body_snippet(telemetry.model),
        "duration_ms": telemetry.elapsed.as_millis().min(u128::from(u64::MAX)) as u64,
        "timeout_ms": Duration::from_secs(config.chat_timeout_secs).as_millis().min(u128::from(u64::MAX)) as u64,
        "timeout_secs": config.chat_timeout_secs,
        "timeout_source": config.chat_timeout_source,
        "timed_out": telemetry.timed_out,
        "aborted_by_user": telemetry.aborted_by_user,
        "ok": telemetry.ok,
        "tools": telemetry.tool_count,
        "native_tools_enabled": telemetry.native_tools_enabled,
    });
    if telemetry.aborted_by_user {
        value["classification"] = json!("aborted_by_user");
    }
    eval_events::emit(config.eval_events_path.as_deref(), value);
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

fn emit_provider_turn_aborted_by_user(
    config: &Config,
    scope: ProviderCallScope,
    provider: &str,
    model: &str,
    elapsed: Duration,
) {
    eval_events::emit(
        config.eval_events_path.as_deref(),
        json!({
            "event": "provider_turn_aborted_by_user",
            "caller_scope": scope.as_str(),
            "provider": provider,
            "model": eval_events::body_snippet(model),
            "classification": "aborted_by_user",
            "duration_ms": elapsed.as_millis().min(u128::from(u64::MAX)) as u64,
        }),
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{Action, Provider};
    use std::path::PathBuf;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, Ordering};

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

    fn test_config(root: &std::path::Path, events_path: PathBuf, chat_timeout_secs: u64) -> Config {
        Config {
            workspace_root: root.to_path_buf(),
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
            chat_timeout_secs,
            chat_timeout_source: "override:test".to_string(),
            field_sources: crate::config::ConfigFieldSources::default(),
            chat_retries: 1,
            eval_events_path: Some(events_path),
            completion_contract_path: None,
            resume: None,
            fresh_session: false,
            no_footer: false,
            narration: crate::config::NarrationMode::Normal,
            profile: "generic".to_string(),
            profile_explicit: false,
            profile_inference: None,
            style: "default".to_string(),
            action: Action::Repl,
        }
    }

    #[test]
    fn cloned_provider_call_times_out_without_waiting_for_worker() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let events_path = tmp.path().join("events.jsonl");
        let config = test_config(tmp.path(), events_path.clone(), 1);
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

    #[test]
    fn cloned_provider_call_aborts_promptly_when_cancelled() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let events_path = tmp.path().join("events.jsonl");
        let config = test_config(tmp.path(), events_path.clone(), 30);
        let mut client = HangingClient {
            delay: Duration::from_secs(30),
        };
        let cancelled = Arc::new(AtomicBool::new(false));
        let thread_cancelled = Arc::clone(&cancelled);
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(100));
            thread_cancelled.store(true, Ordering::SeqCst);
        });

        let started = Instant::now();
        let outcome = chat_with_cancel(
            &mut client,
            &config,
            ProviderChatRequest {
                scope: ProviderCallScope::PlannerStep,
                model: "m",
                messages: &[ConversationMessage::user("plan".to_string())],
                tools: &[],
                native_tools_enabled: false,
            },
            || cancelled.load(Ordering::SeqCst),
        );

        assert!(outcome.aborted_by_user);
        assert!(!outcome.timed_out);
        assert!(started.elapsed() < Duration::from_secs(2));
        let error = outcome.result.unwrap_err().to_string();
        assert!(is_aborted_by_user(&error), "unexpected error: {error}");
        let events = std::fs::read_to_string(events_path).expect("events");
        assert!(events.contains("\"event\":\"provider_turn_duration\""));
        assert!(events.contains("\"aborted_by_user\":true"));
        assert!(events.contains("\"classification\":\"aborted_by_user\""));
        assert!(events.contains("\"event\":\"provider_turn_aborted_by_user\""));
        assert!(!events.contains("\"event\":\"provider_turn_timeout\""));
    }
}
