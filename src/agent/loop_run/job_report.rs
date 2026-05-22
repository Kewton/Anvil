//! Structured per-turn job reports (Issue #666).
//!
//! Agent-layer types + emit chokepoint for the four new
//! `agent.{artifact_completion,verification,repair,memory}.report` events.
//! Persistence wiring lives in `src/session/job_report.rs`; this module
//! does NOT import session-layer DTOs directly (DR3-002 unidirectional
//! dependency).
//!
//! ## Layout summary
//!
//! - [`JobReport`] trait declares the contract every Report kind honours:
//!   stable `EVENT_NAME`, explicit `PAYLOAD_SCHEMA_VERSION` (no default),
//!   deterministic `dedup_key`.
//! - [`SafeStopLinkage`] is the DRY-folded sub-struct that three of the
//!   four Reports embed; `MemoryReport` deliberately omits it.
//! - [`build_envelope`] is the single boilerplate-free envelope builder.
//! - [`enforce_bounds`] is the bound enforcer (truncate / drop / flag).
//! - [`Agent::record_job_report`] is the emit chokepoint (declared in
//!   `loop_run.rs` to keep the implementation next to `Agent::new` for
//!   visibility, see Phase C).

use serde::{Deserialize, Serialize};

/// Trait every kind of per-turn job report implements.
///
/// Per-Report payload version is declared without a default so future
/// kinds cannot silently inherit a stale value (DR1-010).
pub(super) trait JobReport: Sized + serde::Serialize {
    /// `agent.<kind>.report` event name. Used both as the log event name
    /// and as the prefix for the per-turn dedup key namespace.
    const EVENT_NAME: &'static str;

    /// Per-Report payload schema version. Distinct from the envelope-level
    /// `JOB_REPORT_SCHEMA_VERSION` constant in `crate::session::job_report`.
    const PAYLOAD_SCHEMA_VERSION: u32;

    /// Deterministic dedup key. Same input → same output; raw paths /
    /// external strings must be redacted before being incorporated
    /// (DR4-004). Typical shape: `"<kind>::<turn_index>::<path_hash>"`.
    fn dedup_key(&self) -> String;
}

/// Shared SafeStopReport linkage (DR1-004).
///
/// Embedded by `ArtifactCompletionReport`, `VerificationReport`, and
/// `RepairReport`. Memory deliberately omits it because PAM/memory state
/// has no direct relationship with `agent.safe_stop.report`.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub(super) struct SafeStopLinkage {
    /// `None` if no SafeStopReport was emitted this turn. Otherwise the
    /// SafeStop reason label (e.g. `"verifier_weak"`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    /// `true` if `agent.safe_stop.report` was emitted this turn (after
    /// the job Report was emitted).
    #[serde(default)]
    pub report_emitted: bool,
}

/// ArtifactCompletionReport — per-turn snapshot of artifact-completion job state.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(super) struct ArtifactCompletionReport {
    pub turn_index: u64,
    pub job_present: bool,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub attempt_outcomes: Vec<serde_json::Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub role_policy_violation: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub budget_state: Option<serde_json::Value>,
    /// `"not_available"` while #659 projections are absent, else `"available"`.
    #[serde(default = "default_projection_status")]
    pub artifact_projection_status: String,
    #[serde(default)]
    pub safe_stop: SafeStopLinkage,
}

impl JobReport for ArtifactCompletionReport {
    const EVENT_NAME: &'static str = "agent.artifact_completion.report";
    const PAYLOAD_SCHEMA_VERSION: u32 = 1;
    fn dedup_key(&self) -> String {
        // turn_index is sufficient: per-turn fire-once across all 4 Reports
        // is namespaced by EVENT_NAME prefix in record_job_report.
        format!("turn:{}", self.turn_index)
    }
}

