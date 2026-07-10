use std::path::Path;

use serde_json::{Value, json};

use crate::eval_events;
use crate::minimal_loop::interaction_probe::{
    INTERACTION_PROBE_SETUP_REMEDIATION, ProbeAvailability, playwright_availability,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InteractionProbePreflight {
    pub status: String,
    pub ok: bool,
    pub reason: String,
    pub remediation: String,
    pub message: String,
    pub playwright_resolution_location: String,
    pub playwright_module_path: String,
    pub playwright_version: String,
}

impl InteractionProbePreflight {
    fn passed(
        location: impl Into<String>,
        module_path: impl Into<String>,
        version: impl Into<String>,
    ) -> Self {
        Self {
            status: "passed".to_string(),
            ok: true,
            reason: String::new(),
            remediation: String::new(),
            message: "interaction probe preflight passed".to_string(),
            playwright_resolution_location: location.into(),
            playwright_module_path: module_path.into(),
            playwright_version: version.into(),
        }
    }

    fn failed(reason: impl Into<String>) -> Self {
        let reason = reason.into();
        let remediation = INTERACTION_PROBE_SETUP_REMEDIATION.to_string();
        Self {
            status: "failed".to_string(),
            ok: false,
            reason: reason.clone(),
            remediation: remediation.clone(),
            message: format!("interaction probe preflight failed: {reason}; {remediation}"),
            playwright_resolution_location: String::new(),
            playwright_module_path: String::new(),
            playwright_version: String::new(),
        }
    }

    pub fn from_availability(availability: ProbeAvailability) -> Self {
        match availability {
            ProbeAvailability::Available(resolution) => Self::passed(
                resolution.location,
                resolution.module_path,
                resolution.version,
            ),
            ProbeAvailability::Unavailable(reason) => Self::failed(reason),
        }
    }

    pub fn event(&self, context: &str) -> Value {
        json!({
            "event": "probe_preflight",
            "probe": "interaction",
            "context": context,
            "status": self.status,
            "ok": self.ok,
            "reason": self.reason,
            "remediation": self.remediation,
            "message": self.message,
            "playwright_resolution_location": self.playwright_resolution_location,
            "playwright_module_path": self.playwright_module_path,
            "playwright_version": self.playwright_version,
        })
    }
}

pub fn check_interaction_probe_readiness(root: &Path) -> InteractionProbePreflight {
    InteractionProbePreflight::from_availability(playwright_availability(root))
}

pub fn emit_interaction_probe_preflight(
    eval_events_path: Option<&Path>,
    root: &Path,
    context: &str,
) -> InteractionProbePreflight {
    let preflight = check_interaction_probe_readiness(root);
    eval_events::emit(eval_events_path, preflight.event(context));
    preflight
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct LatestProbePreflight {
    pub status: String,
    pub reason: String,
    pub remediation: String,
    pub message: String,
}

impl LatestProbePreflight {
    pub fn failed(&self) -> bool {
        self.status == "failed"
    }
}

pub fn latest_interaction_probe_preflight(events: &[Value]) -> LatestProbePreflight {
    events
        .iter()
        .rev()
        .find(|event| {
            event.get("event").and_then(Value::as_str) == Some("probe_preflight")
                && event.get("probe").and_then(Value::as_str) == Some("interaction")
        })
        .map(|event| LatestProbePreflight {
            status: string_field(event, "status"),
            reason: string_field(event, "reason"),
            remediation: string_field(event, "remediation"),
            message: string_field(event, "message"),
        })
        .unwrap_or_default()
}

fn string_field(event: &Value, key: &str) -> String {
    event
        .get(key)
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::minimal_loop::interaction_probe::PlaywrightResolution;

    fn event_text(path: &Path) -> String {
        std::fs::read_to_string(path).unwrap()
    }

    #[test]
    fn preflight_success_emits_passed_event() {
        let dir = tempfile::tempdir().unwrap();
        let events = dir.path().join("events.jsonl");
        crate::minimal_loop::interaction_probe::write_test_availability_override_with_resolution(
            dir.path(),
            true,
            Some(&PlaywrightResolution {
                module_path: "managed/playwright/index.js".to_string(),
                module_dir: "managed/playwright".to_string(),
                node_path: Some("managed/node_modules".to_string()),
                location: "managed_interaction_probe".to_string(),
                version: "1.2.3-test".to_string(),
            }),
        );

        let report =
            emit_interaction_probe_preflight(Some(&events), dir.path(), "ultra_final_acceptance");

        assert!(report.ok);
        assert_eq!(report.status, "passed");
        assert_eq!(
            report.playwright_resolution_location,
            "managed_interaction_probe"
        );
        let text = event_text(&events);
        assert!(text.contains(r#""event":"probe_preflight""#), "{text}");
        assert!(text.contains(r#""status":"passed""#), "{text}");
        assert!(
            text.contains(r#""playwright_version":"1.2.3-test""#),
            "{text}"
        );
    }

    #[test]
    fn preflight_failure_emits_failed_event_with_setup_guidance() {
        let dir = tempfile::tempdir().unwrap();
        let events = dir.path().join("events.jsonl");
        crate::minimal_loop::interaction_probe::write_test_availability_override(dir.path(), false);

        let report =
            emit_interaction_probe_preflight(Some(&events), dir.path(), "ultra_final_acceptance");

        assert!(!report.ok);
        assert_eq!(report.status, "failed");
        assert_eq!(report.reason, "playwright_not_installed");
        assert!(report.message.contains("--setup-interaction-probe"));
        let text = event_text(&events);
        assert!(text.contains(r#""event":"probe_preflight""#), "{text}");
        assert!(text.contains(r#""status":"failed""#), "{text}");
        assert!(
            text.contains("anvilminimal --setup-interaction-probe"),
            "{text}"
        );
    }

    #[test]
    fn latest_preflight_uses_last_interaction_event() {
        let events = vec![
            json!({
                "event": "probe_preflight",
                "probe": "interaction",
                "status": "failed",
                "reason": "playwright_not_installed",
                "remediation": INTERACTION_PROBE_SETUP_REMEDIATION,
                "message": "old",
            }),
            json!({
                "event": "probe_preflight",
                "probe": "interaction",
                "status": "passed",
                "reason": "",
                "remediation": "",
                "message": "new",
            }),
        ];

        let latest = latest_interaction_probe_preflight(&events);

        assert_eq!(latest.status, "passed");
        assert!(!latest.failed());
        assert_eq!(latest.message, "new");
    }
}
