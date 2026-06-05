//! Issue #994 (parent #988, Issue F) — ContractConflictJob E2E (in-crate
//! `#[cfg(test)]` suite, CB-001 fix pattern; `job_report_e2e_tests.rs`
//! precedent).
//!
//! A real `Agent` is constructed against a mockito-loopback Ollama URL and a
//! TempDir state root. We seed `agent.repair_job` with a `SemanticRepairPlan`
//! carrying a contract conflict, drive the production hook
//! (`maybe_record_contract_arbitration_on_repair_exhausted`), then emit via
//! `Agent::maybe_emit_job_reports` to exercise the full typed-payload pipeline
//! (`record_job_report → build_envelope → enforce_bounds → log_llm_event →
//! mask_payload_inplace` + `persist_job_report_to_session`). No real LLM is
//! invoked. Production binary excludes this module (DR3-001).

use std::path::PathBuf;
use std::sync::OnceLock;

use serde_json::Value;
use tempfile::{TempDir, tempdir};

use super::VerifierDiagnosticFailureKind;
use super::contract_conflict_job::maybe_record_contract_arbitration_on_repair_exhausted;
use super::repair_job::{RepairJob, SemanticRepairPlan};
use super::semantic_failure::{
    ContractConflict, FailureCluster, RawClusterTargetCandidate, SemanticFailureReport,
    cluster_key_for_test,
};
use super::spec_authority::SpecAuthority;
use super::task_contract::ArtifactRole;
use crate::agent::Agent;
use crate::agent::loop_run::FooterHandle;
use crate::config::Config;
use crate::model_registry::RuntimeModels;
use crate::ollama::client::OllamaClient;
use crate::session::store::{SessionSnapshot, SessionStore};

const EVENT_CONTRACT_ARBITRATION: &str = "agent.contract_arbitration.report";

static LOG_DIR: OnceLock<TempDir> = OnceLock::new();

fn shared_log_path() -> PathBuf {
    let _ = LOG_DIR.get_or_init(|| {
        let dir = tempdir().expect("tempdir for shared log");
        let log_path = dir.path().join("llm-io.jsonl");
        let _ = crate::logging::init_logging(crate::config::LogLevel::Info, &log_path);
        dir
    });
    crate::logging::llm_io_log_path()
        .map(|p| p.to_path_buf())
        .expect("logging must be initialised by this point")
}

fn read_session_events(session_id: &str) -> Vec<Value> {
    let path = shared_log_path();
    let contents = match std::fs::read_to_string(&path) {
        Ok(s) => s,
        Err(_) => return Vec::new(),
    };
    contents
        .lines()
        .filter_map(|line| serde_json::from_str::<Value>(line).ok())
        .filter(|rec| {
            rec.get("payload")
                .and_then(|p| p.get("session_id"))
                .and_then(|v| v.as_str())
                == Some(session_id)
        })
        .collect()
}

fn events_by_name(session_id: &str, event_name: &str) -> Vec<Value> {
    read_session_events(session_id)
        .into_iter()
        .filter(|rec| rec.get("event").and_then(|v| v.as_str()) == Some(event_name))
        .collect()
}

fn unique_session_id(prefix: &str) -> String {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let pid = std::process::id();
    format!("{prefix}-{pid}-{nanos}-12345678-1234-1234-1234-123456789012")
}

fn build_live_agent(session_id: &str) -> (Agent, TempDir) {
    // Ensure the global logger is initialised BEFORE any emit so events are
    // captured even when this suite runs in isolation (no sibling e2e test has
    // initialised logging first).
    let _ = shared_log_path();
    let dir = tempdir().unwrap();
    let state_root = dir.path().join("state");
    std::fs::create_dir_all(state_root.join("sessions").join(session_id)).unwrap();

    let config = Config {
        cwd: dir.path().to_path_buf(),
        requested_model: Some("test-model".to_string()),
        state_dir_override: Some(state_root.clone()),
        yes_mode: true,
        max_iterations: 1,
        ..Config::default()
    };

    let workspace_key = format!("anvil-994-{session_id}");
    let session = SessionSnapshot {
        id: session_id.to_string(),
        workspace_key: workspace_key.clone(),
        ..Default::default()
    };

    let agent = Agent::new(
        config,
        RuntimeModels {
            main: "test-model".to_string(),
            sidecar: None,
        },
        OllamaClient::new("http://127.0.0.1:1".to_string()).unwrap(),
        SessionStore::new(&state_root, session_id, &workspace_key),
        session,
        FooterHandle::disabled(),
    );
    (agent, dir)
}

