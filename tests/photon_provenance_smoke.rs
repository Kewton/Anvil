//! Issue #594 — photon seed provenance smoke suite.
//!
//! Covers the read path of seed provenance through `pub` surfaces only:
//!   * `extract_seed_provenance` / `sanitize_provenance_value` pure-function
//!     boundary cases
//!   * `/photon-why` slash command output across the 7
//!     `PhotonContextPackStatus` variants (`build_photon_why_message`)
//!
//! All tests are Ollama-free.
//!
//! CB-003 (Issue #594 review): PV-01 / PV-02 / PV-09 / PV-15 — which used
//! to drive `enumerate_admitted_items_with_provenance` directly — were
//! migrated to module unit tests in `src/photon/prompt.rs` when that
//! function and `AdmittedItemView` were reduced to `pub(crate)` visibility.

use anvil::agent::loop_run::{PhotonContextPackStatus, build_photon_why_message};
use anvil::photon::provenance::{
    MAX_CORRELATED_CASE_IDS, MAX_PROVENANCE_SOURCE_ID_BYTES, SeedProvenanceSummary,
    extract_seed_provenance, sanitize_provenance_value,
};
use serde_json::json;

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn placeholder_provenance(summary_id: Option<&str>) -> SeedProvenanceSummary {
    extract_seed_provenance(summary_id, None)
}

// ---------------------------------------------------------------------------
// PV-03: /photon-why N=2 plural display
// ---------------------------------------------------------------------------
#[test]
fn pv_03_photon_why_n2_plural() {
    let prov = vec![
        SeedProvenanceSummary {
            summary_id: Some("seed_a".to_string()),
            source: "anvil_case_record",
            trust_tier: Some("auto_extracted"),
            provenance_status: "present",
            ..SeedProvenanceSummary::default()
        },
        SeedProvenanceSummary {
            summary_id: Some("seed_b".to_string()),
            source: "human_handcrafted",
            trust_tier: Some("human_reviewed"),
            provenance_status: "present",
            ..SeedProvenanceSummary::default()
        },
    ];
    let msg = build_photon_why_message(PhotonContextPackStatus::Injected, &prov);
    assert!(msg.starts_with("Last turn injected 2 seeds:"));
    assert!(msg.contains("seed_a"));
    assert!(msg.contains("seed_b"));
    assert!(msg.contains("anvil_case_record"));
    assert!(msg.contains("human_handcrafted"));
}

// ---------------------------------------------------------------------------
// PV-04: /photon-why N=0 in Act + photon enabled
// ---------------------------------------------------------------------------
#[test]
fn pv_04_photon_why_n0_no_injection() {
    let msg = build_photon_why_message(PhotonContextPackStatus::NoInjection, &[]);
    assert_eq!(msg, "No seeds were injected in the last turn.");
}

// ---------------------------------------------------------------------------
// PV-05: /photon-why N=1 singular
// ---------------------------------------------------------------------------
#[test]
fn pv_05_photon_why_n1_singular() {
    let prov = vec![SeedProvenanceSummary {
        summary_id: Some("seed_a".to_string()),
        source: "user_slash_command",
        trust_tier: Some("experimental"),
        provenance_status: "present",
        ..SeedProvenanceSummary::default()
    }];
    let msg = build_photon_why_message(PhotonContextPackStatus::Injected, &prov);
    assert!(msg.starts_with("Last turn injected 1 seed:"));
    assert!(msg.contains("seed_a"));
}

// ---------------------------------------------------------------------------
// PV-06: source_id with secret-like content is masked / dropped
// ---------------------------------------------------------------------------
#[test]
fn pv_06_source_id_with_secret_dropped() {
    let prov = json!({
        "source": "anvil_case_record",
        // Format that triggers mask_secrets (e.g. token=<hex>).
        "source_id": "token=abcdef1234567890abcdef1234567890",
    });
    let summary = extract_seed_provenance(None, Some(&prov));
    assert!(
        summary.source_id.is_none(),
        "source_id with token= must be dropped"
    );
    assert_eq!(summary.provenance_status, "sanitized");
}

// ---------------------------------------------------------------------------
// PV-07: source_id over MAX_PROVENANCE_SOURCE_ID_BYTES is dropped
// ---------------------------------------------------------------------------
#[test]
fn pv_07_source_id_over_byte_cap_dropped() {
    let huge = "x".repeat(MAX_PROVENANCE_SOURCE_ID_BYTES + 1);
    let prov = json!({
        "source": "anvil_case_record",
        "source_id": huge,
    });
    let summary = extract_seed_provenance(None, Some(&prov));
    assert!(summary.source_id.is_none());
    assert_eq!(summary.provenance_status, "sanitized");
}