/// VerificationReport — per-turn snapshot of verification job state.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(super) struct VerificationReport {
    pub turn_index: u64,
    pub job_present: bool,
    /// `"Runnable" | "Weak" | "Missing" | "Absent"`.
    pub binding_kind: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub command_hash: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_invocation: Option<serde_json::Value>,
    #[serde(default)]
    pub safe_stop: SafeStopLinkage,
}

impl JobReport for VerificationReport {
    const EVENT_NAME: &'static str = "agent.verification.report";
    const PAYLOAD_SCHEMA_VERSION: u32 = 1;
    fn dedup_key(&self) -> String {
        format!("turn:{}", self.turn_index)
    }
}

/// RepairReport — per-turn snapshot of repair-job lifecycle.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(super) struct RepairReport {
    pub turn_index: u64,
    pub job_present: bool,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub outcomes: Vec<serde_json::Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub exhausted_promotion: Option<serde_json::Value>,
    #[serde(default)]
    pub no_progress_detected: bool,
    #[serde(default)]
    pub safe_stop: SafeStopLinkage,
}

impl JobReport for RepairReport {
    const EVENT_NAME: &'static str = "agent.repair.report";
    const PAYLOAD_SCHEMA_VERSION: u32 = 1;
    fn dedup_key(&self) -> String {
        format!("turn:{}", self.turn_index)
    }
}

/// MemoryReport — per-turn snapshot of PAM / context-pack memory state.
///
/// Does NOT carry `SafeStopLinkage` — memory state has no direct safe-stop
/// linkage by design (DR1-004 explicit asymmetry).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(super) struct MemoryReport {
    pub turn_index: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pam_decision: Option<serde_json::Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context_pack_binding: Option<serde_json::Value>,
    #[serde(default)]
    pub adopted_item_count: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub injection_skipped_reason: Option<String>,
}

impl JobReport for MemoryReport {
    const EVENT_NAME: &'static str = "agent.memory.report";
    const PAYLOAD_SCHEMA_VERSION: u32 = 1;
    fn dedup_key(&self) -> String {
        format!("turn:{}", self.turn_index)
    }
}

fn default_projection_status() -> String {
    "not_available".to_string()
}

// ---------------------------------------------------------------------------
// Envelope builder + bound enforcement (Phase C)
// ---------------------------------------------------------------------------

/// 8 KiB envelope cap and 1 KiB per-field cap.
pub(super) const MAX_REPORT_PAYLOAD_BYTES: usize = 8 * 1024;
pub(super) const MAX_FIELD_BYTES: usize = 1024;

/// Build the envelope skeleton:
/// `{ schema_version, kind, dedup_key, payload }`.
///
/// Serialize failure is fail-closed (payload = null) but observable
/// (`tracing::error`). Section 4-1 / Section 11-1 of design.
pub(super) fn build_envelope<R: JobReport>(report: &R) -> serde_json::Value {
    let payload = match serde_json::to_value(report) {
        Ok(v) => v,
        Err(e) => {
            tracing::error!(
                error = %e,
                event = R::EVENT_NAME,
                "job report payload serialize failed; emitting null payload (fail-closed)"
            );
            serde_json::Value::Null
        }
    };
    serde_json::json!({
        "schema_version": R::PAYLOAD_SCHEMA_VERSION,
        "kind": R::EVENT_NAME,
        "dedup_key": report.dedup_key(),
        "payload": payload,
    })
}

/// Apply per-field and per-envelope size caps. Returns `(overflowed,
/// truncated)`. Mutates `envelope` in place: fields longer than
/// `MAX_FIELD_BYTES` are truncated (suffixed `"...<truncated>"`); if the
/// envelope still exceeds `MAX_REPORT_PAYLOAD_BYTES` the `payload`
/// field is replaced with `"<dropped:overflow>"` and `overflowed=true` is
/// returned.
pub(super) fn enforce_bounds(envelope: &mut serde_json::Value) -> (bool, bool) {
    let mut truncated = false;
    truncate_strings_in_place(envelope, &mut truncated);
    let mut overflowed = false;
    let serialized_len = serde_json::to_string(envelope)
        .map(|s| s.len())
        .unwrap_or(0);
    if serialized_len > MAX_REPORT_PAYLOAD_BYTES
        && let Some(map) = envelope.as_object_mut()
    {
        map.insert(
            "payload".to_string(),
            serde_json::Value::String("<dropped:overflow>".to_string()),
        );
        overflowed = true;
    }
    (overflowed, truncated)
}