// ---------------------------------------------------------------------------
// Conflict-report fixture builders
// ---------------------------------------------------------------------------

fn cluster(
    label: &str,
    observed: &str,
    expected: &str,
    roles: Vec<ArtifactRole>,
    proposed: Vec<RawClusterTargetCandidate>,
) -> FailureCluster {
    FailureCluster {
        cluster_key: cluster_key_for_test(label),
        observed: observed.to_string(),
        expected: expected.to_string(),
        affected_cases: Vec::new(),
        involved_artifacts: roles,
        proposed_target_candidates: proposed,
        admitted_cluster_targets: Vec::new(),
    }
}

fn report(
    implementation: &str,
    test: &str,
    usage_docs: &str,
    clusters: Vec<FailureCluster>,
    confidence: f32,
) -> SemanticFailureReport {
    SemanticFailureReport {
        failure_kind: VerifierDiagnosticFailureKind::AssertionMismatch,
        failure_clusters: clusters,
        contract_conflict: ContractConflict {
            implementation: implementation.to_string(),
            test: test.to_string(),
            usage_docs: usage_docs.to_string(),
        },
        preferred_repair_role: ArtifactRole::Implementation,
        repair_hypothesis: String::new(),
        confidence,
    }
}

/// Install a repair job whose semantic plan carries `report`, so the
/// production hook can read it from `agent.repair_job.semantic_plan`.
fn seed_repair_job(agent: &mut Agent, report: SemanticFailureReport) {
    let cluster_id = report
        .failure_clusters
        .first()
        .map(|c| c.cluster_key.clone())
        .unwrap_or_else(|| cluster_key_for_test("empty"));
    let plan = SemanticRepairPlan {
        semantic_report: report,
        failure_cluster_id: cluster_id,
        semantic_cause: VerifierDiagnosticFailureKind::AssertionMismatch,
        spec_authority: SpecAuthority::ImplementationContract,
        preferred_repair_role: ArtifactRole::Implementation,
        repair_hypothesis: "h".to_string(),
        expected_improvement: None,
        assessment_generation_at_creation: 0,
    };
    agent.repair_job = Some(RepairJob {
        semantic_plan: Some(plan),
        ..RepairJob::new_for_test()
    });
}

/// Drive the production chokepoint + emit, returning the emitted
/// `agent.contract_arbitration.report` events for the session.
fn arbitrate_and_emit(agent: &mut Agent, session_id: &str) -> Vec<Value> {
    maybe_record_contract_arbitration_on_repair_exhausted(agent);
    agent.maybe_emit_job_reports();
    events_by_name(session_id, EVENT_CONTRACT_ARBITRATION)
}

