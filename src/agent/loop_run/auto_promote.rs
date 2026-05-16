//! Post-loop auto-promote hook for photon knowledge base growth (Issue #604).
//!
//! Layer rule (CLAUDE.md DR3-002): agent → session → photon の単方向依存。
//! 本モジュールは agent 層にあり、session 層 (`case_record`,
//! `case_photon_bridge`, `auto_promote_scrub`) と photon 層
//! (`PhotonClient`, `action_memory_v2_adapter`) の双方を import する
//! agent-level entry point となる。
//!
//! Phase 2 (Task 2.1〜2.3) で純関数と型を、Phase 5 (Task 5.1) で
//! `invoke_photon_auto_promote` hook を実装する。caller の配線は
//! Task 5.2 (`turn.rs::run_actor_loop` post-loop section) で行う。

use std::collections::HashSet;
use std::path::Path;
use std::time::Instant;

use serde_json::json;
use sha2::{Digest, Sha256};

use crate::logging::log_llm_event;
use crate::photon::PhotonClient;
use crate::photon::action_memory_v2_adapter::to_v2_upsert_summary;
use crate::photon::prompt::sanitize_summary_id;
use crate::photon::schema::PhotonUpsertError;
use crate::session::auto_promote_scrub::{ScrubAction, ScrubMode, scrub_action_summary};
use crate::session::case_photon_bridge::{
    self, ACTION_SUMMARY_SCHEMA_VERSION, MAX_ACTION_SUMMARY_BYTES, PromoteLogEntry, SkipReason,
    append_promote_log_entry, convert_case_to_action_summary, format_rfc3339_utc, read_promote_log,
};
use crate::session::case_record::CaseRecord;

// ---------------------------------------------------------------------------
// Configuration carrier (DR1-015): agent 層が string→ScrubMode を変換した上で
// hook に渡す。`Config` 自体は scrub_mode を `String` で保持し、本構造体への
// 詰め替え時に `ScrubMode::from_env_str_or_default` を通す。
// ---------------------------------------------------------------------------

/// Hook 単位の取り扱い済 config snapshot。
///
/// `Config` から直接取り出すと scrub_mode の型が string のままになるため、
/// hook entry (`invoke_photon_auto_promote`) は本構造体に詰め直してから
/// `PostLoopInputs` に渡す。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AutoPromoteConfig {
    /// `ANVIL_PHOTON_AUTO_PROMOTE` (default `true`).
    pub enabled: bool,
    /// `ANVIL_PHOTON_NO_AUTO_PROMOTE` (default `false`). `true` で強制 disable。
    pub force_disabled: bool,
    /// `ANVIL_PHOTON_AUTO_PROMOTE_DRY_RUN` (default `true`, Phase 1 rollout)。
    pub dry_run: bool,
    /// `ANVIL_PHOTON_AUTO_PROMOTE_SCRUB_MODE` (default `Strict`)。
    pub scrub_mode: ScrubMode,
}

// ---------------------------------------------------------------------------
// PostLoopInputs (Task 2.2 が依存する純関数引数 carrier)
//
// 設計方針書 §4.1 の `PostLoopInputs<'a>` を Phase 2 用に簡素化したバージョン。
// 主な差分:
//   * `interrupt_flag: bool` に縮退 (caller が `InterruptFlag::is_set()` を
//     解決済の bool で渡す)。`InterruptFlag` 自体は `pub(super)` 可視で agent
//     層内部のため、純関数を本モジュールに閉じ込めるためには本縮退が必要。
//   * `mode`/`snapshot`/`config` を flat な primitive に展開 (純関数の
//     再利用性を上げ、test fixture を構築しやすくする)。
//
// Task 5.1 で `invoke_photon_auto_promote` を実装する際に caller 側で
// `InterruptFlag::is_set()` / `SessionSnapshot.auto_promote_called_this_turn`
// / `AutoPromoteConfig.*` を読み取ってここに詰める。
// ---------------------------------------------------------------------------

/// Phase A `should_auto_promote` の入力。turn.rs から束ねて渡す。
///
/// Lifetime `'a` は `extracted_case` (`Option<CaseRecord>`) と
/// `already_promoted_ids` (`&HashSet<String>`) を borrow するために必要。
pub struct PostLoopInputs<'a> {
    /// 現在の実行モード。`Plan` の場合は `PlanMode` 経路で skip。
    pub plan_mode: bool,
    /// `Config.photon_auto_promote` (= `AutoPromoteConfig.enabled`)。
    pub photon_enabled: bool,
    /// `Agent.photon.is_some()` の解決済 bool。`false` の場合は
    /// `Disabled { sub_reason: "photon_disabled" }` で最上位 hard disable
    /// (DR2-012 / S7-004 #1)。
    pub photon_client_available: bool,
    /// `Config.photon_no_auto_promote` (= `AutoPromoteConfig.force_disabled`)。
    pub no_auto_promote_env: bool,
    /// `Config.photon_auto_promote_dry_run` (= `AutoPromoteConfig.dry_run`)。
    /// Phase A 自体では参照しない (DR1-004: `Promote` 経路の中で hook 側が
    /// `AutoPromoteConfig.dry_run` を直接参照する) が、`PostLoopInputs` を
    /// agent / test fixture 双方から組み立て可能にする carrier として保持。
    #[allow(dead_code)] // carrier field only; consulted via AutoPromoteConfig.dry_run.
    pub dry_run: bool,
    /// `SessionSnapshot.auto_promote_called_this_turn` の解決済 bool。
    pub auto_promote_called_this_turn: bool,
    /// `SessionSnapshot.case_record_extracted_this_turn` の解決済 bool。
    /// `false` の場合は `NotEligible` で skip。
    pub case_record_extracted_this_turn: bool,
    /// `maybe_extract_case_record` の戻り値 (DR1-003 / DR2-008)。
    /// `None` で `NotEligible` skip。
    pub extracted_case: Option<CaseRecord>,
    /// CLI #598 と共有する `state_root/photon-promote-log.jsonl` 由来の
    /// promoted case_id set (caller が `read_promote_log` で取得)。
    pub already_promoted_ids: &'a HashSet<String>,
    /// caller の `SystemTime::now()` から導出した unix sec。
    /// `check_quality_gate` 第 3 引数 (DR1-001)。
    pub now_unix: u64,
    /// `InterruptFlag::is_set()` の解決済 bool (DR2-009 / DR2-010)。
    /// `true` で `Interrupted` skip + event emit (flag は立てない)。
    pub interrupt_flag: bool,
    /// `ScrubMode` (DR1-015): agent 層で string → enum 変換済。Phase A 自体は
    /// 参照しない (scrub は `execute_promote_pipeline` で `AutoPromoteConfig.scrub_mode`
    /// 経由) が、carrier として含めておく。
    #[allow(dead_code)] // carrier field only; consulted via AutoPromoteConfig.scrub_mode.
    pub scrub_mode: ScrubMode,
}

// ---------------------------------------------------------------------------
// Decision (Phase A 純関数戻り値)
// ---------------------------------------------------------------------------

