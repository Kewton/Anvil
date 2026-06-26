# step-plan executable / verify strength 改善 実施結果

## 実施日

- 2026-06-26

## 実装概要

Phase 0〜9 のうち、step-plan 生成品質に関する実装と検証を実施した。

主な変更:

- `PlanQualitySeverity`, `PlanQualityIssue`, `PlanQualityReport`, `PlanQualityContext` を追加
- fatal lint と quality self-check を分離
- weak verify / verify-artifact coupling / instruction specificity を quality issue として検出
- Next.js profile expectation を self-check と prompt の両方に接続
- `retryable_quality` を既存 planner retry 上限内で corrective retry に戻す
- retry 悪化時は last valid plan を保持しつつ、attempt が残っていれば lint/schema retry を継続
- eval summary/report に quality issue / retry / degraded count を追加
- verify policy retry prompt に shell control syntax の代替案を追加
  - multi-statement Node check は smoke-check artifact + `node smoke-check.js`
  - docs assertion は `grep -q` を分割して使う

provider 別分岐、eval scenario 名依存、hard lint 化は追加していない。

## テスト結果

ローカルテスト:

- `cargo fmt --manifest-path mvp/anvilminimal/Cargo.toml -- --check`: pass
- `cargo test --manifest-path mvp/anvilminimal/Cargo.toml`: pass
  - 251 unit tests
  - CLI/eval/TUI integration tests
- `python3 -m unittest discover -s mvp/anvilminimal/tests/eval`: pass
  - 75 tests, 1 skipped
- `python3 -m compileall -q mvp/anvilminimal/scripts mvp/anvilminimal/tests/eval`: pass

release build:

- `cargo build --release --manifest-path mvp/anvilminimal/Cargo.toml`: pass

## Live eval 結果

実行条件:

- local LLM 未使用
- `--model-profile speed-cloud`
- binary: `mvp/anvilminimal/target/release/anvilminimal`

### MVP smoke step-plan

Run root:

- `/private/tmp/anvilminimal-eval-008-smoke-step-4`

結果:

- success: 12/12
- overall avg: 86.5
- plan_quality avg: 86.8
- executable_plan avg: 79.5
- constraint_coverage avg: 93.8
- verify_strength avg: 68.6
- artifact_ownership avg: 98.0
- lint_repair avg: 95.2
- planner_quality_issue_count: 6
- planner_retryable_quality_count: 1
- planner_advisory_quality_count: 5
- planner_quality_retry_count: 1
- planner_quality_retry_degraded_count: 0

baseline との差分:

- executable_plan avg: 78.3 -> 79.5
- verify_strength avg: 62.2 -> 68.6
- success: 12/12 維持
- artifact_ownership: 95 以上維持
- lint_repair: 85 以上維持

### MVP blind step-plan

Run root:

- `/private/tmp/anvilminimal-eval-008-blind-step-2`

結果:

- success: 12/12
- overall avg: 86.1
- plan_quality avg: 88.0
- executable_plan avg: 78.8
- constraint_coverage avg: 93.8
- verify_strength avg: 69.8
- artifact_ownership avg: 99.0
- lint_repair avg: 85.6
- planner_quality_issue_count: 6
- planner_retryable_quality_count: 2
- planner_advisory_quality_count: 4
- planner_quality_retry_count: 1
- planner_quality_retry_degraded_count: 0

blind suite でも success は悪化していない。改善が特定 smoke scenario にだけ寄っている兆候は限定的。

### Plan-run predictiveness

Run root:

- `/private/tmp/anvilminimal-eval-008-smoke-predictiveness-1`

結果:

- step-plan: 24/24 success
- plan-run: 1/24 success

plan-run failure kind:

- `tool_validation_error`: 9
- `max_iterations`: 5
- `missing_tool_call`: 5
- `planner_lint_error`: 2
- `verify_command_policy_error`: 1
- `tool_execution_error`: 1
- empty/other: 1

読み取り:

- step-plan の YAML 品質は上がったが、plan-run 成功率を十分には予測できていない。
- high score plan が plan-run で落ちる false positive はまだ多い。
- 今回の修正範囲は step-plan の B/C 改善であり、plan-run 実行力は別途深掘りが必要。

### Ultra smoke

Run root:

- `/private/tmp/anvilminimal-eval-008-ultra-smoke-1`

結果:

- ultra-plan-run: 0/12 success

failure kind:

- `tool_validation_error`: 5
- `planner_schema_error`: 4
- `phase_scaffold_error`: 2
- `planner_lint_error`: 1

読み取り:

- ultra は今回の step-plan 改善だけでは受け入れ条件を満たしていない。
- 失敗は phase scaffold / minimal loop / ultra schema 側に分布しており、今回追加した quality self-check だけで解消する対象ではない。
- `ultra-plan-run smoke が既存より悪化しない` は、今回の run だけでは満たしたと断定できない。別計画で ultra 経路の baseline 比較と修正が必要。

## 定性レビュー

確認した YAML:

- `mvp-smoke__fix-js-date-helper-small__step-plan__gemini...`
- `mvp-smoke__docs-heading-update-small__step-plan__openai...`
- `mvp-smoke__nextjs-space-invaders-large__step-plan__openai...`

所見:

- Next.js plan は `package.json` / `src/app/page.tsx` / `src/app/layout.tsx` / `src/app/global.d.ts` を含み、`npm run build` が verify step に置かれている。
- Node smoke check は当初 `node` と `smoke-check.js` に割れていたが、retry prompt と degraded retry 継続により `node smoke-check.js` へ改善された。
- docs plan は `grep Usage README.md` のように内容確認へ寄ったが、`grep -q` や quoted phrase までは安定していない。これは advisory として残す妥当な弱点。
- eval scenario 名に依存した分岐や固定ケース専用のロジックは追加していない。

## 未達・残課題

- plan-run predictiveness は未達。
  - step-plan 24/24 に対し plan-run 1/24 のため、false positive が大きい。
  - minimal loop 実行時の tool validation / max_iterations / missing tool call を別途対策する必要がある。
- ultra smoke は未達。
  - 0/12 success。
  - phase scaffold / ultra schema / minimal loop 側の対策が必要。
- docs content assertion はまだ弱いケースがある。
  - `grep -q "Usage" README.md` のようなより強い型を advisory/quality retry の追加対象にできるが、hard lint 化は避けるべき。

## 判定

step-plan の `executable_plan avg` と `verify_strength avg` 改善という主目的は達成した。

ただし、Phase 7 の受け入れ条件のうち以下は未達または保留:

- plan-run predictiveness の false positive 改善
- ultra-plan-run smoke の悪化なし確認

これらは step-plan 生成品質の範囲を超えており、次フェーズで plan-run / ultra-run 実行経路として扱う。
