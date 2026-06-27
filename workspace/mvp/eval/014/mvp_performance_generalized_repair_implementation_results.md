# MVP Performance Generalized Repair Implementation Results

作成日: 2026-06-27

対象:

- `workspace/mvp/eval/014/mvp_performance_generalized_repair_plan.md`
- `workspace/mvp/eval/014/mvp_performance_generalized_repair_work_breakdown.md`

## 実施サマリ

Phase 0〜9 のうち、現時点でコード変更が必要だったのは Phase 1〜3 と eval 側の Phase 3 連携だった。

Phase 4〜6 は、前段までに入っていた step repair context、completion contract、provider failure classification、target runtime metric が計画の受け入れ条件を概ね満たしていたため、今回の追加コードは行わず、既存実装とテストで確認した。

## Phase 別結果

| Phase | 結果 | 実施内容 |
|---|---|---|
| Phase 0 | done | baseline / impact / negative control を `mvp_performance_generalized_repair_baseline.md` に固定 |
| Phase 7 | done | source parity gate を `mvp_performance_source_parity_matrix.md` に作成 |
| Phase 1 | done | schema retry prompt に detected schema issue hints を追加。lint retry prompt に verify 意味保存と setup/dev-server 分離 guidance を追加 |
| Phase 2 | done | `VerifyCommandViolationKind`, `VerifyCommandDiagnosis`, `diagnose_verify_command`, `normalize_verify_command` を追加。既存 reject policy は維持 |
| Phase 3 | done | ultra phase scaffold 後に `ultra_phase_plan_validated` event を追加。eval runtime scoring が明示 validation event を扱うよう更新 |
| Phase 4 | verified | `RepairContext` / `build_repair_prompt_with_context` / `step_obligation_scope` event / bounded repair cap が既存実装済みであることを確認 |
| Phase 5 | verified | completion contract と postcheck stability scoring が既存実装済み。eval-only oracle は通常 runtime に入れない方針を維持 |
| Phase 6 | verified | provider failure は provider layer と capability-excluded に分離済み。retry policy は今回変更なし |
| Phase 8 | in progress | unit / integration / build / eval で検証 |
| Phase 9 | in progress | release build と symlink/TUI smoke を確認 |

## Code Changes

### Planner retry

変更:

- `mvp/anvilminimal/src/planner/runner.rs`
  - `build_schema_retry_prompt()` に `Detected schema issues` を追加
  - `schema_retry_issue_hints()` を追加
  - `lint_retry_hard_constraints()` の verify policy guidance を補強

意図:

- missing goal / empty steps / numeric id / invalid expected_result / unsafe path を、free-form error だけでなく retry prompt 内で明示する。
- shell control syntax を単純な `test -f` に落とすような弱い修復を避ける。

過適応防止:

- scenario id / suite 名 / 固定成果物名は参照していない。
- retry 回数は増やしていない。
- lint/verify policy は緩和していない。

### Verify command diagnosis

変更:

- `mvp/anvilminimal/src/planner/verify.rs`
  - `VerifyCommandViolationKind`
  - `VerifyCommandDiagnosis`
  - `diagnose_verify_command()`
  - `normalize_verify_command()`

意図:

- `verify_command_policy_error` を単なる文字列ではなく、policy violation category として扱えるようにする。
- 既存の `validate_verify_command()` は同じ reject behavior を維持する。

許可した正規化:

- whitespace の正規化のみ。

許可していないこと:

- install/dev server の verify 化。
- shell control syntax の分解実行。
- 強い verify から弱い existence check への silent downgrade。

### Ultra phase diagnostics

変更:

- `mvp/anvilminimal/src/planner/runner.rs`
  - `ultra_phase_plan_validated` event を追加
- `mvp/anvilminimal/scripts/eval_lib/runtime_scoring.py`
  - runtime event allowlist に `ultra_phase_plan_validated` を追加
  - event がある場合は `phase_plan_validity_score` を validation event 比率で算出
  - event がない既存 run は `ultra_phase_scaffold_complete` に fallback

意図:

- ultra phase の StepPlan scaffold が通っただけでなく、phase-local plan として検証済みになった時点を観測できるようにする。
- 既存 run root / anvildev との比較を壊さない。

## Existing Implementation Confirmed

Phase 4:

- `planner/repair.rs::RepairContext` は overall goal / required final artifacts / step instruction / expected paths / verify commands / expected_result / changed files / progress warning を持つ。
- `planner/runner.rs::run_step()` は verification failure 後に bounded repair loop へ入り、同じ failure が進展しない場合は progress warning を出す。
- `minimal_loop/loop_run.rs` は plan-run step scope の `step_obligation_scope` event を出し、completion contract side effect を step 実行中は無効化する。

Phase 5:

- completion contract は eval harness から `--completion-contract-json` として渡される。
- 通常 runtime は suite `expected_artifacts` / postcheck oracle を直接参照しない。
- postcheck stability は eval report 側で reason として扱う。

Phase 6:

- `failure_classification.py` は provider HTTP / transient / 404 / parse error を provider layer に分類する。
- `report.py` は raw success と capability included/excluded を Failure Layers で表示する。

## Tests Run

```bash
python3 mvp/anvilminimal/scripts/eval-report.py \
  --run-root /private/tmp/anvilminimal-eval-013-live-mvp-rerun-net
```

結果:

- pass

```bash
python3 -m pytest mvp/anvilminimal/tests/eval/test_failure_classification.py -q
```

結果:

- `5 passed`

```bash
cargo test --manifest-path mvp/anvilminimal/Cargo.toml planner::verify --lib
```

結果:

- `16 passed`

```bash
cargo test --manifest-path mvp/anvilminimal/Cargo.toml planner::runner --lib
```

結果:

- `36 passed`

```bash
python3 -m pytest \
  mvp/anvilminimal/tests/eval/test_runtime_scoring.py \
  mvp/anvilminimal/tests/eval/test_eval_event_report.py \
  mvp/anvilminimal/tests/eval/test_failure_classification.py \
  mvp/anvilminimal/tests/eval/test_summary_schema.py \
  -q
```

結果:

- `24 passed`

```bash
cargo test --manifest-path mvp/anvilminimal/Cargo.toml planner::repair --lib
```

結果:

- `3 passed`

```bash
cargo test --manifest-path mvp/anvilminimal/Cargo.toml minimal_loop::loop_run --lib
```

結果:

- `34 passed`

```bash
cargo test --manifest-path mvp/anvilminimal/Cargo.toml planner::lint --lib
```

結果:

- `33 passed`

## Residual Work For Phase 8/9

- Full `python3 -m pytest mvp/anvilminimal/tests/eval -q`
- Full `cargo test --manifest-path mvp/anvilminimal/Cargo.toml`
- `cargo build --release --manifest-path mvp/anvilminimal/Cargo.toml`
- known suite eval
- blind suite eval
- symlink check
- TUI routing smoke