fn decision_of(event: &Value) -> &Value {
    &event["payload"]["payload"]["decision"]
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[test]
fn fastapi_conflict_emits_typed_decision() {
    let session_id = unique_session_id("contract994-fastapi");
    let (mut agent, _dir) = build_live_agent(&session_id);
    // impl + docs agree on 200; the test expects 404 -> consensus tie-break.
    let rep = report(
        "returns 200",
        "expects 404",
        "documents 200",
        vec![cluster(
            "fastapi-status",
            "200",
            "404",
            vec![ArtifactRole::Implementation, ArtifactRole::Test],
            Vec::new(),
        )],
        0.7,
    );
    seed_repair_job(&mut agent, rep);

    let events = arbitrate_and_emit(&mut agent, &session_id);
    assert_eq!(events.len(), 1, "exactly one contract arbitration report");
    let ev = &events[0];
    assert_eq!(ev["payload"]["payload"]["classified"], true);
    assert_eq!(ev["payload"]["payload"]["actionable"], true);
    let decision = decision_of(ev);
    // All six typed fields are present.
    assert_eq!(decision["authoritative_role"], "implementation");
    assert_eq!(decision["weaker_role"], "test");
    assert_eq!(decision["allowed_change_kind"], "fix_test");
    assert!(decision.get("confidence").is_some());
    assert!(decision.get("reason").is_some());
    assert!(decision.get("target").is_some());
}

#[test]
fn toml_setup_vs_test_conflict_classifies() {
    let session_id = unique_session_id("contract994-toml");
    let (mut agent, _dir) = build_live_agent(&session_id);
    // Cargo.toml [lib] name vs test import; no impl/test/docs three-way text.
    let rep = report(
        "",
        "",
        "",
        vec![cluster(
            "toml-lib-name",
            "lib name is `mycrate`",
            "test imports `othercrate`",
            vec![ArtifactRole::Setup, ArtifactRole::Test],
            Vec::new(),
        )],
        0.6,
    );
    seed_repair_job(&mut agent, rep);

    let events = arbitrate_and_emit(&mut agent, &session_id);
    assert_eq!(events.len(), 1);
    let ev = &events[0];
    let roles = ev["payload"]["payload"]["involved_roles"]
        .as_array()
        .expect("involved_roles array");
    let role_labels: Vec<&str> = roles.iter().filter_map(|v| v.as_str()).collect();
    assert!(
        role_labels.contains(&"setup"),
        "setup involved: {role_labels:?}"
    );
    assert!(
        role_labels.contains(&"test"),
        "test involved: {role_labels:?}"
    );
    let decision = decision_of(ev);
    assert_eq!(decision["authoritative_role"], "setup");
    assert_eq!(decision["weaker_role"], "test");
}

#[test]
fn docs_data_research_share_the_same_lifecycle() {
    // docs conflict -> fix_usage_docs.
    {
        let session_id = unique_session_id("contract994-docs");
        let (mut agent, _dir) = build_live_agent(&session_id);
        let rep = report(
            "computes mean",
            "",
            "documents median",
            vec![cluster(
                "docs",
                "mean",
                "median",
                vec![ArtifactRole::Implementation, ArtifactRole::UsageDocs],
                Vec::new(),
            )],
            0.6,
        );
        seed_repair_job(&mut agent, rep);
        let events = arbitrate_and_emit(&mut agent, &session_id);
        assert_eq!(events.len(), 1);
        assert_eq!(
            decision_of(&events[0])["allowed_change_kind"],
            "fix_usage_docs"
        );
    }
    // data conflict -> fix_data_output.
    {
        let session_id = unique_session_id("contract994-data");
        let (mut agent, _dir) = build_live_agent(&session_id);
        let rep = report(
            "",
            "",
            "",
            vec![cluster(
                "data-schema",
                "csv has 3 columns",
                "schema declares 4",
                vec![ArtifactRole::Implementation, ArtifactRole::DataOutput],
                Vec::new(),
            )],
            0.6,
        );
        seed_repair_job(&mut agent, rep);
        let events = arbitrate_and_emit(&mut agent, &session_id);
        assert_eq!(events.len(), 1);
        assert_eq!(
            decision_of(&events[0])["allowed_change_kind"],
            "fix_data_output"
        );
    }
    // research conflict (UsageDocs role) -> fix_usage_docs.
    {
        let session_id = unique_session_id("contract994-research");
        let (mut agent, _dir) = build_live_agent(&session_id);
        let rep = report(
            "returns cached value",
            "",
            "report claims live fetch",
            vec![cluster(
                "research",
                "cached",
                "live",
                vec![ArtifactRole::Implementation, ArtifactRole::UsageDocs],
                Vec::new(),
            )],
            0.6,
        );
        seed_repair_job(&mut agent, rep);
        let events = arbitrate_and_emit(&mut agent, &session_id);
        assert_eq!(events.len(), 1);
        assert_eq!(
            decision_of(&events[0])["allowed_change_kind"],
            "fix_usage_docs"
        );
    }
}

#[test]
fn report_envelope_is_schema_v1_with_stable_event_name() {
    let session_id = unique_session_id("contract994-schema");
    let (mut agent, _dir) = build_live_agent(&session_id);
    let rep = report(
        "returns 200",
        "expects 404",
        "documents 200",
        vec![cluster(
            "schema",
            "200",
            "404",
            vec![ArtifactRole::Implementation, ArtifactRole::Test],
            Vec::new(),
        )],
        0.7,
    );
    seed_repair_job(&mut agent, rep);
    let events = arbitrate_and_emit(&mut agent, &session_id);
    assert_eq!(events.len(), 1);
    let envelope = &events[0]["payload"];
    assert_eq!(envelope["schema_version"], 1);
    assert_eq!(envelope["kind"], EVENT_CONTRACT_ARBITRATION);
    // NOTE: `dedup_key` ("turn:0") is redacted to "***" by `mask_payload_inplace`
    // because the field name is secret-like ("key"). Per-turn dedup is covered
    // by `hook_is_idempotent_within_a_turn` instead.
    assert!(envelope.get("dedup_key").is_some());
    assert_eq!(events[0]["event"], EVENT_CONTRACT_ARBITRATION);
}

#[test]
fn single_role_failure_does_not_record_a_conflict() {
    let session_id = unique_session_id("contract994-single");
    let (mut agent, _dir) = build_live_agent(&session_id);
    let rep = report(
        "returns 200",
        "",
        "",
        vec![cluster(
            "single",
            "boom",
            "",
            vec![ArtifactRole::Implementation],
            Vec::new(),
        )],
        0.8,
    );
    seed_repair_job(&mut agent, rep);
    maybe_record_contract_arbitration_on_repair_exhausted(&mut agent);
    assert!(
        agent.last_contract_conflict_job_this_turn.is_none(),
        "single-role failure must not be classified as a contract conflict"
    );
}

#[test]
fn secret_in_target_is_masked_in_emitted_payload() {
    let session_id = unique_session_id("contract994-secret");
    let (mut agent, _dir) = build_live_agent(&session_id);
    let rep = report(
        "returns 200",
        "expects 404",
        "documents 200",
        vec![cluster(
            "secret",
            "200",
            "404",
            vec![ArtifactRole::Implementation, ArtifactRole::Test],
            vec![RawClusterTargetCandidate {
                raw_path: "tests/api.rs?token=sk-secret-994-value".to_string(),
                role_hint: Some(ArtifactRole::Test),
                reason: String::new(),
            }],
        )],
        0.7,
    );
    seed_repair_job(&mut agent, rep);
    let events = arbitrate_and_emit(&mut agent, &session_id);
    assert_eq!(events.len(), 1);
    let serialized = serde_json::to_string(&events[0]).unwrap();
    assert!(
        !serialized.contains("sk-secret-994-value"),
        "raw secret leaked into emitted payload: {serialized}"
    );
}

#[test]
fn hook_is_idempotent_within_a_turn() {
    let session_id = unique_session_id("contract994-idem");
    let (mut agent, _dir) = build_live_agent(&session_id);
    let rep = report(
        "returns 200",
        "expects 404",
        "documents 200",
        vec![cluster(
            "idem",
            "200",
            "404",
            vec![ArtifactRole::Implementation, ArtifactRole::Test],
            Vec::new(),
        )],
        0.7,
    );
    seed_repair_job(&mut agent, rep);
    maybe_record_contract_arbitration_on_repair_exhausted(&mut agent);
    let round_after_first = agent
        .last_contract_conflict_job_this_turn
        .as_ref()
        .map(|j| j.arbitration_round());
    // A second hook call in the same turn must NOT re-arbitrate (per-turn cap).
    maybe_record_contract_arbitration_on_repair_exhausted(&mut agent);
    let round_after_second = agent
        .last_contract_conflict_job_this_turn
        .as_ref()
        .map(|j| j.arbitration_round());
    assert_eq!(round_after_first, Some(1));
    assert_eq!(round_after_second, Some(1));

    // Emit dedups per turn: exactly one report despite the double-emit path
    // (hook records once; the report fires once).
    agent.maybe_emit_job_reports();
    agent.maybe_emit_job_reports();
    let events = events_by_name(&session_id, EVENT_CONTRACT_ARBITRATION);
    assert_eq!(events.len(), 1, "per-turn dedup must keep a single report");
}
