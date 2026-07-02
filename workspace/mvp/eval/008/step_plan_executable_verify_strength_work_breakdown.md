# step-plan executable / verify strength 改善 作業具体化

## 目的

以下2文書の方針を実装可能な作業単位へ分解する。

- `workspace/mvp/eval/008/step_plan_executable_verify_strength_current_state.md`
- `workspace/mvp/eval/008/step_plan_executable_verify_strength_root_cause_countermeasures.md`

今回の主眼は、planner prompt を強くすることではなく、作成済み plan を deterministic に自己チェックし、valid だが弱い plan を限定的に修正ループへ戻すこと。

## 全体方針

作業は次の順序で進める。

1. B: 作成済み plan をチェックする機構を作る
2. C: 高信頼なチェック結果だけ bounded retry に反映する
3. A: planner prompt / profile guidance を最小補強する
4. eval / tests で過剰適応と不安定化を検知する

やらないこと:

- provider 別分岐を増やさない
- eval scenario 名に依存した runtime logic を追加しない
- dev server 起動を step-plan verify に入れない
- 既存 fatal lint を弱めない
- warning を一律 hard lint 化しない

## 変更対象

### Rust runtime

- `mvp/anvilminimal/src/planner/lint.rs`
  - 既存 fatal lint は維持
  - quality self-check を追加または分離
- `mvp/anvilminimal/src/planner/runner.rs`
  - quality issue event 出力
  - retryable quality issue の bounded retry
  - retry 悪化時の last valid plan 保持
  - minimal prompt 補強
- `mvp/anvilminimal/src/planner/profile.rs`
  - profile expectation を quality check に渡す薄い入口
- `mvp/anvilminimal/src/planner/profiles/nextjs.rs`
  - Next.js の expected verification contract を再利用可能にする

### Eval

- `mvp/anvilminimal/scripts/eval_lib/failure_classification.py`
  - 必要に応じて quality retry / quality exhausted を分類
- `mvp/anvilminimal/scripts/eval_lib/report.py`
  - quality issue / retry reason の表示
- `mvp/anvilminimal/scripts/eval_lib/run_summary.py`
  - quality issue count / retry count の集計
- `mvp/anvilminimal/scripts/eval_lib/plan_scoring.py`
  - 必要に応じて scoring と self-check の乖離を確認

### Tests

- `mvp/anvilminimal/src/planner/*` の Rust unit tests
- `mvp/anvilminimal/tests/eval/*` の Python eval tests
- live eval: `mvp-smoke.yaml`, `mvp-blind.yaml`

## Phase 0: baseline と移植元差分確認

### 作業

1. 現在の baseline を文書に残す。
   - MVP: `/tmp/anvilminimal-step-plan-speed-cloud-20260626-005`
   - anvildev: `/tmp/anvildev-step-plan-speed-cloud-20260626-004`
2. low verify / low executable の代表 YAML を確認する。
   - `nextjs-space-invaders-large`
   - `repair-exhausted-report-large`
   - `docs-heading-update-small`
3. 移植元側の差分を確認する。
   - plan 生成後の quality warning / retry 相当機構
   - verify command allowlist / policy
   - profile guidance / profile verify / postcheck の責務分担
   - plan-run に渡す step instruction 補強
4. 確認結果を `workspace/mvp/eval/008/source_parity_notes.md` に出力する。

### 受け入れ条件

- baseline の run path と主要指標が追跡可能
- 移植漏れと policy mismatch が分けて整理されている
- 追加実装が必要な移植元差分があれば、後続 Phase に明示されている

### テスト

- 文書作成のみ
- コード変更なし

## Phase 1: quality self-check のデータ構造を作る

### 作業

1. `PlanQualitySeverity` を定義する。
   - `Fatal`
   - `RetryableQuality`
   - `Advisory`
2. `PlanQualityIssue` を定義する。
   - `category`
   - `message`
   - `severity`
   - `step_id`
   - `evidence`
3. `PlanQualityReport` を定義する。
   - `issues`
   - `has_retryable_quality()`
   - `has_fatal()`
   - `primary_message()`
4. 既存 `PlanLintReport` と混ぜすぎない。
   - fatal lint は既存 `lint_step_plan_report`
   - quality check は別関数または別 report

候補 API:

```rust
pub fn step_plan_quality_report(plan: &StepPlan, context: &PlanQualityContext) -> PlanQualityReport
```