// ---------------------------------------------------------------------------
// PV-08: unknown source value + bidi-containing field
// ---------------------------------------------------------------------------
#[test]
fn pv_08_unknown_source_and_bidi_field() {
    let prov = json!({
        "source": "future_kind",
        // U+200B = ZERO WIDTH SPACE (bidi-class char in is_bidi_control).
        "source_repo": "github.com/foo/\u{200b}bar",
    });
    let summary = extract_seed_provenance(None, Some(&prov));
    assert_eq!(summary.source, "unknown");
    assert!(summary.source_repo.is_none(), "bidi field must be dropped");
    assert_eq!(summary.provenance_status, "sanitized");
}

// PV-09 (dual layout v0.2 vs legacy) was migrated to a module unit test in
// `src/photon/prompt.rs` under CB-003 (visibility tightening).

// ---------------------------------------------------------------------------
// PV-10: /photon-why shadow_mode
// ---------------------------------------------------------------------------
#[test]
fn pv_10_photon_why_shadow_mode() {
    let msg = build_photon_why_message(PhotonContextPackStatus::ShadowMode, &[]);
    assert_eq!(msg, "Shadow mode active; provenance not surfaced.");
}

// ---------------------------------------------------------------------------
// PV-11: /photon-why canary=0
// ---------------------------------------------------------------------------
#[test]
fn pv_11_photon_why_canary_zero() {
    let msg = build_photon_why_message(PhotonContextPackStatus::CanarySkipped, &[]);
    assert_eq!(msg, "Photon disabled (canary=0); no seeds were considered.");
}

// ---------------------------------------------------------------------------
// PV-12: /photon-why Plan mode
// ---------------------------------------------------------------------------
#[test]
fn pv_12_photon_why_plan_mode() {
    let msg = build_photon_why_message(PhotonContextPackStatus::PlanMode, &[]);
    assert_eq!(msg, "Plan mode; photon context not consulted.");
}

// ---------------------------------------------------------------------------
// PV-13: provenance object over MAX_PROVENANCE_OBJECT_BYTES → oversized
// ---------------------------------------------------------------------------
#[test]
fn pv_13_oversized_provenance_object() {
    let huge_repo = "x".repeat(5000);
    let prov = json!({
        "source": "anvil_case_record",
        "source_repo": huge_repo,
    });
    let outcome = sanitize_provenance_value(Some(&prov));
    assert_eq!(outcome.status, "oversized");
    assert!(outcome.value.is_none());
    // extract_seed_provenance must still produce a placeholder row.
    let summary = extract_seed_provenance(Some("seed"), Some(&prov));
    assert_eq!(summary.source, "unknown");
    assert_eq!(summary.provenance_status, "oversized");
}

// ---------------------------------------------------------------------------
// PV-14: correlated_case_ids over the 8-id cap → truncate + sanitized
// ---------------------------------------------------------------------------
#[test]
fn pv_14_correlated_case_ids_capped() {
    let ids: Vec<String> = (0..33).map(|i| format!("case_{i:032}")).collect();
    let prov = json!({
        "source": "anvil_case_record",
        "correlated_case_ids": ids,
    });
    let summary = extract_seed_provenance(None, Some(&prov));
    assert_eq!(summary.correlated_case_ids.len(), MAX_CORRELATED_CASE_IDS);
    assert_eq!(summary.provenance_status, "sanitized");
}

// PV-15 (adopted == views invariant across cap pipeline) was migrated to a
// module unit test in `src/photon/prompt.rs` under CB-003.

// ---------------------------------------------------------------------------
// PV-16: /photon-why before any turn has executed → NoTurn variant
// ---------------------------------------------------------------------------
#[test]
fn pv_16_photon_why_no_turn() {
    let msg = build_photon_why_message(PhotonContextPackStatus::NoTurn, &[]);
    assert_eq!(msg, "No photon turn has been executed yet.");
}

// ---------------------------------------------------------------------------
// PV-17: /photon-why after a Failed fetch
// ---------------------------------------------------------------------------
#[test]
fn pv_17_photon_why_failed_fetch() {
    let msg = build_photon_why_message(PhotonContextPackStatus::Failed, &[]);
    assert_eq!(msg, "Photon context_pack fetch failed in the last turn.");
}

// ---------------------------------------------------------------------------
// Extra: placeholder_provenance helper sanity check (regression for the
// invariant maintenance pattern).
// ---------------------------------------------------------------------------
#[test]
fn placeholder_helper_returns_missing_status() {
    let row = placeholder_provenance(Some("foo"));
    assert_eq!(row.source, "unknown");
    assert_eq!(row.provenance_status, "missing");
    assert_eq!(row.summary_id.as_deref(), Some("foo"));
}
