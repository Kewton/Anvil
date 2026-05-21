# CLAUDE.md

毎会話の context に load される設計指針。**ルールと不変条件のみ**を記述する。
詳細仕様 / 実装履歴 / API spec は記載しない（コード / git log / dev-reports が SSOT）。

## Repo Intent

Rust 版 local-first coding agent (Ollama 専用、`workspace/v0.1.0` ベース)。
旧 Anvil 拡張を継ぎ足すのではなく、vibe-local 的な小さい実装に振り切る。

## File Map

- `src/photon/*` — Photon サイドカー HTTP 連携 / context_pack レンダラー / adoption signal
- `src/agent/loop_run/*` — エージェントループ / turn 制御 / 各種 confirm skill (work_mode / feedback_kind / quality / photon_user_feedback)
- `src/agent/loop_run/project_verifier.rs` — `ProjectVerifier` capability (verifier_repair cheap check SSOT, Issue #639). private mod, not re-exported (DR3-001) — `turn.rs` is the only in-crate consumer.
- `src/agent/loop_run/semantic_failure.rs` — Issue #647 Phase A.1: `SemanticFailureReport` schema + `build_failure_cluster_from_observation` + `cluster_key` (deterministic, sanitize-then-shape-then-hash). private mod, not re-exported (DR3-001).
- `src/agent/loop_run/spec_authority.rs` — Issue #647 Phase A.2: `SpecAuthority` enum (5 variant) + `select_authority` + `ArtifactConsensus` tie-break + `detect_test_weakening` / `detect_impl_weakening` (9 pure-fn detectors). private mod, not re-exported (DR3-001).
- `src/agent/loop_run/auto_test.rs` — Issue #651: `VerifierCommand` (allowlisted constructor, private fields) + `OwnedTestVerifierPlan::{Runnable, Weak, Missing}` enum + `detect_with_owned_test_artifacts` / `validate_bound_test_artifacts_for_execution` / `run_structured` (spawn + concurrent stdout/stderr drain + polling timeout/kill). `pub(super)`、`loop_run.rs` から re-export しない (DR3-001).
- `src/agent/loop_run/artifact_ownership.rs` — Issue #651: `owned_test_artifacts(artifacts, work_root, scope, edited_this_session_for, scaffold_changed_for)` ヘルパ (SSOT)。内部で `classify_ownership` を必ず通し `Owned` のみ採用、`ignored top dir` 拒否。
- `src/agent/loop_run/task_contract.rs` — Issue #651: `CompletionDecision::SafeStop { reason: SafeStopReason }` variant + `ArtifactRecoveryAction::SafeStop { reason }` variant + `evaluate_with_owned_test_artifacts` gate (`test_execution_required=true && owned_test_artifacts.is_empty()` → `SafeStop`)。既存 `evaluate(&evidence)` は `EvaluateMode::Legacy` で gate を bypass (regression guard).
- `src/agent/loop_run/repair_attempt_outcome.rs` — Issue #653: `RepairAttemptOutcome` lifecycle ledger (`RepairAttemptOutcomeKind` 5 variant / `RepairRejectionKind` 2 variant / `should_promote_to_exhausted_after_push` pure-fn / `MAX_REPAIR_ATTEMPT_OUTCOMES = 16`). private mod, not re-exported (DR3-001).
- `src/agent/loop_run/artifact_completion_job.rs` — Issue #652: `ArtifactCompletionJob` / `ArtifactAttemptOutcome` (4 variant: WrongTarget / NoTool / ProseOnly / RolePolicyViolation) / `ArtifactCompletionFailureSnapshot` + role-specific retry budget (`ARTIFACT_COMPLETION_ATTEMPT_LIMIT=3`) + projection to `RecoveryTarget` / `EffectiveToolPolicy::artifact_directed`. Sanitize pipeline `mask_secrets` + length cap + control-char neutralize + `mask_payload_inplace` final-defence line. private mod, not re-exported (DR3-001). `turn.rs` is the only behavioral in-crate consumer.
- `src/agent/loop_run/safe_stop_e2e_tests.rs` — Issue #654: in-crate `#[cfg(test)] mod` for `agent.safe_stop.report` event 5 stop_reason E2E (artifact_completion_failed / verifier_failed_safe_stop / verifier_weak / verifier_missing / diagnostic_target_missing). production binary には含まれない (DR3-001 / CB-001 fix)。`repair_job.rs` の `StopReason` / `DiagnosticTargetMissingReason` / `ExhaustedAttemptsSummary` / `SafeStopInput` / `SafeStopReport` / `safe_relative_path_string` / `build_actual_actions` / `select_diagnostic_target_missing_reason` も同 Issue で追加（pub(super)、`turn.rs` のみが in-crate consumer）。`turn.rs::build_safe_stop_payload` (純関数、4KB hard cap) + `record_safe_stop_report` (薄い shell、`Agent.safe_stop_report_emitted: HashSet<StopReason>` で per-turn dedup) も同 Issue。
- `src/agent/loop_run/artifact_ledger.rs` — Issue #659: turn-local append-only `ArtifactLedger` SSOT for current-turn artifact observations. `ArtifactOrigin` 3-variant (`#[non_exhaustive] Existing / Scaffold / RepoEdit`) + `LedgerAdmissionContext { work_root, scope }` + `classify_ownership` SSOT 経由の admission rule (workspace-relative + `ancestor_within_work_root` symlink containment + length cap + role re-confirm)。4 record helpers (`record_existing_event` / `record_scaffold_event` idempotent baseline、`record_repo_edit_event` per-observation append、`record_verifier_observation` upsert)、bounded by `MAX_ARTIFACT_LEDGER_EVENTS=256` / `MAX_ARTIFACT_LEDGER_PATH_BYTES=4096` / `MAX_ARTIFACT_LEDGER_OBSERVATIONS=256` (cap 超過は新規 reject + `overflowed=true`)。3 projection (`owned_test_artifacts` / `required_artifacts_completed: BTreeMap` / `active_job_candidates` declaration-order stub) は `overflowed=true` 時 fail-closed。Observability は `agent.artifact_ledger.{event_recorded,turn_summary,divergence_detected}` 3 event を `mask_payload_inplace` 最終防衛線で emit、raw path 非出力 (`path_hash` のみ)。`Agent::artifact_ledger` field は `turn.rs::handle_user_message` 冒頭で `clear()` (per-turn rule、`SessionSnapshot` 不所持)。dual-source divergence assertion (`#[cfg(debug_assertions)]`) は legacy `turn_edited_relative_paths` を authority とする write-through adapter。`task_contract_artifact_states` / `owned_test_artifacts_for_verifier` の内部実装は ledger を並行計算 (adapter 期間中は legacy authority、divergence 時 legacy 結果)。test seam は `pub(crate) fn seed_artifact_ledger_repo_edit_for_test(agent, path)` (旧 `seed_turn_edited_relative_path_for_test` は本 Issue で削除)。private mod, not re-exported (DR3-001)、`turn.rs` が唯一の in-crate consumer。policy 型 (`CompletionDecision` / `ArtifactRecoveryAction` / `EffectiveToolPolicy` / `RepairJob` / `VerifierCommand`) を import しない。後続 #660/#661/#663 が同 projection を読む signature anchor。
- `src/agent/skills/*` — SkillRegistry 基盤 (`AgentSkill` trait, `SkillTrustTier`)
- `src/session/*` — セッションストア / `FeedbackFrame` / `Precaution` / `AnvilScore` / `CaseRecord` / `eval_log` / `case_retrieval` / `case_photon_bridge` / `tmp_tests` / `rollout_policy` / `feedback.rs::redact_verifier_command_for_storage` (verifier command redactor SSOT) / `store.rs::VerifierInvocationRecord` (Phase α-2)
- `src/tools/*` — built-in tools (`bash.rs::check_blocked_command` SSOT, Read/Write/Edit, `test_output.rs` failed-name + trim formatter)
- `src/repo_graph/*` — `RepoGraph` v1 (import scan + LRU persist)
- `src/util/file_classify.rs` — `is_test_file` / `is_setup_file` / `is_implementation_file` SSOT
- `src/util/git_hardened.rs` — `run_git` SSOT (hardened git command runner)
- `src/logging.rs` — `log_llm_event` + `mask_payload_inplace` / `is_secret_like_key`
- `src/config.rs` — CLI / env / `.anvil/config` のマージ
- `src/model_registry.rs` — 利用可能モデルとメモリ量から main / sidecar を選択
- `src/ollama/*` — Ollama HTTP 連携 + `<think>` strip / XML tool call 回収
- `src/modes/plan_act.rs` — Plan / Act mode 状態 + WorkMode 二段階分類純関数
- `src/tui/markdown.rs` — assistant 応答の SGR markdown renderer
- `src/git/checkpoint.rs` — checkpoint / rollback

## Layer Rules (不変条件)

- **DR3-001**: `pub(crate)` 内部 API は `src/photon/mod.rs` 等の facade から re-export しない（module unit tests のみで検証）
- **DR3-002**: photon 層は session 層 / agent 層を import しない（photon → session → agent の単方向依存）
- **DR3-003**: log filter / OnceLock logger 系は unique session_id で隔離（test isolation）

## Security Invariants

- すべての外部由来文字列は `session::feedback::mask_secrets` を通す
- 永続化 / log payload は `logging::mask_payload_inplace` を **最終防衛線** として通す
- summary_id 系の正規化は `src/photon/prompt.rs::sanitize_summary_id` を **SSOT** として注入側 / 評価側の双方で必ず通す (DR4-002)
- localhost / credential なし URL 制約: `validate_localhost_url`

## Development Expectations

- 旧アーキテクチャへ戻す方向の継ぎ足しはしない
- provider abstraction を増やさない
- local LLM 向けの prompt / loop の単純さを優先する
- 新機能は E2E 寄りの検証を伴わせる（mockito ベース、Ollama 不要が望ましい）
- per-turn cap が必要な hook は `<feature>_called_this_turn: bool` を `Agent` / `SessionSnapshot` に追加し、`handle_user_message` 冒頭で reset

## Issue 番号と詳細仕様の所在

- 完了済 Issue の実装詳細 → `git log` / `gh issue view <N>` / `gh pr view <N>`
- 各 Issue の設計判断 → `dev-reports/design/issue-<N>-*.md` / `dev-reports/issue/<N>/`
- アーキテクチャ全体 → コード（モジュール単位で `mod.rs` doc comment 参照）

## Release Process

変更しない。

- package / binary は `anvil`
- `.github/workflows/ci.yml` は `fmt` `clippy` `test` `build`
- `.github/workflows/release.yml` は tag push 時に gzip artifact を作って GitHub Release を切る