// CB-005 fix: marker length is part of the cap budget. Truncating to
// `MAX_FIELD_BYTES - MARKER.len()` first keeps the final string length
// ≤ MAX_FIELD_BYTES, honoring the design's 1 KiB-per-field invariant.
const TRUNCATE_MARKER: &str = "...<truncated>";

fn truncate_strings_in_place(v: &mut serde_json::Value, truncated: &mut bool) {
    match v {
        serde_json::Value::String(s) if s.len() > MAX_FIELD_BYTES => {
            // Reserve room for the marker INSIDE the cap so the final
            // string is ≤ MAX_FIELD_BYTES (design Section 4-3 / 11-1).
            let payload_cap = MAX_FIELD_BYTES.saturating_sub(TRUNCATE_MARKER.len());
            let mut end = payload_cap;
            while !s.is_char_boundary(end) && end > 0 {
                end -= 1;
            }
            s.truncate(end);
            s.push_str(TRUNCATE_MARKER);
            *truncated = true;
        }
        serde_json::Value::Array(arr) => {
            for item in arr.iter_mut() {
                truncate_strings_in_place(item, truncated);
            }
        }
        serde_json::Value::Object(map) => {
            for (_k, val) in map.iter_mut() {
                truncate_strings_in_place(val, truncated);
            }
        }
        _ => {}
    }
}

// ---------------------------------------------------------------------------
// Emit chokepoint + persistence helpers (Phase C/D)
// ---------------------------------------------------------------------------

use super::PhotonContextPackStatus;
use crate::logging::log_llm_event;
use crate::session::job_report::{JOB_REPORT_SCHEMA_VERSION, StructuredJobReportRecord};
use std::time::{SystemTime, UNIX_EPOCH};

impl super::Agent {
    /// Emit + persist the four per-turn job reports for the current
    /// `handle_user_message` invocation (Phase E.1 of work-plan).
    ///
    /// Called once at end of turn (return path) and ALSO from
    /// `record_safe_stop_report` BEFORE the safe_stop emit so the
    /// llm-io order is `4 Report → safe_stop.report` (CB-001 fix,
    /// design Section 8-2). Per-turn dedup makes the second call a
    /// no-op for kinds already emitted at the safe_stop entry.
    pub(super) fn maybe_emit_job_reports(&mut self) {
        self.maybe_emit_job_reports_with_linkage(None);
    }

