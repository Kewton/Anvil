# 022-3 Deterministic Verify Policy Implementation Result

作成日: 2026-06-30

## 実装内容

022-3 の目的に対して、StepPlan/UltraPlan 経由の deterministic verify policy を source semantics に寄せた。

主な変更:

- `mvp/anvilminimal/src/planner/verify.rs`
  - shell control 検出を source の `contains_evidence_poisoning_shell_control` に寄せ、`&`, `<`, `>`, newline, carriage return, backslash も保守的に拒否する。
  - `&&`, `||`, `|`, `;`, backtick, `$(` は引き続き拒否する。
- `mvp/anvilminimal/src/planner/step_plan.rs`
  - LLM 生成 JSON の `expected_result` を必須化した。
  - legacy YAML の既存互換は維持する。
  - verify command の `&&` 分割、`npm install`/dev server command の黙殺を廃止した。
  - 安全な `node -e existsSync(...)` -> `test -f ...` の機械変換だけを残した。
- `mvp/anvilminimal/src/planner/lint.rs`
  - dependency verify before setup/manifest の negative fixture を追加した。
- `mvp/anvilminimal/src/planner/runner.rs`
  - `dependency_order` lint を `verify_dependency_order_error` として emit する。
- `mvp/anvilminimal/scripts/eval_lib/failure_classification.py`
  - `verify_dependency_order_error` を planning failure kind に追加した。
  - setup/dev server verify policy、missing `expected_result`、dependency order を stderr だけでも具体分類できるようにした。
- `mvp/anvilminimal/eval/fixtures/planner_failures/20260625_speed_cloud_unclassified.json`
  - dependency order の過去 fixture を `planner_lint_error` から `verify_dependency_order_error` に更新した。

## Source Semantics との対応

参照した source:

- `src/agent/loop_run/verifier_command_policy.rs`
- `src/agent/loop_run/completion_evidence.rs`
- `src/agent/loop_run/verifier.rs`
- `src/agent/loop_run/verifier_driver.rs`
- `src/agent/minimal_step_runner.rs`

対応した差分:

| Source semantics | MVP 022-3 対応 |
| --- | --- |
| shell control を verifier evidence として認めない | planner verify command policy でも保守的に拒否 |
| verifier hint は setup/dev server ではない | setup/install/dev server を verify policy error として分類 |
| package manifest/setup 前の build/test verify を成功扱いしない | dependency order lint を具体 failure kind 化 |
| plan schema は expected_result を明示する | generated StepPlan JSON では missing expected_result を schema error |

## Gate 状態

G-S08 は `fail` から `partial` へ移動した。

理由:

- Positive/negative fixture と classifier は追加済み。
- `cargo test` と `pytest` は通過済み。
- ただし、同条件 eval smoke と source/MVP normalized trace diff は未再実行のため `pass` にはしない。

## 検証

実行済み:

```bash
cargo test --manifest-path mvp/anvilminimal/Cargo.toml
python3 -m pytest mvp/anvilminimal/tests/eval
```

結果:

- `cargo test`: pass
- `pytest`: 174 passed, 1 skipped

## 未実施

以下は未実施であり、G-S08 を pass にしない理由として残す。

- 最新 MVP smoke eval
- 最新 `anvildev --engine minimal` same-condition eval
- `runtime-semantics-trace-diff.json` による source/MVP stage/gate diff