`PlanQualityContext` に含めるもの:

- goal
- profile
- expected profile artifacts
- profile verification expectations

### 受け入れ条件

- fatal lint と quality issue の責務が分離されている
- severity が stable category として event / eval で使える
- provider 依存の情報を持たない

### テスト

- `PlanQualityReport::pass` 相当が作れる
- `has_retryable_quality()` が retryable issue のみで true
- advisory のみでは retry 対象にならない

## Phase 2: weak verify / coupling / instruction specificity の self-check

### 作業

1. weak verify を検出する。
   - `test -f`
   - `test -s`
   - `cat`
   - compile-only
   - generic grep
2. 強い verify を検出する。
   - `python3 -m unittest`
   - `python -m unittest`
   - `cargo test`
   - `npm run build`
   - `pnpm build`
   - `yarn build`
   - task-specific content assertion
3. verify と expected_paths の結合を確認する。
   - verify command が expected artifact に触れているか
   - docs task で要求語句を確認しているか
   - code task で実行・test・build に到達しているか
4. implement instruction の具体性を確認する。
   - expected_paths があるのに instruction に対象 path または内容が出てこない
   - "implement the feature" だけのような抽象指示を advisory または retryable にする

### 分類方針

- `RetryableQuality`
  - framework task で build/test expectation があり、順序条件を満たせるのに strong verify がない
  - code task で挙動検証がまったくない
  - verify が expected_paths と明らかに無関係
- `Advisory`
  - docs task の assertion が弱いが、要求が曖昧
  - instruction がやや抽象的だが expected_paths が明確
- `Fatal`
  - 原則ここでは増やさない
  - 既存 fatal lint に委ねる

### 受け入れ条件

- valid だが弱い plan を `RetryableQuality` または `Advisory` に分類できる
- warning を一律 hard lint にしていない
- eval scenario 名に依存していない

### テスト

- `test -f README.md` のみの docs plan は advisory または retryable
- `grep -q "Usage" README.md` のような content assertion は弱 verify 扱いにしない
- Python code task で unittest なしは retryable
- Python code task で `python3 -m unittest test_x.py` は pass
- Next.js task で `npm run build` なしは retryable
- Verify が unrelated file を見る場合は retryable

## Phase 3: profile expectation self-check

### 作業

1. `profile.rs` に profile expectation 取得 API を追加する。

候補:

```rust
pub fn profile_quality_expectations(profile: &str, goal: &str) -> ProfileQualityExpectations
```

2. `ProfileQualityExpectations` は最小構造にする。
   - required_artifacts
   - preferred_verify
   - forbidden_verify
   - dependency_order_hint
3. Next.js profile で以下を返す。
   - required artifacts: `package.json`, `src/app/page.tsx`, `src/app/layout.tsx`, `src/app/global.d.ts`
   - preferred verify: `npm run build`
   - forbidden verify: `next dev`, `npm install`, shell control syntax
   - order: package manifest and app entrypoint before build
4. quality self-check で profile expectation を参照する。

### 受け入れ条件

- profile expectation が planner prompt だけでなく self-check の根拠になる
- Next.js build verify は setup / entrypoint 後にある場合だけ strong verify とみなす
- dev server 起動を verify に入れない

### テスト

- Next.js plan で package + entrypoint + `npm run build` がある場合 pass
- Next.js plan で `npm run build` が entrypoint 前にある場合は既存 dependency lint が効く
- Next.js plan で build がない場合 retryable
- 3011 port は dev script / profile verification 側の責務として残る

## Phase 4: quality issue event と bounded retry

### 作業

1. `runner.rs` の step-plan generation loop に quality check を追加する。
   - schema parse pass
   - mechanical repair
   - fatal lint pass
   - quality report
   - retryable issue があれば retry
2. last valid plan を保持する。
   - fatal lint を通った plan を `last_valid_plan` として保持
   - quality retry が fatal lint に悪化した場合、retry 結果を採用しない
3. quality retry prompt を追加する。

候補:

```rust
fn build_quality_retry_prompt(
    goal: &str,
    quality_report: &PlanQualityReport,
    attempt: usize,
) -> String
```

4. event を出力する。
   - `planner_quality_issue`
   - `planner_quality_retry`
   - `planner_quality_retry_degraded`
   - `planner_quality_retry_exhausted`