    /// Variant invoked from `record_safe_stop_report` with the
    /// in-flight stop reason so the SafeStopLinkage carried by the 3
    /// linked reports reflects the reason the safe_stop will emit
    /// with, even though `safe_stop_report_emitted` is not yet
    /// populated (`linkage_reason_override = Some(stop_reason_label)`).
    /// When called from the normal end-of-turn finalizer
    /// (`linkage_reason_override = None`), the linkage is taken from
    /// the dedup set populated during the turn.
    ///
    /// Build is conditional on observable job state (CB-004 fix): if
    /// no per-Report state was touched this turn the Report is
    /// skipped entirely rather than emitting a low-information
    /// `job_present: false` payload.
    pub(super) fn maybe_emit_job_reports_with_linkage(
        &mut self,
        linkage_reason_override: Option<String>,
    ) {
        let turn_index = self.current_turn_index as u64;
        let safe_stop = match linkage_reason_override {
            Some(reason) => SafeStopLinkage {
                reason: Some(reason),
                report_emitted: true,
            },
            None => self.snapshot_safe_stop_linkage(),
        };

        // ArtifactCompletionReport — only when job state observable
        // this turn or a SafeStop linkage exists. Idle turns produce
        // no report (design Section 8-1 — empty reports are skipped).
        let acr_observable = self.artifact_completion_job.is_some()
            || self.artifact_completion_exhausted_this_turn
            || self.artifact_completion_failed_diagnostic_emitted_this_turn
            || safe_stop.report_emitted;
        if acr_observable {
            let acr = ArtifactCompletionReport {
                turn_index,
                job_present: self.artifact_completion_job.is_some(),
                attempt_outcomes: Vec::new(),
                role_policy_violation: None,
                budget_state: None,
                artifact_projection_status: default_projection_status(),
                safe_stop: safe_stop.clone(),
            };
            if let Some(env) = self.record_job_report(acr) {
                self.persist_job_report_to_session(ArtifactCompletionReport::EVENT_NAME, &env);
            }
        }

        // VerificationReport — verifier-related state present?
        let vr_observable = self.missing_verifier_job.is_some()
            || self.task_contract_verifier_repair_pending
            || self.task_contract_verifier_passed_this_actor_loop
            || safe_stop.report_emitted;
        if vr_observable {
            let vr = VerificationReport {
                turn_index,
                job_present: self.missing_verifier_job.is_some()
                    || self.task_contract_verifier_repair_pending,
                binding_kind: "Absent".to_string(),
                command_hash: None,
                last_invocation: None,
                safe_stop: safe_stop.clone(),
            };
            if let Some(env) = self.record_job_report(vr) {
                self.persist_job_report_to_session(VerificationReport::EVENT_NAME, &env);
            }
        }

        // RepairReport — repair job state present?
        let rr_observable = self.repair_job.is_some()
            || self.repair_failure_snapshot.is_some()
            || safe_stop.report_emitted;
        if rr_observable {
            let rr = RepairReport {
                turn_index,
                job_present: self.repair_job.is_some(),
                outcomes: Vec::new(),
                exhausted_promotion: None,
                no_progress_detected: false,
                safe_stop: safe_stop.clone(),
            };
            if let Some(env) = self.record_job_report(rr) {
                self.persist_job_report_to_session(RepairReport::EVENT_NAME, &env);
            }
        }

        // MemoryReport — PAM / context-pack state observable this turn?
        let mr_observable = !self.last_injected_summary_ids.is_empty()
            || self.photon_user_feedback_called_this_turn
            || !matches!(
                self.last_photon_context_pack_status,
                PhotonContextPackStatus::NoTurn
            );
        if mr_observable {
            let mr = MemoryReport {
                turn_index,
                pam_decision: None,
                context_pack_binding: None,
                adopted_item_count: self.last_injected_summary_ids.len() as u32,
                injection_skipped_reason: None,
            };
            if let Some(env) = self.record_job_report(mr) {
                self.persist_job_report_to_session(MemoryReport::EVENT_NAME, &env);
            }
        }
    }

    /// Read the SafeStopLinkage from per-turn dedup state.
    ///
    /// The `safe_stop_report_emitted` HashSet holds every StopReason
    /// emitted in this turn. We surface the first reason via its `.label()`
    /// (stable across versions) for the linkage `reason` field.
    fn snapshot_safe_stop_linkage(&self) -> SafeStopLinkage {
        if self.safe_stop_report_emitted.is_empty() {
            return SafeStopLinkage::default();
        }
        let reason = self
            .safe_stop_report_emitted
            .iter()
            .next()
            .copied()
            .map(|r| r.as_str().to_string());
        SafeStopLinkage {
            reason,
            report_emitted: true,
        }
    }