/// Phase A `should_auto_promote` の戻り値。`Promote` は `CaseRecord` を所有して
/// caller の Phase B 処理 (scrub / serialize / HTTP) に引き渡す。
///
/// `CaseRecord` は約 392 byte と他 variant より大きく `clippy::large_enum_variant`
/// に該当するため `Box` で indirection する (`src/agent/skills/mod.rs` precedent
/// と同形)。`PartialEq`/`Eq` は内包する `CaseRecord` が `Eq` を実装していない
/// ため derive しない (テストは `match` で variant 判定する)。
#[derive(Debug, Clone)]
pub enum AutoPromoteDecision {
    Promote(Box<CaseRecord>),
    Skip(AutoPromoteSkipReason),
}

/// Skip 理由。設計方針書 §4.1 / DR2-010〜DR2-012 の Issue §AP-06 / S3-018
/// reason 7 種に整合する。
///
/// `event_str()` で `agent.photon_auto_promote.skipped` event の `skip_reason`
/// payload key に詰める文字列 SSOT を返し、`event_sub_str()` で
/// `skip_sub_reason` 用の補助文字列を返す。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AutoPromoteSkipReason {
    /// Plan mode 中は seed 投入させない。
    PlanMode,
    /// DR2-012: `sub_reason="photon_disabled"` は photon サイドカー unavailable
    /// 経路 (`Agent.photon.is_none()`)。それ以外 (env disable / force_disabled)
    /// は `sub_reason=None`。
    Disabled { sub_reason: Option<&'static str> },
    /// 同 turn 内で既に hook が走り終えていた (DR1-004 per-turn cap=1)。
    PerTurnCapConsumed,
    /// DR2-010: interrupt 検出。flag は **立てない**ことで次 turn 再試行を
    /// 残しつつ event emit のみ行う (Issue §AP-09 / VR-15)。
    Interrupted,
    /// DR2-011: Phase 1 rollout 中の HTTP skip 経路。`Promote(_)` 判定後に
    /// hook 内で `config.dry_run` を参照して emit する (event は `succeeded`
    /// で emit するため、本 variant 自体は決定経路に使わない / Issue §S7-004 #3)。
    #[allow(dead_code)]
    // dry_run is encoded as `decision="promoted"` with `dry_run=true` payload.
    DryRun,
    /// DR2-011: strict-mode scrub で `ScrubAction::Hit{fields}` を検出した
    /// 経路 (rename: Issue §AP-06 reason="answer_leak")。
    AnswerLeak { fields_detected: Vec<String> },
    /// DR2-003: agent 側 `sanitize_summary_id` 1 段目が `None` を返した経路。
    /// event payload では `skip_reason="quality_gate"` /
    /// `skip_sub_reason="invalid_summary_id"` で表現する。
    InvalidSummaryId,
    /// case 未抽出 (`case_record_extracted_this_turn=false` または
    /// `extracted_case=None`)。
    NotEligible,
    /// `check_quality_gate` SSOT (session::case_photon_bridge) の戻り値を内包。
    QualityGate(SkipReason),
}

impl AutoPromoteSkipReason {
    /// `agent.photon_auto_promote.skipped` event の `skip_reason` payload key
    /// 用の短い文字列 SSOT (DR1-007 / DR2-011)。Issue §AP-06 / S3-018 の
    /// reason 7 種に整合する snake_case。`is_secret_like_key=false` を満たす。
    ///
    /// `QualityGate(_)` / `InvalidSummaryId` / `NotEligible` は一律
    /// `"quality_gate"` に折り畳み、詳細 reason は `event_sub_str()` 経由で
    /// `skip_sub_reason` payload key に併設する。
    pub fn event_str(&self) -> &'static str {
        match self {
            Self::PlanMode => "plan_mode",
            Self::Disabled { .. } => "disabled",
            Self::PerTurnCapConsumed => "per_turn_cap_consumed",
            Self::Interrupted => "interrupted",
            Self::DryRun => "dry_run",
            Self::AnswerLeak { .. } => "answer_leak",
            Self::InvalidSummaryId => "quality_gate",
            Self::NotEligible => "quality_gate",
            Self::QualityGate(_) => "quality_gate",
        }
    }

    /// `skip_sub_reason` payload key 用。`QualityGate` は SSOT
    /// (`SkipReason::as_str`) を必ず通す (DR1-007)。`Disabled` は
    /// `sub_reason="photon_disabled"` を返せる (DR2-012)。`InvalidSummaryId` は
    /// `"invalid_summary_id"` (DR2-003)、`NotEligible` は `"not_eligible"`。
    pub fn event_sub_str(&self) -> Option<&'static str> {
        match self {
            Self::QualityGate(inner) => Some(inner.as_str()),
            Self::Disabled { sub_reason } => *sub_reason,
            Self::InvalidSummaryId => Some("invalid_summary_id"),
            Self::NotEligible => Some("not_eligible"),
            _ => None,
        }
    }
}

// ---------------------------------------------------------------------------
// EvalRecord 用 outcome summary (Task 4.1: session 層に移設済、DR3-002 layer
// 規約 — agent → session 方向の単方向依存を維持するため、本モジュールは
// `pub use` で再エクスポートして call site から `auto_promote::` 経由で
// 参照できるようにする)。
// ---------------------------------------------------------------------------

pub use crate::session::eval_log::AutoPromoteOutcomeSummary;

// ---------------------------------------------------------------------------
// Phase A: should_auto_promote (純関数 / Task 2.2)
//
// Gate 優先度 (Issue §S7-004 + DR2-010 / DR2-012):
//   1. photon_client_available=false → Skip(Disabled{photon_disabled})
//   2. config.force_disabled / !config.enabled → Skip(Disabled{None})
//   3. PlanMode
//   4. PerTurnCapConsumed
//   5. Interrupted (DR2-010 復活: event emit のみ、flag は立てない)
//   6. NotEligible (case_record_extracted_this_turn=false または
//      extracted_case=None)
//   7. QualityGate (check_quality_gate SSOT)
//
// Oversize / Scrub / sanitize_summary_id / dry_run は Phase B (Task 5.1)
// 側に分離する。
// ---------------------------------------------------------------------------

/// Phase A 判定 (HTTP / event emit / serialize は含まない / DR1-004)。
pub fn should_auto_promote(inputs: &PostLoopInputs<'_>) -> AutoPromoteDecision {
    // 1) photon サイドカー unavailable は最上位 hard disable。
    if !inputs.photon_client_available {
        return AutoPromoteDecision::Skip(AutoPromoteSkipReason::Disabled {
            sub_reason: Some("photon_disabled"),
        });
    }
    // 2) env 由来 disable (force_disabled or auto_promote=false)。
    if inputs.no_auto_promote_env || !inputs.photon_enabled {
        return AutoPromoteDecision::Skip(AutoPromoteSkipReason::Disabled { sub_reason: None });
    }
    // 3) Plan mode 中は skip。
    if inputs.plan_mode {
        return AutoPromoteDecision::Skip(AutoPromoteSkipReason::PlanMode);
    }
    // 4) 同 turn 内で既に hook 完走済。
    if inputs.auto_promote_called_this_turn {
        return AutoPromoteDecision::Skip(AutoPromoteSkipReason::PerTurnCapConsumed);
    }
    // 5) interrupt 検出: flag は立てずに skip 経路に流す (DR2-010)。
    if inputs.interrupt_flag {
        return AutoPromoteDecision::Skip(AutoPromoteSkipReason::Interrupted);
    }
    // 6) case 未抽出。
    if !inputs.case_record_extracted_this_turn {
        return AutoPromoteDecision::Skip(AutoPromoteSkipReason::NotEligible);
    }
    let Some(case) = inputs.extracted_case.as_ref() else {
        return AutoPromoteDecision::Skip(AutoPromoteSkipReason::NotEligible);
    };

    // 7) Quality gate (session 層 SSOT)。
    match case_photon_bridge::check_quality_gate(case, inputs.already_promoted_ids, inputs.now_unix)
    {
        Ok(()) => AutoPromoteDecision::Promote(Box::new(case.clone())),
        Err(reason) => AutoPromoteDecision::Skip(AutoPromoteSkipReason::QualityGate(reason)),
    }
}

