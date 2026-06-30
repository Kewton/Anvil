# 022-7 Final Acceptance / UAT-equivalent / TUI Observability Implementation Result

作成日: 2026-06-30

## 対象

`workspace/mvp/eval/022/README.md` の 022-7 に従い、MVP `anvilminimal` の final acceptance を通常実行にも接続し、Next.js interactive app/game の build-only / title-only false positive を抑制した。あわせて TUI/manual run の command-level event と停止理由を保存するようにした。

## 参照した source

- `/Users/maenokota/share/work/github_kewton/Anvil-develop/src/agent/loop_run/verifier.rs`
- `/Users/maenokota/share/work/github_kewton/Anvil-develop/src/agent/loop_run/verifier_driver.rs`
- `/Users/maenokota/share/work/github_kewton/Anvil-develop/src/agent/loop_run/task_contract_completion_policy.rs`
- `/Users/maenokota/share/work/github_kewton/Anvil-develop/src/agent/loop_run/actor_loop_flow.rs`
- `/Users/maenokota/share/work/github_kewton/Anvil-develop/src/agent/minimal_step_runner/profile.rs`

## 実装内容

- `RuntimeAcceptanceReport` の required evidence に `nextjs_route_evidence` を追加し、Next.js app/game の route evidence を判定できるようにした。
- `build_command_or_dependency_missing_boundary` を、verify/deferred build だけでなく `package.json` の `scripts.build = next build` からも build contract evidence として扱うようにした。
- plan-run / ultra-plan-run の final contract で、Next.js app/game goal から `nextjs_route_evidence` と `build_command_or_dependency_missing_boundary` を推定 required evidence として追加した。
- final acceptance event に `release_gate_status` / `release_gate_reasons` を追加し、interactive app/game で browser readiness / interaction evidence が未取得のときは release gate を `partial` にするようにした。
- eval acceptance outcome に `release_gate_status` / `release_gate_reasons` を追加し、summary row にも出せるようにした。
- CLI action 全体で `run_stop` event を出し、失敗時は `.anvil/runs/<run-id>/summary.md` へ停止理由を保存するようにした。
- TUI slash command 実行で `tui_command_start` / `tui_command_stop` / failure時の `loop_stop` を出し、失敗 stage と stop reason を保存するようにした。

## 追加した fixture / test

- static title-only Next.js app は acceptance false positive として検出される。
- build-only / route-only では interactive app/game acceptance を満たさない。
- interactive Next.js app は route / build contract / capability evidence を満たす。
- browser oracle が未有効の場合、acceptance success でも release gate は `partial` になる。
- TUI slash command failure で run events と failure stage が `.anvil/runs/.../events.jsonl` に残る。

## Gate 更新

- `G-S12 final acceptance` は `fail` から `partial` に変更する。
- 理由: final acceptance と release gate event は通常実行・eval の双方に接続したが、browser interaction 実行証跡と manual UAT trace はまだ添付していない。
- `G-S16 TUI/manual run observability` は `partial` のまま、command-level failure event fixture を追加した。

## 検証

- `python3 -m pytest mvp/anvilminimal/tests/eval/test_acceptance_outcome.py mvp/anvilminimal/tests/eval/test_browser_interaction_oracle.py`
  - 4 passed
- `cargo test --manifest-path mvp/anvilminimal/Cargo.toml plan_run_nextjs -- --nocapture`
  - 5 passed
- `cargo test --manifest-path mvp/anvilminimal/Cargo.toml nextjs_interactive_app_requires_route_and_build_contract_evidence -- --nocapture`
  - 1 passed
- `cargo test --manifest-path mvp/anvilminimal/Cargo.toml tui_slash_failure_records_run_events_and_failure_stage -- --nocapture`
  - 1 passed
- `python3 -m pytest mvp/anvilminimal/tests/eval`
  - 176 passed, 1 skipped
- `cargo test --manifest-path mvp/anvilminimal/Cargo.toml`
  - 368 lib tests passed
  - integration tests passed
  - ignored live/provider/TUI pty tests は従来どおり未実行

## 残タスク

- browser readiness / interaction adapter を明示 opt-in の release gate として実行し、証跡を `source_mvp_trace_manifest.md` に添付する。
- manual TUI UAT で `.anvil/runs/<run-id>/events.jsonl` と `summary.md` が実運用でも残ることを確認する。
- MVP / anvildev の同条件 trace で final acceptance lifecycle stage の差分を確認する。
