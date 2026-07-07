use std::sync::{Arc, Condvar, Mutex, OnceLock};
use std::time::{Duration, Instant};

use serde_json::Value;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct StatusTime {
    millis: u64,
}

impl StatusTime {
    pub const ZERO: Self = Self { millis: 0 };

    pub fn from_millis(millis: u64) -> Self {
        Self { millis }
    }

    pub fn from_secs(secs: u64) -> Self {
        Self {
            millis: secs.saturating_mul(1_000),
        }
    }

    pub fn elapsed_secs_since(self, start: Self) -> u64 {
        self.millis.saturating_sub(start.millis) / 1_000
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PhaseStatus {
    pub index: usize,
    pub total: usize,
    pub id: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StepStatus {
    pub id: String,
    pub kind: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProviderTurnStatus {
    pub scope: String,
    pub deadline_secs: u64,
    pub started_at: StatusTime,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommandStatus {
    pub excerpt: String,
    pub cap_secs: u64,
    pub started_at: StatusTime,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RepairStatus {
    pub attempt: usize,
    pub max: usize,
    pub kind: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RuntimeStatus {
    pub generation: u64,
    pub phase: Option<PhaseStatus>,
    pub step: Option<StepStatus>,
    pub provider: Option<ProviderTurnStatus>,
    pub command: Option<CommandStatus>,
    pub repair: Option<RepairStatus>,
    pub stage: Option<String>,
    pub last_event: Option<String>,
    pub interrupt_requested: bool,
    pub force_finalize_requested: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StatusEvent {
    PhaseStart {
        index: usize,
        total: usize,
        id: String,
    },
    PhaseEnd {
        index: usize,
        total: usize,
        id: String,
    },
    StepStart {
        id: String,
        kind: String,
    },
    ProviderStarted {
        scope: String,
        deadline_secs: u64,
        started_at: StatusTime,
    },
    ProviderFinished {
        duration_secs: u64,
    },
    CommandStarted {
        excerpt: String,
        cap_secs: u64,
        started_at: StatusTime,
    },
    CommandFinished,
    Repair {
        attempt: usize,
        max: usize,
        kind: String,
    },
    Stage {
        label: String,
    },
    LastEvent {
        label: String,
    },
    InterruptRequested,
    ForceFinalizeRequested,
}

#[derive(Debug)]
struct BusInner {
    status: Mutex<RuntimeStatus>,
    wake: Condvar,
}

#[derive(Debug, Clone)]
pub struct StatusPublisher {
    inner: Option<Arc<BusInner>>,
}

#[derive(Debug, Clone)]
pub struct StatusSubscriber {
    inner: Arc<BusInner>,
}

impl StatusPublisher {
    pub fn disabled() -> Self {
        Self { inner: None }
    }

    pub fn is_enabled(&self) -> bool {
        self.inner.is_some()
    }

    pub fn publish(&self, event: StatusEvent) -> bool {
        let Some(inner) = &self.inner else {
            return false;
        };
        let mut status = inner
            .status
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        apply_event(&mut status, event);
        status.generation = status.generation.saturating_add(1);
        drop(status);
        inner.wake.notify_all();
        true
    }

    pub fn snapshot(&self) -> Option<RuntimeStatus> {
        let inner = self.inner.as_ref()?;
        Some(
            inner
                .status
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .clone(),
        )
    }
}

impl Default for StatusPublisher {
    fn default() -> Self {
        Self::disabled()
    }
}

impl StatusSubscriber {
    pub fn snapshot(&self) -> RuntimeStatus {
        self.inner
            .status
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone()
    }

    pub fn wait_for_change(&self, previous_generation: u64, timeout: Duration) -> RuntimeStatus {
        let status = self
            .inner
            .status
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if status.generation != previous_generation {
            return status.clone();
        }
        let (status, _) = self
            .inner
            .wake
            .wait_timeout(status, timeout)
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        status.clone()
    }
}

pub fn channel() -> (StatusPublisher, StatusSubscriber) {
    let inner = Arc::new(BusInner {
        status: Mutex::new(RuntimeStatus::default()),
        wake: Condvar::new(),
    });
    (
        StatusPublisher {
            inner: Some(inner.clone()),
        },
        StatusSubscriber { inner },
    )
}

fn apply_event(status: &mut RuntimeStatus, event: StatusEvent) {
    match event {
        StatusEvent::PhaseStart { index, total, id }
        | StatusEvent::PhaseEnd { index, total, id } => {
            status.phase = Some(PhaseStatus { index, total, id });
        }
        StatusEvent::StepStart { id, kind } => {
            status.step = Some(StepStatus { id, kind });
        }
        StatusEvent::ProviderStarted {
            scope,
            deadline_secs,
            started_at,
        } => {
            status.command = None;
            status.provider = Some(ProviderTurnStatus {
                scope,
                deadline_secs,
                started_at,
            });
        }
        StatusEvent::ProviderFinished { duration_secs } => {
            status.provider = None;
            status.last_event = Some(format!("provider_turn_finished:{duration_secs}s"));
        }
        StatusEvent::CommandStarted {
            excerpt,
            cap_secs,
            started_at,
        } => {
            status.provider = None;
            status.command = Some(CommandStatus {
                excerpt,
                cap_secs,
                started_at,
            });
        }
        StatusEvent::CommandFinished => {
            status.command = None;
        }
        StatusEvent::Repair { attempt, max, kind } => {
            status.repair = Some(RepairStatus { attempt, max, kind });
        }
        StatusEvent::Stage { label } => {
            status.stage = Some(label);
        }
        StatusEvent::LastEvent { label } => {
            status.last_event = Some(label);
        }
        StatusEvent::InterruptRequested => {
            status.interrupt_requested = true;
        }
        StatusEvent::ForceFinalizeRequested => {
            status.interrupt_requested = true;
            status.force_finalize_requested = true;
        }
    }
}

static GLOBAL_STATUS_BUS: OnceLock<Mutex<StatusPublisher>> = OnceLock::new();
static STARTED_AT: OnceLock<Instant> = OnceLock::new();

fn global_publisher() -> &'static Mutex<StatusPublisher> {
    GLOBAL_STATUS_BUS.get_or_init(|| Mutex::new(StatusPublisher::disabled()))
}

pub struct GlobalStatusBusGuard {
    previous: StatusPublisher,
}

impl Drop for GlobalStatusBusGuard {
    fn drop(&mut self) {
        let mut current = global_publisher()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        *current = self.previous.clone();
    }
}

pub fn install_global(publisher: StatusPublisher) -> GlobalStatusBusGuard {
    let mut current = global_publisher()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let previous = current.clone();
    *current = publisher;
    GlobalStatusBusGuard { previous }
}

pub fn publish_global(event: StatusEvent) -> bool {
    let publisher = global_publisher()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .clone();
    publisher.publish(event)
}

pub fn now() -> StatusTime {
    let started = STARTED_AT.get_or_init(Instant::now);
    let millis = started.elapsed().as_millis().min(u128::from(u64::MAX)) as u64;
    StatusTime::from_millis(millis)
}

pub fn publish_provider_started(scope: &str, deadline_secs: u64) -> bool {
    publish_global(StatusEvent::ProviderStarted {
        scope: scope.to_string(),
        deadline_secs,
        started_at: now(),
    })
}

pub fn provider_scope_label(scope: &str) -> &'static str {
    match scope {
        "planner_ultra" => "planning the overall plan",
        "planner_step" => "planning steps",
        "executor" => "implementing",
        "repair" => "repairing",
        _ => "model turn",
    }
}

pub fn publish_provider_finished(duration: Duration) -> bool {
    publish_global(StatusEvent::ProviderFinished {
        duration_secs: duration.as_secs(),
    })
}

pub fn publish_command_started(command: &std::process::Command, cap: Duration) -> bool {
    publish_global(StatusEvent::CommandStarted {
        excerpt: sanitize_command_excerpt(&format!("{command:?}")),
        cap_secs: cap.as_secs(),
        started_at: now(),
    })
}

pub fn publish_command_finished() -> bool {
    publish_global(StatusEvent::CommandFinished)
}

pub fn publish_interrupt_requested() -> bool {
    publish_global(StatusEvent::InterruptRequested)
}

pub fn publish_force_finalize_requested() -> bool {
    publish_global(StatusEvent::ForceFinalizeRequested)
}

pub fn publish_eval_projection(event: &Value) -> bool {
    let Some(name) = event.get("event").and_then(Value::as_str) else {
        return false;
    };
    match name {
        "ultra_phase_start" => publish_phase_event(event, false),
        "ultra_phase_complete"
        | "ultra_phase_failed"
        | "ultra_phase_execute_complete"
        | "ultra_phase_scaffold_complete"
        | "ultra_phase_plan_validated" => publish_phase_event(event, true),
        "step_prompt_contract" => {
            let id = event_text(event, "step_id").unwrap_or_default();
            let kind = event_text(event, "step_kind").unwrap_or_default();
            if id.is_empty() && kind.is_empty() {
                false
            } else {
                publish_global(StatusEvent::StepStart { id, kind })
            }
        }
        "final_acceptance_repair_start" => publish_repair_event(event, "final_acceptance"),
        "step_verify_repair" | "verify_repair_turn" | "verify_repair_progress" => {
            publish_repair_event(event, "step")
        }
        "browser_probe" => publish_stage("browser readiness probe"),
        "browser_interaction_probe" | "profile_behavior_probe" => {
            publish_stage("interaction probe")
        }
        "tool_args_path_normalized"
        | "tool_args_path_salvaged"
        | "verify_command_substituted"
        | "verify_command_normalized_at_runtime"
        | "planner_verify_command_normalized"
        | "ultra_plan_generation_metadata_normalized" => publish_global(StatusEvent::LastEvent {
            label: name.to_string(),
        }),
        _ => false,
    }
}

fn publish_phase_event(event: &Value, completed: bool) -> bool {
    let index = event
        .get("phase_index")
        .and_then(Value::as_u64)
        .unwrap_or(0) as usize;
    let total = event
        .get("total_phases")
        .and_then(Value::as_u64)
        .unwrap_or(0) as usize;
    let id = event_text(event, "phase_id").unwrap_or_default();
    if index == 0 && total == 0 && id.is_empty() {
        return false;
    }
    if completed {
        publish_global(StatusEvent::PhaseEnd { index, total, id })
    } else {
        publish_global(StatusEvent::PhaseStart { index, total, id })
    }
}

fn publish_repair_event(event: &Value, default_kind: &str) -> bool {
    let attempt = event
        .get("attempt")
        .or_else(|| event.get("cycle_index"))
        .and_then(Value::as_u64)
        .unwrap_or(1) as usize;
    let max = event
        .get("attempt_limit")
        .or_else(|| event.get("repair_cap"))
        .and_then(Value::as_u64)
        .unwrap_or(2) as usize;
    let kind = event_text(event, "repair_target").unwrap_or_else(|| default_kind.to_string());
    publish_global(StatusEvent::Repair { attempt, max, kind })
}

fn publish_stage(label: &str) -> bool {
    publish_global(StatusEvent::Stage {
        label: label.to_string(),
    })
}

fn event_text(event: &Value, key: &str) -> Option<String> {
    event
        .get(key)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
}

fn sanitize_command_excerpt(value: &str) -> String {
    let clean = value.replace(['\n', '\r'], " ");
    crate::util::excerpt_with_marker(&clean, 120, "...")
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn disabled_publisher_is_noop() {
        let publisher = StatusPublisher::disabled();

        assert!(!publisher.publish(StatusEvent::LastEvent {
            label: "tool_args_path_normalized".to_string(),
        }));
        assert!(publisher.snapshot().is_none());
    }

    #[test]
    fn bus_projects_phase_step_and_provider_turns() {
        let (publisher, subscriber) = channel();

        assert!(publisher.publish(StatusEvent::PhaseStart {
            index: 2,
            total: 5,
            id: "core-logic".to_string(),
        }));
        assert!(publisher.publish(StatusEvent::StepStart {
            id: "write-game".to_string(),
            kind: "implement".to_string(),
        }));
        assert!(publisher.publish(StatusEvent::ProviderStarted {
            scope: "planner_step".to_string(),
            deadline_secs: 600,
            started_at: StatusTime::from_secs(10),
        }));

        let snapshot = subscriber.snapshot();
        assert_eq!(snapshot.phase.as_ref().unwrap().id, "core-logic");
        assert_eq!(snapshot.step.as_ref().unwrap().kind, "implement");
        assert_eq!(snapshot.provider.as_ref().unwrap().scope, "planner_step");
        assert!(snapshot.generation >= 3);
    }

    #[test]
    fn bus_projects_force_finalize_interrupt() {
        let (publisher, subscriber) = channel();

        assert!(publisher.publish(StatusEvent::ForceFinalizeRequested));
        let snapshot = subscriber.snapshot();

        assert!(snapshot.interrupt_requested);
        assert!(snapshot.force_finalize_requested);
    }

    #[test]
    fn eval_event_projection_tracks_notable_normalization() {
        let (publisher, subscriber) = channel();
        let _guard = install_global(publisher);

        assert!(publish_eval_projection(&json!({
            "event": "tool_args_path_normalized",
            "tool": "Write",
        })));

        assert_eq!(
            subscriber.snapshot().last_event.as_deref(),
            Some("tool_args_path_normalized")
        );
    }
}