// ---------------------------------------------------------------------------
// Phase B: decide_oversize (純関数 / Task 2.3)
//
// `serde_json::to_vec(&summary_v2)?.len()` 後の byte 数判定。SSOT は
// `session::case_photon_bridge::MAX_ACTION_SUMMARY_BYTES` を流用する
// (新規 const を導入しない / DR1-004)。
// ---------------------------------------------------------------------------

/// `MAX_ACTION_SUMMARY_BYTES` を超えた場合に
/// `Some(QualityGate(Oversize))` を返す。境界 (==MAX) は許容して `None`。
pub fn decide_oversize(serialized_bytes: usize) -> Option<AutoPromoteSkipReason> {
    if serialized_bytes > MAX_ACTION_SUMMARY_BYTES {
        Some(AutoPromoteSkipReason::QualityGate(SkipReason::Oversize))
    } else {
        None
    }
}

// ---------------------------------------------------------------------------
// request_id SSOT (Task 4.2 / DR2-016 / Issue S3-014)
//
// sha256(session_id || NUL || turn_index.to_le_bytes() || NUL || case_id)[..8]
// → 16 hex char。`uuid` 等の追加依存は導入しない。
// ---------------------------------------------------------------------------

pub(crate) fn build_request_id(session_id: &str, turn_index: u64, case_id: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(session_id.as_bytes());
    hasher.update(b"\x00");
    hasher.update(turn_index.to_le_bytes());
    hasher.update(b"\x00");
    hasher.update(case_id.as_bytes());
    let digest = hasher.finalize();
    let hex = format!("{digest:x}");
    hex[..16].to_string()
}

// ---------------------------------------------------------------------------
// invoke_photon_auto_promote hook (Task 5.1)
//
// 設計方針書 §4.1 / §5.2 / §7 (event 5 種) / §11 (request_id) / §12 (fail-open)
//
// 主要ステップ:
//   1. PostLoopInputs 組み立て (read_promote_log / scrub_mode / etc.)
//   2. should_auto_promote (Phase A: 7-gate 判定)
//   3. Promote(case) → scrub → sanitize_summary_id → adapter → decide_oversize
//      → request_id → upsert_action_summary
//   4. event emit + Agent.last_auto_promote_outcome 更新
//   5. AutoPromoteOutcomeSummary を return
//
// 全パスで fail-open: panic / `?` で agent loop を中断しない。
// ---------------------------------------------------------------------------

/// `agent.photon_auto_promote.*` event 5 種の suffix SSOT。`event_str()` の
/// `Skip` 経路 7 種とは別レイヤ (decision 軸 = succeeded/skipped/scrubbed/
/// failed/rejected_by_photon)。
const EVENT_SUCCEEDED: &str = "agent.photon_auto_promote.succeeded";
const EVENT_SKIPPED: &str = "agent.photon_auto_promote.skipped";
const EVENT_SCRUBBED: &str = "agent.photon_auto_promote.scrubbed";
const EVENT_FAILED: &str = "agent.photon_auto_promote.failed";
const EVENT_REJECTED: &str = "agent.photon_auto_promote.rejected_by_photon";

/// Post-loop auto-promote hook entry. Returns a fail-open
/// `AutoPromoteOutcomeSummary` that the caller (Task 5.2 `run_actor_loop`)
/// must persist into `agent.last_auto_promote_outcome` and pass through to
/// `build_eval_record`.
///
/// **Borrow design**: this function is intentionally **not** a method on
/// `Agent`. The caller is `run_actor_loop` which already holds `&mut self`
/// and must simultaneously borrow `self.photon`; splitting the borrow is
/// easier at the call site than inside this module. Likewise, the per-turn
/// cap flag (`auto_promote_called_this_turn`) is set by the caller before
/// invoking the hook on non-skip paths (DR2-010 — Interrupted leaves the
/// flag false so the next turn can re-try). The hook itself never mutates
/// the snapshot.
///
/// Fail-open: every error path produces a non-panicking
/// `AutoPromoteOutcomeSummary` so the agent loop can continue normally.
#[allow(clippy::too_many_arguments)]
pub(crate) fn invoke_photon_auto_promote(
    session_id: &str,
    turn_index: u64,
    plan_mode: bool,
    auto_promote_called_this_turn: bool,
    case_record_extracted_this_turn: bool,
    extracted_case: Option<&CaseRecord>,
    state_root: &Path,
    now_unix: u64,
    interrupt_flag: bool,
    photon_client: Option<&PhotonClient>,
    config: &AutoPromoteConfig,
) -> AutoPromoteOutcomeSummary {
    let started = Instant::now();

    // Step 1: assemble PostLoopInputs.
    let already_promoted_ids = match read_promote_log(state_root) {
        Ok(set) => set,
        Err(err) => {
            // fail-open: treat as empty set; warn so operators can debug.
            tracing::warn!("auto-promote: read_promote_log failed (fail-open): {err}");
            HashSet::new()
        }
    };
    let inputs = PostLoopInputs {
        plan_mode,
        photon_enabled: config.enabled,
        photon_client_available: photon_client.is_some(),
        no_auto_promote_env: config.force_disabled,
        dry_run: config.dry_run,
        auto_promote_called_this_turn,
        case_record_extracted_this_turn,
        extracted_case: extracted_case.cloned(),
        already_promoted_ids: &already_promoted_ids,
        now_unix,
        interrupt_flag,
        scrub_mode: config.scrub_mode,
    };

    // Step 2: Phase A — 7-gate decision.
    let decision = should_auto_promote(&inputs);

    match decision {
        AutoPromoteDecision::Skip(reason) => {
            emit_skipped_event(session_id, turn_index, &reason, started);
            AutoPromoteOutcomeSummary {
                decision: "skipped".to_string(),
                skip_reason: Some(reason.event_str().to_string()),
                summary_id: None,
            }
        }
        AutoPromoteDecision::Promote(case_box) => execute_promote_pipeline(
            session_id,
            turn_index,
            *case_box,
            state_root,
            now_unix,
            config,
            photon_client,
            started,
        ),
    }
}