5. retry 回数は既存 planner attempt 上限内に収める。
   - 専用の無制限 loop は作らない
   - retry のために provider call を過度に増やさない

### 採用ルール

- fatal lint がある plan は採用しない
- retryable quality issue が改善された plan は採用する
- retry で fatal lint に悪化した場合は last valid plan を保持する
- quality issue exhausted は初期実装では失敗扱いにしない
- quality issue を失敗扱いへ昇格する場合は、別フェーズで blind eval と plan-run predictiveness の根拠を要求する
- それ以外は valid plan を採用し、advisory event を残す

### 受け入れ条件

- quality retry が既存 retry 上限内で止まる
- retry 悪化時に valid plan を失わない
- event から retry reason と改善有無が追跡できる
- planner_lint_error が増えない
- advisory のみでは provider call を増やさない
- quality issue exhausted だけで YAML 生成成功率を落とさない

### テスト

- 1回目 weak plan、2回目 strong plan の mock client で strong plan が採用される
- 1回目 weak valid plan、2回目 fatal lint plan の mock client で first valid plan が保持される
- advisory のみでは retry しない
- retryable issue が event に出る
- retry exhausted でも無限ループしない
- retry exhausted でも last valid plan が採用される

## Phase 5: prompt / profile guidance の最小補強

### 作業

1. `plan_generation_system_prompt()` を短く補強する。
   - verify は file existence を最後の fallback とする
   - code task は test/build/smoke/compile を優先
   - docs task は要求文言・見出しの content assertion を優先
   - framework task は build を優先
2. `build_step_plan_user_prompt()` に profile verification section を追加する。
   - required artifacts
   - preferred verify
   - dependency order hint
3. `strengthen_step_plan_for_profile()` の後段補強と矛盾しないよう整理する。
   - profile guidance を最後の step だけに閉じ込めない
   - 既存挙動を急に削除しない

### 受け入れ条件

- prompt が肥大化していない
- provider 別分岐がない
- self-check と同じ契約を短く補助している
- eval scenario 名や固定 artifact 文字列へ過適応していない

### テスト

- prompt snapshot または部分一致テスト
- Next.js profile prompt に preferred verify が含まれる
- unknown profile では余計な profile guidance が増えない

## Phase 6: eval reporting / summary 拡張

### 作業

1. ANVIL_EVAL_EVENTS から quality event を集計する。
2. summary に以下を追加する。
   - quality_issue_count
   - retryable_quality_count
   - advisory_quality_count
   - quality_retry_count
   - quality_retry_degraded_count
3. report に lowest quality issue examples を追加する。
4. Plan Run Predictiveness と併せて false positive / false negative を見る。

### 受け入れ条件

- quality retry が成功率や score と別軸で見える
- retry が多い planner を不安定として確認できる
- anvildev policy mismatch と MVP quality issue が混ざらない

### テスト

- fake event file から quality counts を集計できる
- report に quality issue section が出る
- quality event がない既存 run でも report が壊れない

## Phase 7: test / eval 実行

### Unit test

実行候補:

```bash
cargo fmt --manifest-path mvp/anvilminimal/Cargo.toml -- --check
cargo test --manifest-path mvp/anvilminimal/Cargo.toml planner::lint
cargo test --manifest-path mvp/anvilminimal/Cargo.toml planner::runner
cargo test --manifest-path mvp/anvilminimal/Cargo.toml
```

必要に応じて:

```bash
cargo clippy --manifest-path mvp/anvilminimal/Cargo.toml -- -D warnings
```

### Eval script test

実行候補:

```bash
python3 -m unittest discover -s mvp/anvilminimal/tests/eval
python3 -m compileall -q mvp/anvilminimal/scripts/eval_lib mvp/anvilminimal/scripts/eval-run.py
```

### Live eval

スピード重視、ローカル LLM 未使用:

```bash
python3 mvp/anvilminimal/scripts/eval-run.py \
  --suite mvp/anvilminimal/eval/suites/mvp-smoke.yaml \
  --profile speed-cloud \
  --modes step-plan \
  --runs 1 \
  --parallel 4
```

blind:

```bash
python3 mvp/anvilminimal/scripts/eval-run.py \
  --suite mvp/anvilminimal/eval/suites/mvp-blind.yaml \
  --profile speed-cloud \
  --modes step-plan \
  --runs 1 \
  --parallel 4
```

