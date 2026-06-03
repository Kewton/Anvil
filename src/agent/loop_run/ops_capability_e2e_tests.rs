//! Issue #923 (P6): Ops capability E2E — the OpsRunbook obligation bridge and
//! the tiered `ops_runbook_pass` exercised through the **production**
//! `verifier_diagnostic_for_obligation` path (not the test-only
//! `artifact_evidence` adapter). CB-001 pattern: in-crate `#[cfg(test)]` so the
//! production binary excludes the module.
//!
//! These tests prove the design-review findings end-to-end:
//! - DR3-001: an ops request now produces an `OpsRunbook` obligation (without
//!   the bridge, loosening `ops_runbook_pass` would be a production no-op).
//! - DR3-002: the obligation uses `kind = OpsRunbook` + `schema = None` so it is
//!   not hijacked by the docs `RequiredSections` gate / observed-role shortcut.
//! - DR3-003: a pure setup/install request (Ops via `asks_for_setup`) gets no
//!   OpsRunbook obligation.
//! - Behavior change: a rollback-omitted 3/4 runbook fails under the old 4-way
//!   AND but completes under the new tier; an *explicitly* requested rollback
//!   stays mandatory.

use super::task_contract::{
    ArtifactObligation, ArtifactRole, DeliverableKind, TaskContract, TaskKind,
};
use super::verifier::verifier_diagnostic_for_obligation;

fn ops_runbook_obligation(contract: &TaskContract) -> Option<&ArtifactObligation> {
    contract
        .required_artifact_identities
        .iter()
        .find(|identity| identity.kind == DeliverableKind::OpsRunbook)
}

#[test]
fn ops_request_creates_opsrunbook_obligation() {
    let contract =
        TaskContract::from_request("Prepare a deployment runbook checklist with rollback steps");
    assert_eq!(contract.task_kind, TaskKind::Ops);
    let obligation = ops_runbook_obligation(&contract)
        .expect("ops runbook request must produce an OpsRunbook obligation (DR3-001 bridge)");
    assert_eq!(obligation.role, ArtifactRole::UsageDocs);
    assert_eq!(obligation.kind, DeliverableKind::OpsRunbook);
    assert_eq!(obligation.path, "runbook.md");
    // schema = None so the docs RequiredSections gate cannot hijack it (DR3-002).
    assert!(obligation.schema.is_none());
    // Explicit rollback request is recorded; only canonical labels are stored
    // (DR4-002).
    assert!(obligation.required_sections.iter().any(|s| s == "rollback"));
    assert!(
        obligation
            .required_sections
            .iter()
            .all(|s| matches!(s.as_str(), "checklist" | "validation" | "rollback" | "risk"))
    );
}

#[test]
fn pure_setup_request_does_not_create_opsrunbook_obligation() {
    // Reaches TaskKind::Ops via `asks_for_setup`, not ops keywords → no
    // OpsRunbook obligation; setup completion stays on the Setup /
    // SetupBootstrap path (DR3-003).
    let contract =
        TaskContract::from_request("Install project dependencies and set up the environment");
    assert_eq!(contract.task_kind, TaskKind::Ops);
    assert!(
        ops_runbook_obligation(&contract).is_none(),
        "pure setup/install must not get an OpsRunbook obligation"
    );
}

#[test]
fn loosened_predicate_completes_rollback_omitted_runbook() {
    // Request does NOT explicitly require rollback. Under the old 4-way AND a
    // rollback-omitted runbook FAILED; under the new tier (core + >=3/4) it
    // completes — production behavior change proved through the obligation path.
    let contract = TaskContract::from_request("Prepare a deployment runbook checklist");
    let obligation = ops_runbook_obligation(&contract).expect("ops obligation");
    let excerpt =
        "## Checklist\n[x] deploy\n## Validation\nVerify health endpoint.\n## Risk\nImpact low.";
    let diagnostic =
        verifier_diagnostic_for_obligation(TaskKind::Ops, obligation, Some(excerpt), true);
    assert!(
        diagnostic.is_none(),
        "rollback-omitted 3/4 runbook should complete under the new tier, got {diagnostic:?}"
    );
}

#[test]
fn explicit_rollback_request_rejects_rollback_omitted_runbook() {
    // Request explicitly asks for rollback → rollback is mandatory; an otherwise
    // 3/4 runbook that omits it must FAIL (Codex S5-001 / S7-002).
    let contract = TaskContract::from_request("Prepare a deployment runbook with rollback steps");
    let obligation = ops_runbook_obligation(&contract).expect("ops obligation");
    assert!(obligation.required_sections.iter().any(|s| s == "rollback"));
    let rollback_omitted =
        "## Checklist\n[x] deploy\n## Validation\nVerify health.\n## Risk\nImpact low.";
    let diagnostic =
        verifier_diagnostic_for_obligation(TaskKind::Ops, obligation, Some(rollback_omitted), true);
    assert!(
        diagnostic.is_some(),
        "an explicitly required rollback must not be droppable by the tier"
    );
}

#[test]
fn core_incomplete_runbook_fails_through_obligation_path() {
    let contract = TaskContract::from_request("Prepare a deployment runbook checklist");
    let obligation = ops_runbook_obligation(&contract).expect("ops obligation");
    // Validation core missing (only checklist + risk).
    let excerpt = "## Checklist\n[ ] deploy\n## Risk\nlow";
    let diagnostic =
        verifier_diagnostic_for_obligation(TaskKind::Ops, obligation, Some(excerpt), true);
    assert!(
        diagnostic.is_some(),
        "a core-incomplete runbook must still fail (core floor)"
    );
}

#[test]
fn japanese_runbook_excerpt_completes_through_obligation_path() {
    // Bilingual parity is about the *excerpt* keywords (手順/検証/リスク...), which
    // is what #923 owns. JP request *classification* precedence (Docs vs Ops) is
    // #917/#919 territory, so we obtain the obligation from a reliably-Ops ASCII
    // request and verify a JP runbook excerpt is accepted through the production
    // path.
    let contract = TaskContract::from_request("Prepare a deployment runbook checklist");
    let obligation = ops_runbook_obligation(&contract).expect("ops obligation");
    let jp_excerpt = "## 手順\n[x] デプロイ\n## 検証\nヘルスチェック確認。\n## リスク\n影響は小。";
    let diagnostic =
        verifier_diagnostic_for_obligation(TaskKind::Ops, obligation, Some(jp_excerpt), true);
    assert!(
        diagnostic.is_none(),
        "JP 3/4 runbook excerpt should complete under the new tier, got {diagnostic:?}"
    );
}