    /// Single emit chokepoint for any `JobReport`.
    ///
    /// Pipeline (Section 5-1 of design):
    /// 1. Per-turn dedup check (`{event_name}::{dedup_key}`)
    /// 2. `build_envelope::<R>`
    /// 3. `enforce_bounds` (truncate / drop / flag)
    /// 4. Clone-for-caller (because `log_llm_event` consumes the Value)
    /// 5. `log_llm_event(EVENT_NAME, envelope)` — runs
    ///    `mask_payload_inplace` last-defence
    /// 6. Return `Some(envelope_for_caller)` for the persistence helper
    ///    (or `None` on dedup-skip)
    pub(super) fn record_job_report<R: JobReport>(
        &mut self,
        report: R,
    ) -> Option<serde_json::Value> {
        let key = format!("{}::{}", R::EVENT_NAME, report.dedup_key());
        if !self.job_report_dedup_keys.insert(key) {
            return None;
        }
        let mut envelope = build_envelope(&report);
        // Inject session_id at the ENVELOPE top level so downstream UAT
        // filters that traverse `payload.session_id` (precedent from
        // safe_stop_e2e_tests.rs::read_session_events) work without
        // nested-payload knowledge. The agent's session_id is the
        // session-creation UUID and is not redacted (not a secret).
        if let serde_json::Value::Object(obj) = &mut envelope {
            obj.insert(
                "session_id".to_string(),
                serde_json::Value::String(self.session_store.session_id().to_string()),
            );
        }
        let (overflowed, truncated) = enforce_bounds(&mut envelope);
        if overflowed {
            envelope["overflowed"] = serde_json::Value::Bool(true);
        }
        if truncated {
            envelope["truncated"] = serde_json::Value::Bool(true);
        }
        let envelope_for_caller = envelope.clone();
        log_llm_event(R::EVENT_NAME, envelope);
        Some(envelope_for_caller)
    }