predictiveness:

```bash
python3 mvp/anvilminimal/scripts/eval-run.py \
  --suite mvp/anvilminimal/eval/suites/mvp-smoke.yaml \
  --profile speed-cloud \
  --modes step-plan,plan-run \
  --runs 2 \
  --parallel 4
```

anvildev comparison:

```bash
python3 mvp/anvilminimal/scripts/eval-run.py \
  --suite mvp/anvilminimal/eval/suites/mvp-smoke.yaml \
  --profile speed-cloud \
  --modes step-plan \
  --runs 1 \
  --parallel 4 \
  --binary anvildev \
  --engine minimal
```

ultra smoke:

```bash
python3 mvp/anvilminimal/scripts/eval-run.py \
  --suite mvp/anvilminimal/eval/suites/mvp-smoke.yaml \
  --profile speed-cloud \
  --modes ultra-plan-run \
  --runs 1 \
  --parallel 2
```

### 受け入れ条件

- MVP `mvp-smoke` step-plan success は 12/12 を維持
- blind suite success が悪化しない
- `planner_lint_error` が増えない
- quality retry が既存上限内で止まる
- quality issue exhausted だけで success rate が落ちない
- advisory のみでは追加 provider call が発生しない
- `verify_strength avg` が baseline 62.2 から改善
- `executable_plan avg` が baseline 78.3 から改善
- `artifact_ownership avg` は 95 以上を維持
- `lint_repair avg` は 85 未満に悪化しない
- plan-run predictiveness の false positive が悪化しない
- ultra-plan-run smoke が既存より悪化しない

## Phase 8: 定性レビュー

### 作業

1. low score YAML を目視確認する。
2. retry 前後 YAML を比較する。
3. Next.js の setup -> implement -> verify が自然か確認する。
4. docs task の verify が要求文言を見ているか確認する。
5. code task の verify が実行・test・build に到達しているか確認する。
6. eval scenario 専用の文字列や固定ファイル名に寄っていないか確認する。

### 受け入れ条件

- スコア改善だけでなく、人間が見ても plan が自然
- retry 後 plan が過剰に細かすぎない
- minimal loop に渡す step instruction として実行可能
- blind eval でも同じ改善傾向がある

## Phase 9: ドキュメント更新

### 作業

1. `workspace/mvp/eval/008/step_plan_executable_verify_strength_current_state.md` を更新する。
   - before / after metrics
   - quality issue count
   - retry count
   - plan-run predictiveness
2. `workspace/mvp/eval/008/step_plan_executable_verify_strength_root_cause_countermeasures.md` を更新する。
   - 実装結果
   - 方針変更があれば理由
   - 残課題
3. 必要なら `workspace/mvp/eval/008/implementation_results.md` を追加する。

### 受け入れ条件

- 実装内容、検証結果、残課題が追跡可能
- 数値だけでなく定性レビュー結果も残っている
- 移植漏れ / 移植不備が見つかった場合は明記されている

## 実装順序の推奨

1. Phase 0
2. Phase 1
3. Phase 2
4. Phase 3
5. Phase 4
6. Phase 5
7. Phase 6
8. Phase 7
9. Phase 8
10. Phase 9

Phase 5 の prompt 補強は、Phase 1-4 の self-check / retry ができてから実施する。prompt だけ先に強化すると、今回の根本原因である B/C の弱さが残る。

## 完了判定

完了とみなす条件:

- `quality self-check` が runtime に実装されている
- `retryable_quality` が bounded retry に反映される
- retry 悪化時に valid plan が失われない
- quality issue exhausted だけで step-plan 成功率を落とさない
- profile expectation が self-check と prompt の両方に使われる
- eval report で quality issue / retry が追える
- `cargo fmt --manifest-path mvp/anvilminimal/Cargo.toml -- --check`、Rust unit test、eval script test が通る
- live eval で success / predictiveness / blind が悪化しない
- ultra-plan-run smoke が悪化しない
- YAML の定性確認で eval 過適応が見られない

完了とみなさない条件:

- prompt だけ強化して self-check / retry がない
- `verify_strength` だけ上がり、plan-run false positive が増える
- hard lint 化で成功率が落ちる
- quality retry 失敗時に valid plan を捨てる
- provider 別分岐で特定モデルだけ救う
- eval scenario 名に依存する
