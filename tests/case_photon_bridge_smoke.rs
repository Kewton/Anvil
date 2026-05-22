//! Case → Photon bridge E2E smoke tests (Issue #593, Phase A).
//!
//! 12 cases CB-01 .. CB-12 covering the full promotion pipeline. All cases
//! are Ollama-free and complete in well under 5 seconds; they exercise the
//! Public API of `case_photon_bridge` plus `sessions_cli::run_photon_promote`
//! directly (no clap argument parsing).

use std::collections::HashSet;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use anvil::safety::host_validation::validate_localhost_url;
use anvil::session::anvil_score::AnvilScore;
use anvil::session::case_photon_bridge::{
    ACTION_SUMMARY_SCHEMA_VERSION, ActionSummary, FRESHNESS_THRESHOLD_DAYS,
    MAX_PHOTON_PROMOTE_LOG_BYTES, PromoteLogEntry, SUMMARY_ID_PREFIX, SkipReason,
    append_promote_log_entry, check_quality_gate, convert_case_to_action_summary,
    format_rfc3339_utc, read_promote_log,
};
use anvil::session::case_record::{CaseRecord, RepoFingerprint, persist};
use anvil::session::feedback::FeedbackKind;
use anvil::session::sessions_cli::run_photon_promote;
use tempfile::TempDir;

// ---------------------------------------------------------------------------
// Fixture helpers
// ---------------------------------------------------------------------------

fn baseline_score() -> AnvilScore {
    AnvilScore {
        build_passed: Some(true),
        tests_passed: Some(true),
        compile_errors_delta: Some(0),
        test_failures_delta: Some(0),
        compile_error_count: Some(0),
        test_failure_count: Some(0),
        implementation_files_changed: Some(1),
        test_files_changed: Some(1),
        setup_files_changed: Some(0),
        unsafe_actions_blocked: 0,
        consecutive_no_progress_turns: 0,
        user_visible_artifact: true,
    }
}

fn now_unix() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(1_700_000_000)
}

fn make_case(case_id: &str) -> CaseRecord {
    CaseRecord {
        case_id: case_id.to_string(),
        created_at: now_unix(),
        repo_fingerprint: RepoFingerprint {
            workspace_key: "ws-key".to_string(),
            git_remote: Some("https://github.com/o/r.git".to_string()),
            git_head_branch: Some("main".to_string()),
            language_stack_hash: "deadbeefdeadbeef".to_string(),
        },
        task_signature: "fix bug".to_string(),
        language_stack: vec!["rust".to_string()],
        initial_feedback: vec![FeedbackKind::CompileError],
        successful_precautions: vec![],
        changed_files_summary: vec![],
        verify_commands: vec!["cargo test".to_string()],
        outcome_score: baseline_score(),
    }
}

fn write_case(state_root: &std::path::Path, case: &CaseRecord) {
    persist(state_root, case).expect("persist case");
}

