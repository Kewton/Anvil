# Ultra Phase Context Continuity Implementation Result

作成日: 2026-06-29

## 実装内容

Phase 0〜6 の実装として、以下を反映した。

- `run_step_plan_with_ui` は public behavior を維持し、内部で新規 `SessionSnapshot` を作る。
- ultra-run 内部用に `run_step_plan_with_session_with_ui` を追加し、caller-owned session を受け取る。
- `StepPlanRunOutcome` / `StepRunOutcome` / `StepPlanRunError` / `StepRunError` を追加し、失敗時も partial outcome を保持する。
- `run_ultra_plan_with_ui` の phase step execution は ultra 全体の `ultra_session` を使い回す。
- `UltraRunContext` を追加し、completed phases、changed paths、verify failures、repair changed paths、pending final artifacts、repair targets を bounded summary として保持する。
- `ultra_phase_prompt` に prior ultra context を追加した。
- `ultra_context_initialized`、`ultra_phase_context_attached`、`ultra_phase_context_updated` event を追加した。
- eval に `ultra_context_continuity_score` と関連 subscore を追加した。

## 意図的に対象外としたもの

計画に合わせ、profile final repair は shared execution session に載せていない。profile failure は `UltraRunContext` に記録するが、repair 実行自体は従来どおり独立 session のままとした。

## 変更ファイル

- `mvp/anvilminimal/src/planner/runner.rs`
- `mvp/anvilminimal/tests/tui_integration.rs`
- `mvp/anvilminimal/scripts/eval_lib/runtime_scoring.py`
- `mvp/anvilminimal/scripts/eval_lib/run_summary.py`
- `mvp/anvilminimal/scripts/eval_lib/report.py`
- `mvp/anvilminimal/eval/plan_score_schema.yaml`
- `mvp/anvilminimal/tests/eval/test_runtime_scoring.py`

## 受入条件への対応

- ultra-run phase 2 の execution request は phase 1 の session history を含む。
- planner request は phase ごとの独立 call を維持する。
- step-plan public API は独立 session のまま。
- failure path で partial outcome を context に反映できる。
- bounded context は path、failure snippet、repair target に限定し、raw stderr を保持しない。
- eval event で shared session と bounded context の有無を観測できる。