    /// Persist an already-emitted envelope to `job-reports.jsonl`.
    ///
    /// Best-effort: errors are logged at ERROR level but NOT propagated —
    /// the agent loop continues and persistence failure for one Report
    /// does not skip emit/persist of another Report in the same turn
    /// (kind-level isolation, DR3-003).
    ///
    /// **DR4-001 final-defence**: callers pass an envelope that has
    /// already traversed `mask_payload_inplace` via `log_llm_event`.
    /// `mask_payload_inplace` is re-applied here defensively so any
    /// future caller that hand-builds an envelope still gets redaction.
    pub(super) fn persist_job_report_to_session(
        &self,
        event_name: &'static str,
        envelope: &serde_json::Value,
    ) {
        let recorded_at_unix_ms = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0);
        let mut envelope_for_persist = envelope.clone();
        crate::logging::mask_payload_inplace(&mut envelope_for_persist);
        let record = StructuredJobReportRecord::new(
            recorded_at_unix_ms,
            self.current_turn_index as u64,
            event_name.to_string(),
            envelope_for_persist,
        );
        let _ = record.schema_version; // silence unused-field lint if reached
        if let Err(e) = self.session_store.append_job_report(&record) {
            tracing::error!(
                error = %e,
                report_kind = event_name,
                turn_index = self.current_turn_index,
                schema_version = JOB_REPORT_SCHEMA_VERSION,
                "structured job report persist failed (best-effort)"
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_acr() -> ArtifactCompletionReport {
        ArtifactCompletionReport {
            turn_index: 7,
            job_present: true,
            attempt_outcomes: vec![],
            role_policy_violation: None,
            budget_state: None,
            artifact_projection_status: default_projection_status(),
            safe_stop: SafeStopLinkage::default(),
        }
    }

    #[test]
    fn job_report_trait_event_names_match_design() {
        assert_eq!(
            ArtifactCompletionReport::EVENT_NAME,
            "agent.artifact_completion.report"
        );
        assert_eq!(VerificationReport::EVENT_NAME, "agent.verification.report");
        assert_eq!(RepairReport::EVENT_NAME, "agent.repair.report");
        assert_eq!(MemoryReport::EVENT_NAME, "agent.memory.report");
    }

    #[test]
    fn payload_schema_version_is_v1_for_all_reports() {
        assert_eq!(ArtifactCompletionReport::PAYLOAD_SCHEMA_VERSION, 1);
        assert_eq!(VerificationReport::PAYLOAD_SCHEMA_VERSION, 1);
        assert_eq!(RepairReport::PAYLOAD_SCHEMA_VERSION, 1);
        assert_eq!(MemoryReport::PAYLOAD_SCHEMA_VERSION, 1);
    }

    #[test]
    fn dedup_key_is_deterministic_per_turn() {
        let a = sample_acr();
        let b = sample_acr();
        assert_eq!(a.dedup_key(), b.dedup_key());
    }

    #[test]
    fn build_envelope_emits_canonical_shape() {
        let r = sample_acr();
        let env = build_envelope(&r);
        assert_eq!(env["schema_version"], serde_json::json!(1));
        assert_eq!(env["kind"], "agent.artifact_completion.report");
        assert_eq!(env["dedup_key"], "turn:7");
        assert!(env["payload"].is_object());
    }

    #[test]
    fn enforce_bounds_truncates_oversize_field() {
        let r = ArtifactCompletionReport {
            turn_index: 1,
            job_present: true,
            attempt_outcomes: vec![],
            role_policy_violation: Some("x".repeat(MAX_FIELD_BYTES + 10)),
            budget_state: None,
            artifact_projection_status: default_projection_status(),
            safe_stop: SafeStopLinkage::default(),
        };
        let mut env = build_envelope(&r);
        let (overflowed, truncated) = enforce_bounds(&mut env);
        assert!(truncated);
        assert!(!overflowed);
        let masked = env["payload"]["role_policy_violation"]
            .as_str()
            .unwrap()
            .to_string();
        assert!(masked.ends_with(TRUNCATE_MARKER));
        // CB-005 fix: marker length is part of the cap; final string must
        // be ≤ MAX_FIELD_BYTES (not MAX_FIELD_BYTES + marker.len()).
        assert!(
            masked.len() <= MAX_FIELD_BYTES,
            "truncated field {} > MAX_FIELD_BYTES {}",
            masked.len(),
            MAX_FIELD_BYTES
        );
    }

    #[test]
    fn enforce_bounds_drops_envelope_when_total_exceeds_cap() {
        // Build 16 outcomes each 700 bytes so total > 8 KiB envelope cap.
        let outcomes: Vec<serde_json::Value> = (0..16)
            .map(|i| {
                serde_json::json!({
                    "i": i,
                    "blob": "y".repeat(700),
                })
            })
            .collect();
        let r = RepairReport {
            turn_index: 1,
            job_present: true,
            outcomes,
            exhausted_promotion: None,
            no_progress_detected: false,
            safe_stop: SafeStopLinkage::default(),
        };
        let mut env = build_envelope(&r);
        let (overflowed, _truncated) = enforce_bounds(&mut env);
        assert!(overflowed);
        assert_eq!(env["payload"].as_str(), Some("<dropped:overflow>"));
    }

    #[test]
    fn safe_stop_linkage_serde_round_trip_with_defaults() {
        let l = SafeStopLinkage {
            reason: Some("verifier_weak".to_string()),
            report_emitted: true,
        };
        let s = serde_json::to_string(&l).unwrap();
        let back: SafeStopLinkage = serde_json::from_str(&s).unwrap();
        assert_eq!(back.reason.as_deref(), Some("verifier_weak"));
        assert!(back.report_emitted);

        // Defaults round-trip cleanly (missing fields).
        let default: SafeStopLinkage = serde_json::from_str("{}").unwrap();
        assert_eq!(default.reason, None);
        assert!(!default.report_emitted);
    }

    #[test]
    fn memory_report_has_no_safe_stop_field() {
        // Compile-time check: MemoryReport struct fields do not include
        // safe_stop. We verify via serde round-trip — absent field stays
        // absent.
        let m = MemoryReport {
            turn_index: 3,
            pam_decision: None,
            context_pack_binding: None,
            adopted_item_count: 0,
            injection_skipped_reason: None,
        };
        let env = build_envelope(&m);
        assert!(env["payload"].get("safe_stop").is_none());
    }
}