fn unique_case_id(idx: usize) -> String {
    // 24 hex-ish lowercase chars satisfying the case_id allowlist.
    let body = format!("{:0>24x}", idx);
    format!("case_{body}")
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

// CB-01: verifier-passed + user_visible_artifact → confidence=1.0 → promoted.
#[test]
fn cb_01_verifier_passed_with_artifact_promotes_with_confidence_one() {
    let tmp = TempDir::new().unwrap();
    let c = make_case(&unique_case_id(1));
    write_case(tmp.path(), &c);

    let n = run_photon_promote(
        tmp.path(),
        tmp.path(),
        None,
        None,
        true,
        false,
        true,
        false,
        None,
    )
    .unwrap();
    assert_eq!(n, 0);

    // Verify dedup log contains the case.
    let promoted = read_promote_log(tmp.path()).unwrap();
    assert!(
        promoted.contains(&c.case_id),
        "case_id must be in dedup log, got: {promoted:?}"
    );

    // Confirm via convert_case_to_action_summary that confidence=1.0.
    let summary = convert_case_to_action_summary(&c, "sess-1", "2026-05-16T00:00:00Z");
    let prov = summary.provenance.unwrap();
    assert_eq!(prov.confidence_prior, 1.0);
    assert!(prov.verifier_active);
}

// CB-02: verifier-less success → confidence=0.6, verifier_active=false.
#[test]
fn cb_02_verifier_less_success_uses_0_6_confidence() {
    let tmp = TempDir::new().unwrap();
    let mut c = make_case(&unique_case_id(2));
    c.outcome_score.build_passed = None;
    c.outcome_score.tests_passed = None;
    c.outcome_score.user_visible_artifact = true;
    write_case(tmp.path(), &c);

    run_photon_promote(
        tmp.path(),
        tmp.path(),
        None,
        None,
        true,
        false,
        true,
        false,
        None,
    )
    .unwrap();

    let promoted = read_promote_log(tmp.path()).unwrap();
    assert!(promoted.contains(&c.case_id));

    let summary = convert_case_to_action_summary(&c, "sess", "t");
    let prov = summary.provenance.unwrap();
    assert!(!prov.verifier_active);
    assert_eq!(prov.confidence_prior, 0.6);
}

// CB-03: empty language_stack → SkipReason::EmptyLanguageStack.
#[test]
fn cb_03_empty_language_stack_skips() {
    let mut c = make_case(&unique_case_id(3));
    c.language_stack = vec![];
    let promoted_set: HashSet<String> = HashSet::new();
    let r = check_quality_gate(&c, &promoted_set, c.created_at + 1);
    assert_eq!(r, Err(SkipReason::EmptyLanguageStack));
}

// CB-04: empty task_signature → SkipReason::EmptyTaskSignature.
#[test]
fn cb_04_empty_task_signature_skips() {
    let mut c = make_case(&unique_case_id(4));
    c.task_signature = "  \t  ".to_string();
    let promoted_set: HashSet<String> = HashSet::new();
    let r = check_quality_gate(&c, &promoted_set, c.created_at + 1);
    assert_eq!(r, Err(SkipReason::EmptyTaskSignature));
}

// CB-05: created_at > 30 days → SkipReason::StaleCase.
#[test]
fn cb_05_stale_case_over_30_days_skips() {
    let c = make_case(&unique_case_id(5));
    let now = c.created_at + (FRESHNESS_THRESHOLD_DAYS + 1) * 86_400;
    let promoted_set: HashSet<String> = HashSet::new();
    let r = check_quality_gate(&c, &promoted_set, now);
    assert_eq!(r, Err(SkipReason::StaleCase));
}

// CB-06: same case_id submitted twice → SkipReason::AlreadyPromoted on 2nd.
#[test]
fn cb_06_already_promoted_skips_on_second_submission() {
    let tmp = TempDir::new().unwrap();
    let c = make_case(&unique_case_id(6));
    write_case(tmp.path(), &c);

    // First call promotes.
    run_photon_promote(
        tmp.path(),
        tmp.path(),
        None,
        None,
        true,
        false,
        true,
        false,
        None,
    )
    .unwrap();
    let promoted_after_first = read_promote_log(tmp.path()).unwrap();
    assert!(promoted_after_first.contains(&c.case_id));

    // Second call: should detect AlreadyPromoted via dedup set.
    let already = read_promote_log(tmp.path()).unwrap();
    let r = check_quality_gate(&c, &already, c.created_at + 1);
    assert_eq!(r, Err(SkipReason::AlreadyPromoted));
}

// CB-07: dry-run table contains a header row and one data row.
#[test]
fn cb_07_dry_run_table_format() {
    // We exercise the path by calling run_photon_promote with dry_run=true.
    // The function prints to stdout; we can't capture easily without piping,
    // but we can at least verify it returns 0 and writes nothing to the log.
    let tmp = TempDir::new().unwrap();
    let c = make_case(&unique_case_id(7));
    write_case(tmp.path(), &c);

    let rc = run_photon_promote(
        tmp.path(),
        tmp.path(),
        None,
        None,
        true,
        true,
        false,
        false,
        None,
    )
    .unwrap();
    assert_eq!(rc, 0);

    // Dry-run must NOT write to the dedup log.
    let log_path = tmp.path().join("photon-promote-log.jsonl");
    assert!(
        !log_path.exists(),
        "dry-run must not create the dedup log file"
    );
}

// CB-08: photon URL with credentials must be rejected by validate_localhost_url.
#[test]
fn cb_08_photon_url_with_credentials_rejected() {
    let r = validate_localhost_url("https://user:pw@127.0.0.1:3030".to_string(), "photon_url");
    assert!(r.is_err(), "credential-bearing photon_url must be rejected");
    let msg = r.unwrap_err();
    assert!(
        msg.contains("credentials"),
        "error msg should mention credentials, got: {msg}"
    );
}

// CB-09: --case-id + --print-summary → 1 ActionSummary line on stdout.
#[test]
fn cb_09_case_id_print_summary_emits_single_summary() {
    let tmp = TempDir::new().unwrap();
    let c = make_case(&unique_case_id(9));
    let other = make_case(&unique_case_id(99));
    write_case(tmp.path(), &c);
    write_case(tmp.path(), &other);

    // Drive the conversion directly so we can validate output deterministically.
    let summary = convert_case_to_action_summary(&c, &c.case_id, "2026-05-16T00:00:00Z");
    let line = serde_json::to_string(&summary).unwrap();
    assert!(line.starts_with('{') && line.ends_with('}'));
    assert!(line.contains(&format!("{SUMMARY_ID_PREFIX}{}", c.case_id)));
    // The CLI handler itself is exercised in CB-01 / CB-02 / CB-06; here we
    // confirm the --case-id pre-filter does not collide with print_summary.
    run_photon_promote(
        tmp.path(),
        tmp.path(),
        None,
        Some(c.case_id.clone()),
        false,
        true,
        false,
        true,
        None,
    )
    .unwrap();
}

// Lightweight in-test mirror of `logging::is_secret_like_key` semantics so
// E2E does not depend on a `pub(crate)` helper. Mirrors the canonical list
// of substring / suffix triggers documented in `logging.rs`.
fn is_secret_like_key_mirror(key: &str) -> bool {
    let lk = key.to_ascii_lowercase();
    const SUBSTRINGS: &[&str] = &[
        "api_key",
        "token",
        "secret",
        "password",
        "access_key",
        "client_secret",
    ];
    for s in SUBSTRINGS {
        if lk.contains(s) {
            return true;
        }
    }
    lk.ends_with("_key") || lk.ends_with("_token")
}

// CB-10: all 7 provenance keys are not secret-like (would survive mask)
// AND the actual values survive a full `mask_payload_inplace` round-trip
// unchanged (the most direct E2E check).
#[test]
fn cb_10_all_provenance_keys_are_not_secret_like() {
    let c = make_case(&unique_case_id(10));
    let summary = convert_case_to_action_summary(&c, "sess-X", "2026-05-16T00:00:00Z");
    let v = serde_json::to_value(&summary).unwrap();
    let prov = v.get("_provenance").expect("provenance present");
    let prov_obj = prov.as_object().expect("provenance is object");
    let expected_keys = [
        "source",
        "case_id",
        "session_id",
        "anvil_version",
        "extracted_at",
        "confidence_prior",
        "verifier_active",
    ];
    for k in expected_keys {
        assert!(
            prov_obj.contains_key(k),
            "provenance must contain key {k}, keys: {:?}",
            prov_obj.keys().collect::<Vec<_>>()
        );
        assert!(
            !is_secret_like_key_mirror(k),
            "key {k} must not be secret-like (would be masked away)"
        );
    }
    // End-to-end: the values survive `apply_defensive_mask` (already invoked
    // by `convert_case_to_action_summary`), so the session_id we passed in
    // is still readable.
    let prov_session = prov_obj
        .get("session_id")
        .and_then(|v| v.as_str())
        .unwrap_or("");
    assert_eq!(prov_session, "sess-X");
    // Sanity: the schema version is unchanged.
    assert_eq!(summary.schema_version, ACTION_SUMMARY_SCHEMA_VERSION);
}

// CB-11: oversize ActionSummary → SkipReason::Oversize.
//
// CaseRecord on disk is capped at 16 KiB pretty-printed by `persist`, so we
// cannot drive this skip through the CLI path. Instead we verify the size
// gate semantics: build a CaseRecord that serializes (compact) into an
// ActionSummary > MAX_ACTION_SUMMARY_BYTES, then assert that the size check
// fires (the CLI uses the same check on `serde_json::to_vec(&summary)`).
#[test]
fn cb_11_oversize_action_summary_exceeds_cap() {
    use anvil::session::case_photon_bridge::MAX_ACTION_SUMMARY_BYTES;
    let mut c = make_case(&unique_case_id(11));
    // Each verify_command becomes a Hint (kind=verify, target=cmd). Targets
    // of ~100 bytes × 200 ≈ 20 KiB. The encoded ActionSummary will clear the
    // 16 KiB cap.
    let big: Vec<String> = (0..200)
        .map(|i| format!("cargo run --bin xx_{i:0>100}"))
        .collect();
    c.verify_commands = big;

    let summary = convert_case_to_action_summary(&c, "sess", "2026-05-16T00:00:00Z");
    let serialized = serde_json::to_vec(&summary).unwrap();
    assert!(
        serialized.len() > MAX_ACTION_SUMMARY_BYTES,
        "expected serialized > {MAX_ACTION_SUMMARY_BYTES} bytes, got {}",
        serialized.len()
    );
}

// CB-12: photon-promote-log.jsonl > 1MiB → warning emitted, append continues.
#[test]
fn cb_12_large_log_warns_but_continues_appending() {
    let tmp = TempDir::new().unwrap();
    let log_path = tmp.path().join("photon-promote-log.jsonl");
    // Build a "fake" log file just above MAX_PHOTON_PROMOTE_LOG_BYTES.
    let filler = "a".repeat((MAX_PHOTON_PROMOTE_LOG_BYTES as usize) + 1024);
    std::fs::write(&log_path, &filler).unwrap();

    let entry = PromoteLogEntry {
        case_id: "case_aaaaaaaaaaaaaaaaaaaaaaaa".into(),
        summary_id: "anvil-case-case_aaaaaaaaaaaaaaaaaaaaaaaa".into(),
        extracted_at: format_rfc3339_utc(1_700_000_000),
        outcome: "promoted".into(),
        reason: None,
    };
    // Append must succeed (rotation is Phase B). The warning is emitted via
    // stderr and is not captured here, but we assert the append happened.
    append_promote_log_entry(tmp.path(), &entry).unwrap();

    let body = std::fs::read_to_string(&log_path).unwrap();
    let new_size = body.len();
    assert!(
        new_size > filler.len(),
        "appended bytes should grow the file, was {} now {}",
        filler.len(),
        new_size
    );
    assert!(body.contains("case_aaaaaaaaaaaaaaaaaaaaaaaa"));
}

// Sanity: ensure the CLI handler returns a non-error on a fully empty state.
#[test]
fn empty_state_root_returns_ok() {
    let tmp = TempDir::new().unwrap();
    let rc = run_photon_promote(
        tmp.path(),
        tmp.path(),
        None,
        None,
        true,
        true,
        false,
        false,
        None,
    )
    .unwrap();
    assert_eq!(rc, 0);
}

// Sanity: --output writes a JSONL file with one line per promoted summary.
#[test]
fn output_flag_writes_jsonl_one_line_per_summary() {
    let tmp = TempDir::new().unwrap();
    let c1 = make_case(&unique_case_id(50));
    let c2 = make_case(&unique_case_id(51));
    write_case(tmp.path(), &c1);
    write_case(tmp.path(), &c2);

    let out: PathBuf = tmp.path().join("seeds.jsonl");
    run_photon_promote(
        tmp.path(),
        tmp.path(),
        None,
        None,
        true,
        false,
        true,
        false,
        Some(out.clone()),
    )
    .unwrap();

    let body = std::fs::read_to_string(&out).unwrap();
    let lines: Vec<_> = body.lines().collect();
    assert_eq!(
        lines.len(),
        2,
        "expected 2 jsonl lines, got {}",
        lines.len()
    );
    for line in lines {
        let parsed: ActionSummary = serde_json::from_str(line).unwrap();
        assert_eq!(parsed.schema_version, ACTION_SUMMARY_SCHEMA_VERSION);
    }
}

// ---------------------------------------------------------------------------
// Issue #608 Phase α-2 (AP-10 / 案A): tests-only success regression
// ---------------------------------------------------------------------------

/// AP-10: a CaseRecord with `tests_passed=true` and no artifact, where
/// `build_passed` is None, must still pass the quality gate so it is
/// promoted to a photon seed. Pins the design 設計判断 #1 (案A) interpretation.
#[test]
fn ap10_tests_only_success_passes_quality_gate() {
    let mut c = make_case(&unique_case_id(70));
    c.outcome_score.build_passed = None;
    c.outcome_score.tests_passed = Some(true);
    c.outcome_score.user_visible_artifact = false;
    let promoted: HashSet<String> = HashSet::new();
    let r = check_quality_gate(&c, &promoted, c.created_at + 1);
    assert!(
        r.is_ok(),
        "tests-only success must pass quality gate, got {:?}",
        r
    );
}

/// AP-10: `build_passed == Some(false)` must override tests-only success
/// (verifier failure > test success per design 設計判断 #1).
#[test]
fn ap10_build_failure_overrides_tests_only_success() {
    let mut c = make_case(&unique_case_id(71));
    c.outcome_score.build_passed = Some(false);
    c.outcome_score.tests_passed = Some(true);
    c.outcome_score.user_visible_artifact = false;
    let promoted: HashSet<String> = HashSet::new();
    let r = check_quality_gate(&c, &promoted, c.created_at + 1);
    assert_eq!(r, Err(SkipReason::NotSuccessState));
}

/// AP-10: tests_only path runs through full pipeline (convert →
/// ActionSummary serialization). Confidence comes from the verifier-active
/// 3-tier derivation: tests_passed=true && user_visible_artifact=false ⇒
/// confidence_prior = 0.8.
#[test]
fn ap10_tests_only_success_yields_08_confidence_in_summary() {
    let mut c = make_case(&unique_case_id(72));
    c.outcome_score.tests_passed = Some(true);
    c.outcome_score.user_visible_artifact = false;
    let s = convert_case_to_action_summary(&c, "sess-ap10", "2026-05-17T00:00:00Z");
    let prov = s.provenance.unwrap();
    assert!(prov.verifier_active);
    assert_eq!(prov.confidence_prior, 0.8);
    assert!(s.summary_id.starts_with(SUMMARY_ID_PREFIX));
}

// ---------------------------------------------------------------------------
// CB-002 (codex review fix): unsafe_actions_blocked must block full_pass
// promotion even when build / tests / artifact are all green.
// ---------------------------------------------------------------------------

/// CB-002: a CaseRecord with `build_passed = true`, `tests_passed = true`,
/// `user_visible_artifact = true` but `unsafe_actions_blocked > 0` MUST
/// be rejected by the quality gate with `SkipReason::NotSuccessState`.
/// Previously the full_pass branch ignored `unsafe_actions_blocked`,
/// allowing an unsafe turn to be promoted into a photon seed.
#[test]
fn cb_002_full_pass_with_unsafe_blocks_fails_quality_gate() {
    let mut c = make_case(&unique_case_id(80));
    // baseline_score() already has unsafe_actions_blocked=0; flip it.
    c.outcome_score.build_passed = Some(true);
    c.outcome_score.tests_passed = Some(true);
    c.outcome_score.user_visible_artifact = true;
    c.outcome_score.unsafe_actions_blocked = 1;
    let promoted: HashSet<String> = HashSet::new();
    let r = check_quality_gate(&c, &promoted, c.created_at + 1);
    assert_eq!(
        r,
        Err(SkipReason::NotSuccessState),
        "CB-002: unsafe_actions_blocked > 0 must trigger NotSuccessState even on full_pass"
    );
}

/// CB-002: the verifier-less fallback already enforced
/// `unsafe_actions_blocked == 0`. Pin that path so the invariant holds
/// across both branches.
#[test]
fn cb_002_verifier_less_with_unsafe_blocks_fails_quality_gate() {
    let mut c = make_case(&unique_case_id(81));
    c.outcome_score.build_passed = None;
    c.outcome_score.tests_passed = None;
    c.outcome_score.user_visible_artifact = true;
    c.outcome_score.unsafe_actions_blocked = 1;
    let promoted: HashSet<String> = HashSet::new();
    let r = check_quality_gate(&c, &promoted, c.created_at + 1);
    assert_eq!(r, Err(SkipReason::NotSuccessState));
}

/// CB-002 promotion-pipeline regression: a CaseRecord with
/// `unsafe_actions_blocked > 0` must be SKIPPED end-to-end by
/// `run_photon_promote` even when build/tests/artifact are green.
/// Failing means the case appears in the dedup log → CaseRecord leaked.
#[test]
fn cb_002_unsafe_full_pass_is_skipped_by_run_photon_promote() {
    let tmp = TempDir::new().unwrap();
    let mut c = make_case(&unique_case_id(82));
    c.outcome_score.unsafe_actions_blocked = 2;
    write_case(tmp.path(), &c);

    run_photon_promote(
        tmp.path(),
        tmp.path(),
        None,
        None,
        true, // --all (CB-003 selector exactly-one)
        false,
        true,
        false,
        None,
    )
    .unwrap();

    let promoted = read_promote_log(tmp.path()).unwrap();
    assert!(
        !promoted.contains(&c.case_id),
        "CB-002: case with unsafe_actions_blocked > 0 must NOT appear in dedup log as promoted; got: {promoted:?}"
    );
}

// ---------------------------------------------------------------------------
// CB-003 (codex review fix): photon-promote selector exactly-one + --session
// unsupported.
// ---------------------------------------------------------------------------

/// CB-003: zero selectors is rejected (previously interpreted as
/// "promote everything").
#[test]
fn cb_003_no_selector_rejected() {
    let tmp = TempDir::new().unwrap();
    let r = run_photon_promote(
        tmp.path(),
        tmp.path(),
        None,
        None,
        false,
        true,
        true,
        false,
        None,
    );
    assert!(r.is_err(), "no selector must return Err; got {r:?}");
    let msg = r.unwrap_err();
    assert!(
        msg.contains("exactly one"),
        "error must mention `exactly one` selector requirement; got {msg:?}"
    );
}

/// CB-003: `--session` is explicitly unsupported in Phase A.
#[test]
fn cb_003_session_selector_rejected_as_unsupported() {
    let tmp = TempDir::new().unwrap();
    let r = run_photon_promote(
        tmp.path(),
        tmp.path(),
        Some("sess-x".to_string()),
        None,
        false,
        true,
        true,
        false,
        None,
    );
    assert!(r.is_err(), "--session selector must return Err; got {r:?}");
    let msg = r.unwrap_err();
    assert!(
        msg.contains("--session"),
        "error must reference --session; got {msg:?}"
    );
    assert!(
        msg.contains("not yet supported")
            || msg.contains("unsupported")
            || msg.contains("not supported"),
        "error must say unsupported; got {msg:?}"
    );
}

/// CB-003: `--case-id` only is accepted.
#[test]
fn cb_003_case_id_selector_accepted() {
    let tmp = TempDir::new().unwrap();
    let c = make_case(&unique_case_id(83));
    write_case(tmp.path(), &c);
    let r = run_photon_promote(
        tmp.path(),
        tmp.path(),
        None,
        Some(c.case_id.clone()),
        false,
        true,
        false,
        false,
        None,
    );
    assert!(r.is_ok(), "--case-id alone must be accepted; got {r:?}");
}

/// CB-003: `--all` only is accepted.
#[test]
fn cb_003_all_selector_accepted() {
    let tmp = TempDir::new().unwrap();
    let r = run_photon_promote(
        tmp.path(),
        tmp.path(),
        None,
        None,
        true,
        true,
        false,
        false,
        None,
    );
    assert!(r.is_ok(), "--all alone must be accepted; got {r:?}");
}