/// Phase B execution pipeline for the `Promote(case)` branch. Splits out of
/// `invoke_photon_auto_promote` purely for readability — every code path
/// still returns a non-panicking `AutoPromoteOutcomeSummary`.
#[allow(clippy::too_many_arguments)]
fn execute_promote_pipeline(
    session_id: &str,
    turn_index: u64,
    case: CaseRecord,
    state_root: &Path,
    now_unix: u64,
    config: &AutoPromoteConfig,
    photon_client: Option<&PhotonClient>,
    started: Instant,
) -> AutoPromoteOutcomeSummary {
    let case_id = case.case_id.clone();
    let extracted_at = format_rfc3339_utc(now_unix);
    let mut action_summary = convert_case_to_action_summary(&case, session_id, &extracted_at);

    // Step a/b: scrub (strict → skip, warn → mutate-in-place and continue).
    let scrub_result = scrub_action_summary(&mut action_summary, config.scrub_mode);
    let mut scrubbed_fields: Vec<String> = Vec::new();
    if let ScrubAction::Hit { fields } = scrub_result {
        match config.scrub_mode {
            ScrubMode::Strict => {
                let reason = AutoPromoteSkipReason::AnswerLeak {
                    fields_detected: fields.clone(),
                };
                emit_skipped_event_with_fields(
                    session_id, turn_index, &case_id, &reason, &fields, started,
                );
                return AutoPromoteOutcomeSummary {
                    decision: "skipped".to_string(),
                    skip_reason: Some(reason.event_str().to_string()),
                    summary_id: None,
                };
            }
            ScrubMode::Warn => {
                // Record fields for the `scrubbed` event later (after upsert).
                scrubbed_fields = fields;
            }
        }
    }

    // Step c: sanitize_summary_id (stage-1 defense; client.rs does stage-2).
    let raw_summary_id = action_summary.summary_id.clone();
    let clean_summary_id = match sanitize_summary_id(&raw_summary_id) {
        Some(id) => id,
        None => {
            let reason = AutoPromoteSkipReason::InvalidSummaryId;
            emit_skipped_event(session_id, turn_index, &reason, started);
            return AutoPromoteOutcomeSummary {
                decision: "skipped".to_string(),
                skip_reason: Some(reason.event_str().to_string()),
                summary_id: None,
            };
        }
    };
    action_summary.summary_id = clean_summary_id.clone();

    // Step d: convert to photon PR #120 nested upsert schema.
    let upsert_body = to_v2_upsert_summary(&action_summary);

    // Step e: Phase B oversize gate (post-serialize byte size).
    let serialized = match serde_json::to_vec(&upsert_body) {
        Ok(b) => b,
        Err(err) => {
            // serde_json failure on a freshly-built Value is essentially
            // impossible; treat as a non-fatal failure for safety.
            emit_failed_event(
                session_id,
                turn_index,
                &case_id,
                &clean_summary_id,
                "serialize_failure",
                Some(&err.to_string()),
                started,
            );
            return AutoPromoteOutcomeSummary {
                decision: "failed".to_string(),
                skip_reason: None,
                summary_id: Some(clean_summary_id),
            };
        }
    };
    if let Some(reason) = decide_oversize(serialized.len()) {
        emit_skipped_event(session_id, turn_index, &reason, started);
        return AutoPromoteOutcomeSummary {
            decision: "skipped".to_string(),
            skip_reason: Some(reason.event_str().to_string()),
            summary_id: Some(clean_summary_id),
        };
    }

    // Step g: build request_id SSOT.
    let request_id = build_request_id(session_id, turn_index, &case_id);

    // Step f: dry_run short-circuit — emit succeeded event without HTTP.
    if config.dry_run {
        emit_succeeded_event(
            session_id,
            turn_index,
            &case_id,
            &clean_summary_id,
            &request_id,
            true, // dry_run
            None,
            started,
        );
        // dry_run intentionally does NOT touch the promote log so a real
        // promotion later can still proceed.
        return AutoPromoteOutcomeSummary {
            decision: "promoted".to_string(),
            skip_reason: None,
            summary_id: Some(clean_summary_id),
        };
    }

    // Step h: HTTP POST (fail-open).
    let client = match photon_client {
        Some(c) => c,
        None => {
            // PostLoopInputs.photon_client_available should have caught this,
            // but be defensive: emit failed event so the per-turn outcome is
            // still recorded.
            emit_failed_event(
                session_id,
                turn_index,
                &case_id,
                &clean_summary_id,
                "photon_client_unavailable",
                None,
                started,
            );
            return AutoPromoteOutcomeSummary {
                decision: "failed".to_string(),
                skip_reason: None,
                summary_id: Some(clean_summary_id),
            };
        }
    };

    let response =
        client.upsert_action_summary(ACTION_SUMMARY_SCHEMA_VERSION, &request_id, upsert_body);

    match response {
        Some(Ok(_resp)) => {
            // Step j: success — append promote log + emit succeeded/scrubbed.
            let log_entry = PromoteLogEntry {
                case_id: case_id.clone(),
                summary_id: clean_summary_id.clone(),
                extracted_at: extracted_at.clone(),
                outcome: "promoted".to_string(),
                reason: None,
            };
            if let Err(err) = append_promote_log_entry(state_root, &log_entry) {
                tracing::warn!("auto-promote: append_promote_log_entry failed (fail-open): {err}",);
            }
            if scrubbed_fields.is_empty() {
                emit_succeeded_event(
                    session_id,
                    turn_index,
                    &case_id,
                    &clean_summary_id,
                    &request_id,
                    false,
                    None,
                    started,
                );
                AutoPromoteOutcomeSummary {
                    decision: "promoted".to_string(),
                    skip_reason: None,
                    summary_id: Some(clean_summary_id),
                }
            } else {
                emit_scrubbed_event(
                    session_id,
                    turn_index,
                    &case_id,
                    &clean_summary_id,
                    &request_id,
                    &scrubbed_fields,
                    started,
                );
                AutoPromoteOutcomeSummary {
                    decision: "scrubbed".to_string(),
                    skip_reason: None,
                    summary_id: Some(clean_summary_id),
                }
            }
        }
        Some(Err(PhotonUpsertError::AnswerLeakDetected(warnings))) => {
            emit_rejected_event(
                session_id,
                turn_index,
                &case_id,
                &clean_summary_id,
                &request_id,
                &warnings,
                started,
            );
            AutoPromoteOutcomeSummary {
                decision: "rejected_by_photon".to_string(),
                skip_reason: None,
                summary_id: Some(clean_summary_id),
            }
        }
        None => {
            emit_failed_event(
                session_id,
                turn_index,
                &case_id,
                &clean_summary_id,
                "http_failure",
                None,
                started,
            );
            AutoPromoteOutcomeSummary {
                decision: "failed".to_string(),
                skip_reason: None,
                summary_id: Some(clean_summary_id),
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Event emit helpers. All payload keys are non-secret (is_secret_like_key=false)
// per DR2-025 / Issue §VR-11. `log_llm_event` applies `mask_payload_inplace`
// internally so any free-text values pass through the masker (DR4-001).
// ---------------------------------------------------------------------------

fn latency_ms(started: Instant) -> f64 {
    started.elapsed().as_secs_f64() * 1000.0
}

fn emit_skipped_event(
    session_id: &str,
    turn_index: u64,
    reason: &AutoPromoteSkipReason,
    started: Instant,
) {
    let mut payload = json!({
        "session_id": session_id,
        "turn_index": turn_index,
        "skip_reason": reason.event_str(),
        "latency_ms": latency_ms(started),
    });
    if let Some(sub) = reason.event_sub_str() {
        payload["skip_sub_reason"] = json!(sub);
    }
    log_llm_event(EVENT_SKIPPED, payload);
}

fn emit_skipped_event_with_fields(
    session_id: &str,
    turn_index: u64,
    case_id: &str,
    reason: &AutoPromoteSkipReason,
    fields: &[String],
    started: Instant,
) {
    let mut payload = json!({
        "session_id": session_id,
        "turn_index": turn_index,
        "case_id": case_id,
        "skip_reason": reason.event_str(),
        "scrubbed_fields": fields,
        "latency_ms": latency_ms(started),
    });
    if let Some(sub) = reason.event_sub_str() {
        payload["skip_sub_reason"] = json!(sub);
    }
    log_llm_event(EVENT_SKIPPED, payload);
}

#[allow(clippy::too_many_arguments)]
fn emit_succeeded_event(
    session_id: &str,
    turn_index: u64,
    case_id: &str,
    summary_id: &str,
    request_id: &str,
    dry_run: bool,
    photon_status: Option<&str>,
    started: Instant,
) {
    let mut payload = json!({
        "session_id": session_id,
        "turn_index": turn_index,
        "case_id": case_id,
        "summary_id": summary_id,
        "request_id": request_id,
        "dry_run": dry_run,
        "latency_ms": latency_ms(started),
    });
    if let Some(s) = photon_status {
        payload["photon_status"] = json!(s);
    }
    log_llm_event(EVENT_SUCCEEDED, payload);
}

#[allow(clippy::too_many_arguments)]
fn emit_scrubbed_event(
    session_id: &str,
    turn_index: u64,
    case_id: &str,
    summary_id: &str,
    request_id: &str,
    scrubbed_fields: &[String],
    started: Instant,
) {
    let payload = json!({
        "session_id": session_id,
        "turn_index": turn_index,
        "case_id": case_id,
        "summary_id": summary_id,
        "request_id": request_id,
        "scrub_status": "fields_removed",
        "scrubbed_fields": scrubbed_fields,
        "latency_ms": latency_ms(started),
    });
    log_llm_event(EVENT_SCRUBBED, payload);
}

#[allow(clippy::too_many_arguments)]
fn emit_failed_event(
    session_id: &str,
    turn_index: u64,
    case_id: &str,
    summary_id: &str,
    fail_reason: &str,
    detail_error: Option<&str>,
    started: Instant,
) {
    let mut payload = json!({
        "session_id": session_id,
        "turn_index": turn_index,
        "case_id": case_id,
        "summary_id": summary_id,
        "fail_reason": fail_reason,
        "latency_ms": latency_ms(started),
    });
    if let Some(err) = detail_error {
        payload["detail_error"] = json!(err);
    }
    log_llm_event(EVENT_FAILED, payload);
}

#[allow(clippy::too_many_arguments)]
fn emit_rejected_event(
    session_id: &str,
    turn_index: u64,
    case_id: &str,
    summary_id: &str,
    request_id: &str,
    quality_warnings: &[String],
    started: Instant,
) {
    let payload = json!({
        "session_id": session_id,
        "turn_index": turn_index,
        "case_id": case_id,
        "summary_id": summary_id,
        "request_id": request_id,
        "fail_reason": "answer_leak_detected",
        "quality_warnings": quality_warnings,
        "latency_ms": latency_ms(started),
    });
    log_llm_event(EVENT_REJECTED, payload);
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use std::collections::HashSet;

    use crate::session::anvil_score::AnvilScore;
    use crate::session::auto_promote_scrub::ScrubMode;
    use crate::session::case_photon_bridge::{MAX_ACTION_SUMMARY_BYTES, SkipReason};
    use crate::session::case_record::{CaseRecord, RepoFingerprint};
    use crate::session::feedback::FeedbackKind;

    use super::{
        AutoPromoteConfig, AutoPromoteDecision, AutoPromoteOutcomeSummary, AutoPromoteSkipReason,
        PostLoopInputs, decide_oversize, should_auto_promote,
    };

    // -- Task 2.1: event_str / event_sub_str SSOT mapping ---------------------

    #[test]
    fn event_str_covers_all_7_issue_reasons() {
        assert_eq!(AutoPromoteSkipReason::PlanMode.event_str(), "plan_mode");
        assert_eq!(
            AutoPromoteSkipReason::Disabled { sub_reason: None }.event_str(),
            "disabled"
        );
        assert_eq!(
            AutoPromoteSkipReason::Disabled {
                sub_reason: Some("photon_disabled")
            }
            .event_str(),
            "disabled"
        );
        assert_eq!(
            AutoPromoteSkipReason::PerTurnCapConsumed.event_str(),
            "per_turn_cap_consumed"
        );
        assert_eq!(
            AutoPromoteSkipReason::Interrupted.event_str(),
            "interrupted"
        );
        assert_eq!(AutoPromoteSkipReason::DryRun.event_str(), "dry_run");
        assert_eq!(
            AutoPromoteSkipReason::AnswerLeak {
                fields_detected: vec!["facts[0]".into()]
            }
            .event_str(),
            "answer_leak"
        );
        // The 3 quality_gate-collapsed variants
        assert_eq!(
            AutoPromoteSkipReason::InvalidSummaryId.event_str(),
            "quality_gate"
        );
        assert_eq!(
            AutoPromoteSkipReason::NotEligible.event_str(),
            "quality_gate"
        );
        assert_eq!(
            AutoPromoteSkipReason::QualityGate(SkipReason::Oversize).event_str(),
            "quality_gate"
        );
    }

    #[test]
    fn event_sub_str_mirrors_session_layer_ssot_for_quality_gate() {
        // QualityGate must round-trip through SkipReason::as_str (session SSOT).
        for sr in [
            SkipReason::AlreadyPromoted,
            SkipReason::NotSuccessState,
            SkipReason::EmptyLanguageStack,
            SkipReason::EmptyTaskSignature,
            SkipReason::StaleCase,
            SkipReason::Oversize,
        ] {
            let r = AutoPromoteSkipReason::QualityGate(sr);
            assert_eq!(r.event_sub_str(), Some(sr.as_str()));
        }
    }

    #[test]
    fn event_sub_str_handles_disabled_and_special_variants() {
        // Disabled{None} returns None; Disabled{Some} returns Some.
        assert_eq!(
            AutoPromoteSkipReason::Disabled { sub_reason: None }.event_sub_str(),
            None
        );
        assert_eq!(
            AutoPromoteSkipReason::Disabled {
                sub_reason: Some("photon_disabled")
            }
            .event_sub_str(),
            Some("photon_disabled")
        );
        // Folded-into-quality_gate variants expose their own sub_str.
        assert_eq!(
            AutoPromoteSkipReason::InvalidSummaryId.event_sub_str(),
            Some("invalid_summary_id")
        );
        assert_eq!(
            AutoPromoteSkipReason::NotEligible.event_sub_str(),
            Some("not_eligible")
        );
        // Simple variants have no sub_str.
        for r in [
            AutoPromoteSkipReason::PlanMode,
            AutoPromoteSkipReason::PerTurnCapConsumed,
            AutoPromoteSkipReason::Interrupted,
            AutoPromoteSkipReason::DryRun,
            AutoPromoteSkipReason::AnswerLeak {
                fields_detected: vec![],
            },
        ] {
            assert_eq!(r.event_sub_str(), None);
        }
    }

    #[test]
    fn auto_promote_outcome_summary_serializes_as_3_field_flat_shape() {
        // DR2-007: EvalRecord schema must stay 3-field flat. Confirm the
        // JSON shape matches the SSOT and `summary_id` / `skip_reason`
        // emit `null` (no skip_serializing_if at the type level — the host
        // EvalRecord controls per-field elision).
        let s = AutoPromoteOutcomeSummary {
            decision: "promoted".into(),
            skip_reason: None,
            summary_id: Some("anvil-case-x".into()),
        };
        let v = serde_json::to_value(&s).unwrap();
        let obj = v.as_object().unwrap();
        assert_eq!(obj.len(), 3, "3-field flat: {v}");
        assert_eq!(obj["decision"], "promoted");
        assert!(obj["skip_reason"].is_null());
        assert_eq!(obj["summary_id"], "anvil-case-x");
    }

    // -- Task 2.2: should_auto_promote 12 fixture ----------------------------

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

    /// Build a minimally-valid CaseRecord that passes `check_quality_gate`
    /// at `now_unix == created_at + 1` against an empty promoted-id set.
    fn fixture_promotable_case() -> CaseRecord {
        CaseRecord {
            case_id: "case_aaaaaaaaaaaaaaaaaaaaaaaa".into(),
            created_at: 1_700_000_000,
            repo_fingerprint: RepoFingerprint {
                workspace_key: "ws-key".into(),
                git_remote: Some("https://github.com/o/r.git".into()),
                git_head_branch: Some("main".into()),
                language_stack_hash: "deadbeefdeadbeef".into(),
            },
            task_signature: "fix bug".into(),
            language_stack: vec!["rust".into()],
            initial_feedback: vec![FeedbackKind::CompileError],
            successful_precautions: vec![],
            changed_files_summary: vec![],
            verify_commands: vec!["cargo test".into()],
            outcome_score: baseline_score(),
        }
    }

    fn base_inputs<'a>(
        promoted: &'a HashSet<String>,
        case: Option<CaseRecord>,
    ) -> PostLoopInputs<'a> {
        PostLoopInputs {
            plan_mode: false,
            photon_enabled: true,
            photon_client_available: true,
            no_auto_promote_env: false,
            dry_run: false,
            auto_promote_called_this_turn: false,
            case_record_extracted_this_turn: case.is_some(),
            extracted_case: case,
            already_promoted_ids: promoted,
            now_unix: 1_700_000_001,
            interrupt_flag: false,
            scrub_mode: ScrubMode::Strict,
        }
    }

    #[test]
    fn should_auto_promote_returns_promote_on_happy_path() {
        let promoted: HashSet<String> = HashSet::new();
        let inputs = base_inputs(&promoted, Some(fixture_promotable_case()));
        match should_auto_promote(&inputs) {
            AutoPromoteDecision::Promote(c) => {
                assert_eq!(c.case_id, "case_aaaaaaaaaaaaaaaaaaaaaaaa")
            }
            other => panic!("expected Promote, got {other:?}"),
        }
        // Box<CaseRecord> still derefs to fields, so the assertion above
        // works untouched.
    }

    #[test]
    fn should_auto_promote_skips_when_photon_client_unavailable() {
        let promoted: HashSet<String> = HashSet::new();
        let mut inputs = base_inputs(&promoted, Some(fixture_promotable_case()));
        inputs.photon_client_available = false;
        // Even with everything else set, photon_disabled is the top-most gate.
        inputs.plan_mode = true;
        inputs.no_auto_promote_env = true;
        match should_auto_promote(&inputs) {
            AutoPromoteDecision::Skip(AutoPromoteSkipReason::Disabled { sub_reason }) => {
                assert_eq!(sub_reason, Some("photon_disabled"));
            }
            other => panic!("expected Disabled(photon_disabled), got {other:?}"),
        }
    }

    #[test]
    fn should_auto_promote_skips_when_no_auto_promote_env_set() {
        let promoted: HashSet<String> = HashSet::new();
        let mut inputs = base_inputs(&promoted, Some(fixture_promotable_case()));
        inputs.no_auto_promote_env = true;
        match should_auto_promote(&inputs) {
            AutoPromoteDecision::Skip(AutoPromoteSkipReason::Disabled { sub_reason }) => {
                assert_eq!(sub_reason, None);
            }
            other => panic!("expected Disabled(None), got {other:?}"),
        }
    }

    #[test]
    fn should_auto_promote_skips_when_photon_enabled_is_false() {
        let promoted: HashSet<String> = HashSet::new();
        let mut inputs = base_inputs(&promoted, Some(fixture_promotable_case()));
        inputs.photon_enabled = false;
        match should_auto_promote(&inputs) {
            AutoPromoteDecision::Skip(AutoPromoteSkipReason::Disabled { sub_reason }) => {
                assert_eq!(sub_reason, None);
            }
            other => panic!("expected Disabled(None), got {other:?}"),
        }
    }

    #[test]
    fn should_auto_promote_skips_in_plan_mode() {
        let promoted: HashSet<String> = HashSet::new();
        let mut inputs = base_inputs(&promoted, Some(fixture_promotable_case()));
        inputs.plan_mode = true;
        assert!(matches!(
            should_auto_promote(&inputs),
            AutoPromoteDecision::Skip(AutoPromoteSkipReason::PlanMode)
        ));
    }

    #[test]
    fn should_auto_promote_skips_when_per_turn_cap_consumed() {
        let promoted: HashSet<String> = HashSet::new();
        let mut inputs = base_inputs(&promoted, Some(fixture_promotable_case()));
        inputs.auto_promote_called_this_turn = true;
        assert!(matches!(
            should_auto_promote(&inputs),
            AutoPromoteDecision::Skip(AutoPromoteSkipReason::PerTurnCapConsumed)
        ));
    }

    #[test]
    fn should_auto_promote_skips_on_interrupt() {
        let promoted: HashSet<String> = HashSet::new();
        let mut inputs = base_inputs(&promoted, Some(fixture_promotable_case()));
        inputs.interrupt_flag = true;
        assert!(matches!(
            should_auto_promote(&inputs),
            AutoPromoteDecision::Skip(AutoPromoteSkipReason::Interrupted)
        ));
    }

    #[test]
    fn should_auto_promote_skips_when_case_not_extracted_this_turn() {
        let promoted: HashSet<String> = HashSet::new();
        let mut inputs = base_inputs(&promoted, Some(fixture_promotable_case()));
        inputs.case_record_extracted_this_turn = false;
        assert!(matches!(
            should_auto_promote(&inputs),
            AutoPromoteDecision::Skip(AutoPromoteSkipReason::NotEligible)
        ));
    }

    #[test]
    fn should_auto_promote_skips_when_extracted_case_is_none() {
        let promoted: HashSet<String> = HashSet::new();
        // case_record_extracted_this_turn=true but extracted_case=None
        // (defensive: real call sites won't normally produce this combo).
        let mut inputs = base_inputs(&promoted, None);
        inputs.case_record_extracted_this_turn = true;
        assert!(matches!(
            should_auto_promote(&inputs),
            AutoPromoteDecision::Skip(AutoPromoteSkipReason::NotEligible)
        ));
    }

    #[test]
    fn should_auto_promote_skips_on_quality_gate_already_promoted() {
        let mut promoted: HashSet<String> = HashSet::new();
        promoted.insert("case_aaaaaaaaaaaaaaaaaaaaaaaa".into());
        let inputs = base_inputs(&promoted, Some(fixture_promotable_case()));
        match should_auto_promote(&inputs) {
            AutoPromoteDecision::Skip(AutoPromoteSkipReason::QualityGate(
                SkipReason::AlreadyPromoted,
            )) => {}
            other => panic!("expected QualityGate(AlreadyPromoted), got {other:?}"),
        }
    }

    #[test]
    fn should_auto_promote_skips_on_quality_gate_not_success_state() {
        let promoted: HashSet<String> = HashSet::new();
        let mut case = fixture_promotable_case();
        // Break the success criteria so check_quality_gate returns NotSuccessState.
        case.outcome_score.build_passed = Some(false);
        let inputs = base_inputs(&promoted, Some(case));
        match should_auto_promote(&inputs) {
            AutoPromoteDecision::Skip(AutoPromoteSkipReason::QualityGate(
                SkipReason::NotSuccessState,
            )) => {}
            other => panic!("expected QualityGate(NotSuccessState), got {other:?}"),
        }
    }

    #[test]
    fn should_auto_promote_skips_on_quality_gate_empty_language_stack() {
        let promoted: HashSet<String> = HashSet::new();
        let mut case = fixture_promotable_case();
        case.language_stack.clear();
        let inputs = base_inputs(&promoted, Some(case));
        match should_auto_promote(&inputs) {
            AutoPromoteDecision::Skip(AutoPromoteSkipReason::QualityGate(
                SkipReason::EmptyLanguageStack,
            )) => {}
            other => panic!("expected QualityGate(EmptyLanguageStack), got {other:?}"),
        }
    }

    #[test]
    fn should_auto_promote_gate_priority_photon_disabled_beats_everything() {
        // Sanity that the order matches the design SSOT: photon_disabled is
        // checked first, so even with plan_mode + interrupt + no_auto_promote
        // we still see Disabled(photon_disabled).
        let promoted: HashSet<String> = HashSet::new();
        let mut inputs = base_inputs(&promoted, Some(fixture_promotable_case()));
        inputs.photon_client_available = false;
        inputs.plan_mode = true;
        inputs.no_auto_promote_env = true;
        inputs.interrupt_flag = true;
        inputs.auto_promote_called_this_turn = true;
        match should_auto_promote(&inputs) {
            AutoPromoteDecision::Skip(AutoPromoteSkipReason::Disabled { sub_reason }) => {
                assert_eq!(sub_reason, Some("photon_disabled"));
            }
            other => panic!("expected Disabled(photon_disabled), got {other:?}"),
        }
    }

    // -- Task 2.3: decide_oversize boundary ----------------------------------

    #[test]
    fn decide_oversize_under_max_returns_none() {
        assert_eq!(decide_oversize(MAX_ACTION_SUMMARY_BYTES - 1), None);
    }

    #[test]
    fn decide_oversize_at_max_returns_none() {
        // Boundary is inclusive: == MAX is still allowed.
        assert_eq!(decide_oversize(MAX_ACTION_SUMMARY_BYTES), None);
    }

    #[test]
    fn decide_oversize_above_max_returns_quality_gate_oversize() {
        match decide_oversize(MAX_ACTION_SUMMARY_BYTES + 1) {
            Some(AutoPromoteSkipReason::QualityGate(SkipReason::Oversize)) => {}
            other => panic!("expected Some(QualityGate(Oversize)), got {other:?}"),
        }
    }

    // -- AutoPromoteConfig basic shape check ---------------------------------

    #[test]
    fn auto_promote_config_default_shape_compiles() {
        // Sanity: confirm the field set matches the design SSOT so other
        // call sites can construct a config without surprises.
        let c = AutoPromoteConfig {
            enabled: true,
            force_disabled: false,
            dry_run: true,
            scrub_mode: ScrubMode::Strict,
        };
        assert!(c.enabled);
        assert!(!c.force_disabled);
        assert!(c.dry_run);
        assert_eq!(c.scrub_mode, ScrubMode::Strict);
    }

    // -- Task 4.2: build_request_id SSOT (sha256 NUL || le_bytes) ------------

    #[test]
    fn build_request_id_is_16_hex_chars_and_deterministic() {
        let id1 = super::build_request_id("sess-x", 3, "case_abc");
        let id2 = super::build_request_id("sess-x", 3, "case_abc");
        assert_eq!(id1, id2, "deterministic");
        assert_eq!(id1.len(), 16, "16 hex chars (= 8 bytes)");
        // hex chars only
        assert!(
            id1.chars().all(|c| c.is_ascii_hexdigit()),
            "hex only: {id1}",
        );
    }

    #[test]
    fn build_request_id_differs_on_session_or_turn_or_case() {
        let base = super::build_request_id("s", 1, "c");
        assert_ne!(base, super::build_request_id("S", 1, "c"));
        assert_ne!(base, super::build_request_id("s", 2, "c"));
        assert_ne!(base, super::build_request_id("s", 1, "C"));
    }

    // -- Task 5.1: invoke_photon_auto_promote mockito smoke ------------------
    //
    // These tests exercise the hook directly with a real mockito-backed
    // `PhotonClient`. The hook does not depend on `Agent`; the caller
    // (`turn.rs`) merges the outcome into `agent.last_auto_promote_outcome`.

    use crate::photon::PhotonClient;
    use crate::session::case_photon_bridge::PromoteLogEntry;
    use std::sync::OnceLock;
    use tempfile::TempDir;

    fn make_photon_client(url: String) -> PhotonClient {
        PhotonClient::new(url, 5_000).expect("client init")
    }

    fn make_config(dry_run: bool, scrub_mode: ScrubMode) -> AutoPromoteConfig {
        AutoPromoteConfig {
            enabled: true,
            force_disabled: false,
            dry_run,
            scrub_mode,
        }
    }

    /// Init the OnceLock-guarded `EVAL_LOG_LOGGER` once so `log_llm_event`
    /// inside the hook does not panic. Idempotent across tests.
    fn ensure_test_logger() {
        static INIT: OnceLock<()> = OnceLock::new();
        INIT.get_or_init(|| {
            // log_llm_event writes via the global logger; if not set, it is a
            // no-op. Nothing to init beyond that for tests.
        });
    }

    // VR-04: Phase A photon_client_unavailable hard disable.
    #[test]
    fn hook_skips_when_photon_client_unavailable() {
        ensure_test_logger();
        let tmp = TempDir::new().unwrap();
        let case = fixture_promotable_case();
        let cfg = make_config(true, ScrubMode::Strict);
        let outcome = super::invoke_photon_auto_promote(
            "sess-vr04",
            1,
            false, // plan_mode
            false, // auto_promote_called_this_turn
            true,  // case_record_extracted_this_turn
            Some(&case),
            tmp.path(),
            1_700_000_001,
            false, // interrupt
            None,  // photon_client
            &cfg,
        );
        assert_eq!(outcome.decision, "skipped");
        assert_eq!(outcome.skip_reason.as_deref(), Some("disabled"));
        assert!(outcome.summary_id.is_none());
    }

    // VR-05: Phase A interrupt routing.
    #[test]
    fn hook_skips_on_interrupt() {
        ensure_test_logger();
        let tmp = TempDir::new().unwrap();
        let mut server = mockito::Server::new();
        let m = server
            .mock("POST", "/v1/summary/upsert")
            .expect(0) // must NOT be called on interrupt
            .with_status(200)
            .create();
        let client = make_photon_client(server.url());
        let case = fixture_promotable_case();
        let cfg = make_config(true, ScrubMode::Strict);
        let outcome = super::invoke_photon_auto_promote(
            "sess-vr05",
            1,
            false,
            false,
            true,
            Some(&case),
            tmp.path(),
            1_700_000_001,
            true, // interrupt
            Some(&client),
            &cfg,
        );
        assert_eq!(outcome.decision, "skipped");
        assert_eq!(outcome.skip_reason.as_deref(), Some("interrupted"));
        m.assert();
    }

    // VR-01: dry_run happy path emits succeeded outcome but no HTTP.
    #[test]
    fn hook_dry_run_emits_succeeded_without_http() {
        ensure_test_logger();
        let tmp = TempDir::new().unwrap();
        let mut server = mockito::Server::new();
        let m = server
            .mock("POST", "/v1/summary/upsert")
            .expect(0) // dry_run skips HTTP
            .with_status(200)
            .create();
        let client = make_photon_client(server.url());
        let case = fixture_promotable_case();
        let cfg = make_config(true, ScrubMode::Strict);
        let outcome = super::invoke_photon_auto_promote(
            "sess-vr01dry",
            1,
            false,
            false,
            true,
            Some(&case),
            tmp.path(),
            1_700_000_001,
            false,
            Some(&client),
            &cfg,
        );
        assert_eq!(outcome.decision, "promoted");
        assert!(outcome.skip_reason.is_none());
        assert!(
            outcome.summary_id.is_some(),
            "summary_id present on dry_run promoted: {outcome:?}",
        );
        m.assert();
    }

    // VR-01: live mode (dry_run=false) happy path — HTTP 200 → promoted.
    #[test]
    fn hook_live_happy_path_emits_promoted() {
        ensure_test_logger();
        let tmp = TempDir::new().unwrap();
        let mut server = mockito::Server::new();
        let _m = server
            .mock("POST", "/v1/summary/upsert")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(
                r#"{"schema_version":"action-memory.v0.2","request_id":"deadbeefcafef00d","summary_id":"anvil-case-aaaaaaaaaaaaaaaaaaaaaaaa","status":"stored"}"#,
            )
            .create();
        let client = make_photon_client(server.url());
        let case = fixture_promotable_case();
        let cfg = make_config(false, ScrubMode::Strict);
        let outcome = super::invoke_photon_auto_promote(
            "sess-vr01live",
            1,
            false,
            false,
            true,
            Some(&case),
            tmp.path(),
            1_700_000_001,
            false,
            Some(&client),
            &cfg,
        );
        assert_eq!(outcome.decision, "promoted", "outcome: {outcome:?}");
        assert!(outcome.summary_id.is_some());
        // promote log was written on success.
        let log_path = tmp.path().join("photon-promote-log.jsonl");
        assert!(log_path.exists(), "promote log written");
        let contents = std::fs::read_to_string(&log_path).unwrap();
        let entry: PromoteLogEntry = serde_json::from_str(contents.trim()).unwrap();
        assert_eq!(entry.outcome, "promoted");
        assert_eq!(entry.case_id, "case_aaaaaaaaaaaaaaaaaaaaaaaa");
    }

    // VR-08: HTTP 500 → failed outcome (fail-open).
    #[test]
    fn hook_http_5xx_emits_failed() {
        ensure_test_logger();
        let tmp = TempDir::new().unwrap();
        let mut server = mockito::Server::new();
        let _m = server
            .mock("POST", "/v1/summary/upsert")
            .with_status(500)
            .with_body("internal error")
            .create();
        let client = make_photon_client(server.url());
        let case = fixture_promotable_case();
        let cfg = make_config(false, ScrubMode::Strict);
        let outcome = super::invoke_photon_auto_promote(
            "sess-vr08",
            1,
            false,
            false,
            true,
            Some(&case),
            tmp.path(),
            1_700_000_001,
            false,
            Some(&client),
            &cfg,
        );
        assert_eq!(outcome.decision, "failed", "outcome: {outcome:?}");
        assert!(outcome.summary_id.is_some());
        // No promote log entry on failure.
        assert!(!tmp.path().join("photon-promote-log.jsonl").exists());
    }

    // VR-16: HTTP 422 answer_leak_detected → rejected_by_photon outcome.
    #[test]
    fn hook_http_422_emits_rejected_by_photon() {
        ensure_test_logger();
        let tmp = TempDir::new().unwrap();
        let mut server = mockito::Server::new();
        let _m = server
            .mock("POST", "/v1/summary/upsert")
            .with_status(422)
            .with_header("content-type", "application/json")
            .with_body(
                r#"{"detail":{"error":"answer_leak_detected"},"quality_warnings":["facts[0]: literal 42"]}"#,
            )
            .create();
        let client = make_photon_client(server.url());
        let case = fixture_promotable_case();
        let cfg = make_config(false, ScrubMode::Strict);
        let outcome = super::invoke_photon_auto_promote(
            "sess-vr16",
            1,
            false,
            false,
            true,
            Some(&case),
            tmp.path(),
            1_700_000_001,
            false,
            Some(&client),
            &cfg,
        );
        assert_eq!(
            outcome.decision, "rejected_by_photon",
            "outcome: {outcome:?}",
        );
        assert!(outcome.summary_id.is_some());
    }

    // VR-06/VR-07 (combined): strict-mode scrub hit → skipped(answer_leak),
    // no HTTP call.
    #[test]
    fn hook_strict_scrub_hit_skips_with_answer_leak() {
        ensure_test_logger();
        let tmp = TempDir::new().unwrap();
        let mut server = mockito::Server::new();
        let m = server
            .mock("POST", "/v1/summary/upsert")
            .expect(0)
            .with_status(200)
            .create();
        let client = make_photon_client(server.url());
        // Inject a case whose verify_command will appear as a hint with a
        // numeric_answer_equality pattern hit (e.g. "score = 42").
        let mut case = fixture_promotable_case();
        case.verify_commands = vec!["score = 42".to_string()];
        let cfg = make_config(true, ScrubMode::Strict);
        let outcome = super::invoke_photon_auto_promote(
            "sess-vr06",
            1,
            false,
            false,
            true,
            Some(&case),
            tmp.path(),
            1_700_000_001,
            false,
            Some(&client),
            &cfg,
        );
        assert_eq!(outcome.decision, "skipped");
        assert_eq!(outcome.skip_reason.as_deref(), Some("answer_leak"));
        assert!(outcome.summary_id.is_none());
        m.assert();
    }
}
