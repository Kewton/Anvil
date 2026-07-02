# step-plan / minimal-loop failure trend repair 作業具体化

作成日: 2026-06-26

入力計画:

- `workspace/mvp/eval/007/step_plan_minimal_loop_failure_trend_investigation.md`

## 目的

直近 eval で確認した `step-plan` と `minimal-loop` の失敗傾向を、MVP の設計思想を崩さずに改善する。

対象は次の 4 層に分ける。

1. `step-plan` lint / retry
   - Python stdlib verifier の dependency-order 誤判定を解消する。
   - retry prompt を単発の primary error ではなく、累積 hard constraints にする。
2. `minimal-loop` completion contract
   - framework app を required paths の存在だけで完了扱いしない。
   - dependency setup を verify から除外しつつ、build requirement を completion authority から消さない。
3. verifier repair feedback
   - verifier failure 後に、失敗 command、target 候補、failure excerpt を feedback に含める。
   - test weakening を誘導せず、実装、セットアップ、テストの authority を確認させる。
4. eval / runtime consistency
   - suite の `expected_artifacts` / `postcheck` / `profile` から生成する completion contract を snapshot 化する。
   - `step-plan`, `minimal-loop`, `plan-run`, `ultra-plan-run`, TUI への影響を確認する。

## 非目的

- eval scenario id 固有の分岐を追加しない。
- `Space Invaders`、`markdown_lint.py`、`test_markdown_lint.py` 固有の補正を runtime に入れない。
- provider 固有の special case を増やさない。
- postcheck command 全体を runtime 内へそのまま埋め込まない。
- `npm install` など network / dependency setup を通常 verify として強制実行しない。
- verifier failure から自動的にテスト期待値を書き換える機構を入れない。
- source の task contract / heavy repair 全体を丸ごと移植しない。

## 対象ファイル

主な実装対象:

- `mvp/anvilminimal/src/planner/lint.rs`
- `mvp/anvilminimal/src/planner/runner.rs`
- `mvp/anvilminimal/src/planner/step_plan.rs`
- `mvp/anvilminimal/src/planner/profiles/nextjs.rs`
- `mvp/anvilminimal/src/planner/profile.rs`
- `mvp/anvilminimal/src/minimal_loop/completion.rs`
- `mvp/anvilminimal/src/minimal_loop/feedback.rs`
- `mvp/anvilminimal/src/minimal_loop/loop_run.rs`
- `mvp/anvilminimal/src/minimal_loop/repair_progress.rs`
- `mvp/anvilminimal/src/eval_events.rs`

eval / script / test 対象:

- `mvp/anvilminimal/scripts/eval-run.py`
- `mvp/anvilminimal/scripts/eval_lib/failure_classification.py`
- `mvp/anvilminimal/scripts/eval_lib/report.py`
- `mvp/anvilminimal/tests/eval/test_eval_run_dry.py`
- `mvp/anvilminimal/tests/eval/test_failure_snapshot_classification.py`
- `mvp/anvilminimal/tests/eval/test_step_plan_source_parity_fixtures.py`
- `mvp/anvilminimal/tests/eval/test_planner_provider_request_fixtures.py`
- `mvp/anvilminimal/tests/eval/test_eval_event_report.py`
- `mvp/anvilminimal/tests/tui_pty.rs`
- 新規候補:
  - `mvp/anvilminimal/tests/eval/fixtures/step_minimal_007/`
  - `mvp/anvilminimal/tests/eval/test_completion_contract_snapshots.py`
  - `mvp/anvilminimal/tests/eval/test_step_minimal_failure_trend_fixtures.py`

参照する移植元:

- `src/agent/minimal_step_runner/plan_lint.rs`
- `src/agent/minimal_step_runner.rs`
- `src/agent/minimal_step_runner/repair.rs`
- `src/agent/minimal_step_runner/verify.rs`
- `src/agent/minimal_step_runner/profiles/nextjs.rs`
- `src/agent/loop_run/task_contract_completion_policy.rs`
- `src/agent/loop_run/completion_evidence.rs`
- `src/agent/loop_run/repair_assertion_analysis.rs`
- `src/agent/loop_run/repair_python_test_analysis.rs`

## フェーズ一覧

| Phase | 対応 | 目的 | 主な成果物 | 完了条件 |
|---|---|---|---|---|
| Phase 0 | P0-P5 | baseline / fixture 固定 | failure fixture, contract snapshot | 直近失敗を local deterministic test で説明できる |
| Phase 1 | P0 | lint policy 修正 | verifier command class 判定 | Python unittest は setup-order lint で落ちない |
| Phase 2 | P3/P5 | completion contract schema | profile/deferred requirement | build requirement が contract から消えない |
| Phase 3 | P2 | Next.js profile gate | static profile verify integration | nextjs required paths only で早期成功しない |
| Phase 4 | P4 | verifier repair feedback | structured feedback | verify failure 後の no-edit 停滞を減らす |
| Phase 5 | P1 | retry prompt 改善 | cumulative hard constraints | 複合 lint failure fixture が 3 attempt 内に収束する |
| Phase 6 | all | classification / report / 横断 tests | unit, integration, Python tests | failure kind が粗くならず regression を拾える |
| Phase 7 | all | eval / TUI 検証 | speed-cloud trend report | `step-plan,minimal-loop` 2 runs で同一原因再発なし |

実装順序は Phase 0 から Phase 7 を基本とする。ただし Phase 5 の retry prompt は LLM 挙動に依存するため、Phase 1 から Phase 4 の deterministic 修正後に実施する。

## 完了条件 / テスト計画レビュー結果

レビュー観点:

- 完了条件が eval 成功率だけに寄っていないか。
- step-plan の「実行できる plan」だけでなく「分解品質」を検証できるか。
- `deferred_verify_requirements` の成功条件が曖昧なまま残っていないか。
- `plan-run` / `ultra-plan-run` / `ultra-step-run` / TUI へ横展開できるか。
- 移植元 `anvildev` との比較で、MVP 側だけの過剰制約や緩和を検出できるか。
- eval harness、report、redaction、failure classification 自体の退行を拾えるか。

不足していた点:

1. `step-plan` は lint pass だけでは不十分で、plan quality score、責務分界、verify coverage の退行検知が必要だった。
2. `deferred_verify_requirements` は「残す」と書いていたが、どの状態なら completion success とみなせるかが弱かった。
3. `ultra-step-run` は影響範囲には入っていたが、作業分解の横断 smoke に含まれていなかった。
4. `anvildev` 比較は eval harness で可能だが、完了条件に入っていなかった。
5. `.env` key 不在時の live check skip、`--scenario` 単一指定、redaction、report schema のテストが明示不足だった。
6. `cargo fmt` / `clippy` 相当の静的品質 gate が最終検証に含まれていなかった。

反映内容:

- Phase 0 に plan quality / eval harness / source comparison fixture を追加する。
- Phase 2 に deferred requirement の状態遷移と success semantics の negative test を追加する。
- Phase 5 に plan quality score の受け入れ条件を追加する。
- Phase 6/7 に `ultra-step-run`、`anvildev` dry-run/targeted comparison、report/redaction/schema の検証を追加する。
- 最終検証に `cargo fmt --manifest-path ... -- --check` と `cargo clippy` を条件付きで追加する。

## Phase 0: Baseline fixture / contract snapshot 固定

### 目的

live LLM の揺れに依存せず、今回の失敗形状と completion contract の不足を local test で再現できるようにする。

### 作業

| ID | タスク | 対象 |
|---|---|---|
| P0-1 | 直近 eval run の失敗 summary を distilled fixture にする | `mvp/anvilminimal/tests/eval/fixtures/step_minimal_007/` |
| P0-2 | `step-plan` の Python unittest dependency-order 失敗 fixture を追加 | `test_step_minimal_failure_trend_fixtures.py` |
| P0-3 | `step-plan` の shell control / duplicate ownership retry 横滑り fixture を追加 | `test_step_minimal_failure_trend_fixtures.py` |
| P0-4 | `minimal-loop` の Next.js artifact-only completion fixture を追加 | `test_completion_contract_snapshots.py` |
| P0-5 | `minimal-loop` の verifier no-edit repair fixture を追加 | `test_failure_snapshot_classification.py` |
| P0-6 | `expected_artifacts` / `postcheck` / `profile` から生成される contract snapshot を追加 | `test_completion_contract_snapshots.py` |
| P0-7 | fixture に secret、API key、absolute home path が含まれないことを検査 | fixture review / redaction test |
| P0-8 | 旧 fixture の分類結果が変わらないことを確認 | `test_failure_snapshot_classification.py` |
| P0-9 | step-plan quality score の baseline fixture を追加 | `test_plan_scoring.py`, `test_step_minimal_failure_trend_fixtures.py` |
| P0-10 | `--scenario` が単一 ID 指定であることを eval dry-run test に固定 | `test_eval_run_dry.py` |
| P0-11 | `anvilminimal` / `anvildev` の eval command rendering 差分を dry-run fixture 化 | `test_eval_run_dry.py` |

fixture 案:

- `step_minimal_007/step_plan_python_unittest_dependency_order.json`
- `step_minimal_007/step_plan_retry_shell_control_duplicate_ownership.json`
- `step_minimal_007/minimal_loop_nextjs_artifact_only_completion.json`
- `step_minimal_007/minimal_loop_verify_repair_no_change.json`
- `step_minimal_007/completion_contract_nextjs_expected.json`
- `step_minimal_007/completion_contract_python_unittest_expected.json`
- `step_minimal_007/completion_contract_docs_only_expected.json`
- `step_minimal_007/step_plan_quality_baseline.json`
- `step_minimal_007/eval_dry_run_binary_kind_matrix.json`

### テスト

```bash
python3 -m unittest mvp/anvilminimal/tests/eval/test_failure_snapshot_classification.py
python3 -m unittest mvp/anvilminimal/tests/eval/test_completion_contract_snapshots.py
python3 -m unittest mvp/anvilminimal/tests/eval/test_step_minimal_failure_trend_fixtures.py
python3 -m unittest mvp/anvilminimal/tests/eval/test_eval_run_dry.py
python3 -m unittest mvp/anvilminimal/tests/eval/test_plan_scoring.py
```

### 受け入れ条件

- 直近の `step-plan` 失敗 3 件と `minimal-loop` 失敗 3 件を fixture で説明できる。
- Next.js scenario の contract snapshot で `npm run build` requirement が消えている現状を再現できる。
- Python unittest scenario の contract snapshot で verify command が残るべきことを表現できる。
- docs-only scenario は verify なしでも artifact completion を許す期待が snapshot 化されている。
- step-plan の score baseline があり、lint pass だが低品質な plan を検出できる。
- `anvilminimal` と `anvildev` の eval command rendering 差分が dry-run で説明できる。
- `--scenario` 複数 ID 一括指定を前提にした手順が残らない。
- fixture は API key、token、absolute home path、巨大な generated app 一式を含まない。
- この Phase では runtime の挙動を変更しない。

## Phase 1: planner lint policy を source parity 寄りに修正

### 目的

`python3 -m unittest ...` を dependency setup 必須 verify と誤分類しない。

### 作業

| ID | タスク | 対象 |
|---|---|---|
| P1-1 | `is_build_verify()` を分割し、setup-order 対象を明示する | `planner/lint.rs` |
| P1-2 | `python -m unittest` / `python3 -m unittest` を setup-order 対象外にする | `planner/lint.rs` |
| P1-3 | `pytest` / `cargo test` は plan lint で一律 setup 必須にしない | `planner/lint.rs` |
| P1-4 | npm/pnpm/yarn build/test は setup/order 不備を引き続き検出する | `planner/lint.rs` |
| P1-5 | shell control syntax 禁止、setup-in-verify 禁止、duplicate ownership 禁止は維持する | `planner/lint.rs` |
| P1-6 | lint error message を eval 分類しやすい形に保つ | `planner/lint.rs`, `failure_classification.py` |
| P1-7 | source lint との差分をコメントではなく test 名で追跡する | Rust unit tests |

実装メモ:

- `is_build_verify()` をそのまま広義の verifier 判定として使い続けると再発しやすい。
- 役割を分ける候補:
  - `is_dependency_setup_command(cmd)`
  - `requires_dependency_setup_before_verify(cmd)`
  - `is_verify_command(cmd)`
  - `is_shell_control_syntax_forbidden(cmd)`
- setup-order lint は初期対応では npm/pnpm/yarn に絞る。
- Python dependency missing は plan lint ではなく、実行時の verifier/precondition feedback で扱う。

### テスト

```bash
cargo test --manifest-path mvp/anvilminimal/Cargo.toml planner_lint_python_unittest_without_setup_passes
cargo test --manifest-path mvp/anvilminimal/Cargo.toml planner_lint_python3_unittest_without_setup_passes
cargo test --manifest-path mvp/anvilminimal/Cargo.toml planner_lint_pytest_is_not_dependency_order_failure
cargo test --manifest-path mvp/anvilminimal/Cargo.toml planner_lint_cargo_test_is_not_dependency_order_failure
cargo test --manifest-path mvp/anvilminimal/Cargo.toml planner_lint_npm_build_requires_setup_or_manifest
cargo test --manifest-path mvp/anvilminimal/Cargo.toml planner_lint_shell_control_still_fails
cargo test --manifest-path mvp/anvilminimal/Cargo.toml planner_lint_duplicate_expected_path_still_fails
```

### 受け入れ条件

- `python3 -m unittest test_*.py` は setup step なしでも lint pass する。
- `npm run build` / `npm test` / `npm run test` は package/setup の不備を引き続き検出する。
- `pytest` / `cargo test` は plan lint の setup-order failure にならない。
- shell control syntax と duplicate expected path ownership は緩まない。
- scenario id、suite 名、特定ファイル名による分岐がない。

## Phase 2: completion contract に profile / deferred requirement を追加

### 目的

dependency setup を verify command から除外しつつ、`npm run build` などの completion authority を contract から消さない。

### 作業

| ID | タスク | 対象 |
|---|---|---|
| P2-1 | `CompletionContract` に profile / deferred verify requirement を追加する | `minimal_loop/completion.rs` |
| P2-2 | eval contract generator で dependency setup command を verify から除外し続ける | `scripts/eval-run.py` |
| P2-3 | eval contract generator で npm build requirement を deferred metadata として保持する | `scripts/eval-run.py` |
| P2-4 | Python unittest verify は `verify_commands` に残す | `scripts/eval-run.py` |
| P2-5 | docs-only / generic artifact task は verify なし completion を維持する | `scripts/eval-run.py`, `completion.rs` |
| P2-6 | `deferred_verify_requirements` の success semantics を明文化し test で固定する | `completion.rs`, docs in test |
| P2-7 | eval event に contract shape summary を出す | `eval_events.rs`, `eval-run.py` |
| P2-8 | failure classification に deferred/profile contract failure を追加する | `failure_classification.py` |

`deferred_verify_requirements` の最小 schema 案:

```json
{
  "command": "npm run build",
  "reason": "requires dependency setup",
  "authority": "postcheck",
  "profile": "nextjs"
}
```

success semantics:

- `verify_commands` は loop 内で実行可能な deterministic verifier。
- `deferred_verify_requirements` は loop 内で直接実行しない可能性があるが、成功判定から消してはいけない requirement。
- profile verify が存在する場合は、required paths が揃っても profile verify または deferred requirement risk が解消されるまで artifact-only success にしない。
- deferred requirement は `pending`, `covered_by_static_profile_check`, `blocked_by_dependency_setup`, `failed_profile_substitute` のような状態を持つ。`pending` / `blocked_by_dependency_setup` のまま成功扱いにしてはいけない。
- build requirement を runtime で実行しない場合でも、static profile check が代替 evidence として何を確認したかを event/report に残す。
- generic/docs-only task で deferred requirement が空なら、required paths による early success を許す。

### テスト

```bash
python3 -m unittest mvp/anvilminimal/tests/eval/test_completion_contract_snapshots.py
cargo test --manifest-path mvp/anvilminimal/Cargo.toml completion_contract_accepts_deferred_verify_requirements
cargo test --manifest-path mvp/anvilminimal/Cargo.toml completion_contract_rejects_setup_command_as_verify
cargo test --manifest-path mvp/anvilminimal/Cargo.toml completion_contract_docs_only_allows_artifact_completion
cargo test --manifest-path mvp/anvilminimal/Cargo.toml completion_contract_nextjs_deferred_build_prevents_plain_artifact_success
cargo test --manifest-path mvp/anvilminimal/Cargo.toml completion_contract_pending_deferred_requirement_blocks_success
cargo test --manifest-path mvp/anvilminimal/Cargo.toml completion_contract_profile_substitute_records_covered_deferred_requirement
```

### 受け入れ条件

- `npm install --ignore-scripts` は `verify_commands` に混入しない。
- `npm run build` requirement は `verify_commands: []` によって完全には消えない。
- Python unittest scenario では `python3 -m unittest ...` が loop 内 verify command として残る。
- docs-only scenario は不要な profile/deferred verify を要求されない。
- contract snapshot が `expected_artifacts` / `postcheck` / `profile` の関係を維持する。
- `pending` または `blocked_by_dependency_setup` の deferred requirement を抱えた状態で artifact-only success しない。
- static profile check が代替 evidence になる場合、どの deferred requirement を cover したか event/report で追跡できる。

## Phase 3: Next.js profile verify を minimal-loop completion gate に接続

### 目的

`--profile nextjs` または Next.js project shape の task を、required paths の存在だけで完了扱いしない。

### 作業

| ID | タスク | 対象 |
|---|---|---|
| P3-1 | `CompletionContract` の profile 情報を minimal loop へ渡す | `minimal_loop/loop_run.rs`, CLI/eval path |
| P3-2 | required paths 満了後に static profile verify を呼ぶ gate を追加する | `minimal_loop/loop_run.rs` |
| P3-3 | 既存 Next.js profile verify を minimal-loop から再利用できる API に整える | `planner/profiles/nextjs.rs` |
| P3-4 | package/scripts/app entrypoint/global css/tsconfig の整合性 check を追加または確認する | `planner/profiles/nextjs.rs` |
| P3-5 | Next.js version / TypeScript / `@types/*` / `moduleResolution` の明らかな build risk を検出する | `planner/profiles/nextjs.rs` |
| P3-6 | static に判定不能な build requirement は deferred risk として feedback する | `completion.rs`, `feedback.rs` |
| P3-7 | profile verify failure を LLM feedback に変換する | `minimal_loop/feedback.rs` |
| P3-8 | profile 未指定の generic TUI task では gate を無効にする | `loop_run.rs`, TUI integration |

実装メモ:

- profile gate は `nextjs-space-invaders-large` ではなく `profile=nextjs` と project shape に基づく。
- port 3011 は goal/profile guidance の一部として扱う。completion gate は指定 port の dev script / command shape までは確認してよいが、Space Invaders 固有要素は見ない。
- static profile verify が `npm run build` の完全代替になるとは扱わない。検出不能な場合は deferred build requirement を残し、成功条件の判断に使う。

### テスト

```bash
cargo test --manifest-path mvp/anvilminimal/Cargo.toml nextjs_profile_verify_detects_missing_build_script
cargo test --manifest-path mvp/anvilminimal/Cargo.toml nextjs_profile_verify_detects_missing_app_entrypoint
cargo test --manifest-path mvp/anvilminimal/Cargo.toml nextjs_profile_verify_detects_module_resolution_build_risk
cargo test --manifest-path mvp/anvilminimal/Cargo.toml nextjs_profile_verify_detects_dependency_version_risk
cargo test --manifest-path mvp/anvilminimal/Cargo.toml minimal_loop_nextjs_required_paths_only_does_not_complete
cargo test --manifest-path mvp/anvilminimal/Cargo.toml minimal_loop_generic_required_paths_can_complete_without_profile
```

### 受け入れ条件

- Next.js profile task は required paths が揃っただけでは `required_artifacts_satisfied_after_tool` にならない。
- profile verify failure が model に feedback される。
- invalid `package.json`、missing `scripts.build`、missing `src/app/page.tsx`、明らかな tsconfig/dependency build risk を検出する。
- generic/docs-only task の early success は維持される。
- profile gate は TUI 通常入力に不要な制約をかけない。

## Phase 4: verifier failure feedback を source repair prompt に寄せる

### 目的

verifier failure 後に model が Read だけで停滞し、`verify_repair_no_change` へ落ちる確率を下げる。

### 作業

| ID | タスク | 対象 |
|---|---|---|
| P4-1 | verifier failure feedback に failing command を含める | `minimal_loop/feedback.rs` |
| P4-2 | stderr/stdout から bounded failure excerpt を抽出する | `completion.rs` または `feedback.rs` |
| P4-3 | command/output から target file candidates を抽出する | `feedback.rs` |
| P4-4 | assertion mismatch の actual/expected excerpt を feedback に含める | `feedback.rs` |
| P4-5 | implementation/setup/test の authority を確認する文言を追加する | `feedback.rs` |
| P4-6 | test weakening を促す文言を negative test で禁止する | Rust unit tests |
| P4-7 | 2 回連続 no-edit で停止する existing guard は維持する | `repair_progress.rs`, `loop_run.rs` |
| P4-8 | feedback event に sanitized command / target summary を出す | `eval_events.rs` |

feedback の方向性:

- 失敗 command を明示する。
- 関連しそうな小さい file range を Read し、必要なら Edit/Write で最小修正するよう促す。
- 実装が authority なら実装を直す。
- setup/package が authority なら setup/package を直す。
- 生成テスト自体がユーザー要求と矛盾している場合だけ、テスト側の修正を検討する。
- assertion を弱める、期待値を actual に合わせる、skip にする、などは促さない。

### テスト

```bash
cargo test --manifest-path mvp/anvilminimal/Cargo.toml verify_feedback_includes_failing_command
cargo test --manifest-path mvp/anvilminimal/Cargo.toml verify_feedback_includes_target_file_candidates
cargo test --manifest-path mvp/anvilminimal/Cargo.toml verify_feedback_includes_assertion_excerpt
cargo test --manifest-path mvp/anvilminimal/Cargo.toml verify_feedback_does_not_suggest_test_weakening
cargo test --manifest-path mvp/anvilminimal/Cargo.toml verify_repair_no_change_guard_still_stops_after_two_no_edit_turns
python3 -m unittest mvp/anvilminimal/tests/eval/test_failure_snapshot_classification.py
```

### 受け入れ条件

- unittest assertion failure feedback に command、target candidate、failure excerpt が入る。
- feedback は編集を促すが、テスト弱体化を促さない。
- verifier failure を成功扱いしない。
- no-edit guard は引き続き機能する。
- provider/model 固有文言に依存しない。

## Phase 5: lint retry prompt を累積 hard constraints 化

### 目的

planner retry が shell control、duplicate ownership、dependency-order の間で横滑りする状態を減らす。

### 作業

| ID | タスク | 対象 |
|---|---|---|
| P5-1 | lint retry attempt ごとの categories を累積保存する | `planner/runner.rs` |
| P5-2 | retry prompt builder に cumulative hard constraints を渡す | `planner/runner.rs` |
| P5-3 | shell control syntax は複数 verify item へ分割する例を入れる | `planner/runner.rs` |
| P5-4 | duplicate ownership は owner step 一意、verify step expected_paths 空を促す | `planner/runner.rs` |
| P5-5 | setup/implement/verify/report の責務分離を同時に再提示する | `planner/runner.rs` |
| P5-6 | provider request fixture を更新し、provider 固有分岐がないことを確認する | `test_planner_provider_request_fixtures.py` |
| P5-7 | OpenAI/Gemini live planner check の手順と skip 条件を追加する | test / report docs |
| P5-8 | plan quality score と warning の退行を検知する | `plan_scoring.py`, `test_plan_quality_report.py` |

retry prompt hard constraints 候補:

- Top-level `goal` must preserve the original user goal.
- Each implementation-owned expected path must have exactly one owner step.
- Verify-only steps must not claim new expected paths.
- Verify commands must be a single command without shell control syntax.
- Split multiple checks into multiple verify steps.
- Dependency setup steps must install or prepare dependencies, but verify steps must not run setup.
- Python stdlib `unittest` does not require dependency setup by itself.
- Do not add scenario-specific paths unless required by the user goal or artifacts.

### テスト

```bash
cargo test --manifest-path mvp/anvilminimal/Cargo.toml retry_prompt_accumulates_lint_categories
cargo test --manifest-path mvp/anvilminimal/Cargo.toml retry_prompt_mentions_shell_control_split
cargo test --manifest-path mvp/anvilminimal/Cargo.toml retry_prompt_mentions_duplicate_ownership_once
cargo test --manifest-path mvp/anvilminimal/Cargo.toml retry_prompt_keeps_provider_neutral_contract
python3 -m unittest mvp/anvilminimal/tests/eval/test_planner_provider_request_fixtures.py
python3 -m unittest mvp/anvilminimal/tests/eval/test_plan_quality_report.py
python3 -m unittest mvp/anvilminimal/tests/eval/test_plan_scoring.py
```

optional live check:

`--scenario` は単一 ID のみ指定できるため、以下を対象 scenario ごとに実行する。

- `docs-heading-update-small`
- `python-markdown-linter-medium`
- `repair-exhausted-report-large`

```bash
python3 mvp/anvilminimal/scripts/eval-run.py \
  --suite mvp/anvilminimal/eval/suites/mvp-smoke.yaml \
  --model-profiles mvp/anvilminimal/eval/model_profiles.yaml \
  --model-profile speed-cloud \
  --modes step-plan \
  --runs 1 \
  --parallel 2 \
  --scenario docs-heading-update-small \
  --run-root /tmp/anvilminimal-eval-007-step-plan-prompt-live \
  --binary mvp/anvilminimal/target/release/anvilminimal
```

### 受け入れ条件

- retry prompt に過去 attempt の lint categories が累積される。
- shell control / duplicate ownership / dependency-order の複合 fixture が 3 attempt 以内に valid plan へ収束する。
- prompt は provider neutral で、OpenAI/Gemini/Ollama 固有の枝を増やさない。
- `.env` に `OPENAI_API_KEY` / `GEMINI_API_KEY` がある場合は live planner check を行う。
- key がない場合は fixture test を必須にし、live check skipped を report に明記する。
- prompt だけで不安定な場合は deterministic normalizer / plan repair を別計画として判断記録する。
- successful step-plan には `plan_quality_score` が記録される。
- plan quality score が baseline より大きく退行する場合は、成功率が改善していても完了扱いにしない。目安として deterministic fixture は 70 点以上、live eval は同一 scenario の直近 baseline から 5 点超の低下を要調査にする。

## Phase 6: classification / report / 横断 tests

### 目的

今回の修正で failure classification が粗くならず、plan-run / ultra-plan-run / TUI への波及も検知できるようにする。

### 作業

| ID | タスク | 対象 |
|---|---|---|
| P6-1 | deferred/profile completion failure の classification を追加する | `failure_classification.py` |
| P6-2 | planner lint / verify command policy の分類を維持する | `failure_classification.py` |
| P6-3 | eval report に contract shape summary を出す | `eval_lib/report.py` |
| P6-4 | eval event に rawではない sanitized command / target summary を出す | `eval_events.rs`, `report.py` |
| P6-5 | `plan-run` が Phase 1/2/4 の変更で退行しない fixture を追加する | Python/Rust integration |
| P6-6 | `ultra-plan-run` の Next.js profile path が profile/deferred requirement を落とさないことを確認する | Python/Rust integration |
| P6-7 | TUI profile 未指定 path の generic completion を確認する | `tests/tui_pty.rs` |
| P6-8 | TUI slash `/ultra-plan-run --profile nextjs ...` の profile contract propagation を確認する | `tests/tui_pty.rs` or manual smoke |
| P6-9 | `ultra-step-run` の phase replay command rendering と placeholder を dry-run で確認する | `test_eval_run_dry.py` |
| P6-10 | `anvildev` 比較用の `--binary-kind anvildev` dry-run と targeted smoke 手順を固定する | `test_eval_run_dry.py` |
| P6-11 | report schema / redaction が contract summary と feedback excerpt を安全に扱うことを確認する | `test_eval_event_report.py`, `test_summary_schema.py` |

### テスト

```bash
python3 -m unittest discover -s mvp/anvilminimal/tests/eval
cargo test --manifest-path mvp/anvilminimal/Cargo.toml
cargo test --manifest-path mvp/anvilminimal/Cargo.toml --test tui_pty
```

targeted mode smoke:

```bash
python3 mvp/anvilminimal/scripts/eval-run.py \
  --suite mvp/anvilminimal/eval/suites/mvp-smoke.yaml \
  --model-profiles mvp/anvilminimal/eval/model_profiles.yaml \
  --model-profile speed-cloud \
  --modes plan-run,ultra-plan-run,ultra-step-run \
  --runs 1 \
  --parallel 2 \
  --run-root /tmp/anvilminimal-eval-007-plan-ultra-smoke \
  --binary mvp/anvilminimal/target/release/anvilminimal
```

source binary dry-run:

```bash
python3 mvp/anvilminimal/scripts/eval-run.py \
  --suite mvp/anvilminimal/eval/suites/mvp-smoke.yaml \
  --model-profiles mvp/anvilminimal/eval/model_profiles.yaml \
  --model-profile speed-cloud \
  --modes step-plan,minimal-loop,plan-run,ultra-plan-run,ultra-step-run \
  --runs 1 \
  --parallel 1 \
  --binary anvildev \
  --binary-kind anvildev \
  --dry-run
```

### 受け入れ条件

- `unclassified_process_failure` へ新規 failure が落ちない。
- `planner_lint_error`, `verify_command_policy_error`, `postcheck_failure`, `verify_repair_no_change` の既存分類が退化しない。
- deferred/profile failure が summary/report で識別できる。
- plan-run / ultra-plan-run の targeted smoke で、step-plan lint と minimal-loop completion の変更が致命的退行を起こしていない。
- ultra-step-run の dry-run command rendering が phase replay 前提を壊していない。
- `anvildev` 比較用 dry-run が通り、MVP 固有の eval harness 変更で source binary 経路を壊していない。
- TUI generic path は profile gate の影響を受けない。
- TUI slash command の `--profile nextjs` は profile/deferred requirement を落とさない。
- report schema と redaction test が通り、feedback excerpt や command summary に secret/home path が漏れない。

## Phase 7: eval trend / 最終検証

### 目的

local LLM 未使用の speed-cloud 条件で、今回の同一失敗傾向が再発しないことを確認する。

### 事前 build

```bash
cargo build --release --manifest-path mvp/anvilminimal/Cargo.toml
```

### 必須テスト

```bash
cargo fmt --manifest-path mvp/anvilminimal/Cargo.toml -- --check
cargo clippy --manifest-path mvp/anvilminimal/Cargo.toml --all-targets -- -D warnings
cargo test --manifest-path mvp/anvilminimal/Cargo.toml
python3 -m unittest discover -s mvp/anvilminimal/tests/eval
```

`cargo fmt --manifest-path ... -- --check` が環境の Cargo で受け付けられない場合は、`cd mvp/anvilminimal && cargo fmt -- --check` で実行する。

### eval smoke

`step-plan,minimal-loop` を 2 回実行する。

```bash
python3 mvp/anvilminimal/scripts/eval-run.py \
  --suite mvp/anvilminimal/eval/suites/mvp-smoke.yaml \
  --model-profiles mvp/anvilminimal/eval/model_profiles.yaml \
  --model-profile speed-cloud \
  --modes step-plan,minimal-loop \
  --runs 2 \
  --parallel 4 \
  --context-budget 65536 \
  --run-root /tmp/anvilminimal-eval-007-step-minimal \
  --timeout-sec 1800 \
  --binary mvp/anvilminimal/target/release/anvilminimal
```

必要に応じて追加で 1 回 trend 確認する。

```bash
python3 mvp/anvilminimal/scripts/eval-run.py \
  --suite mvp/anvilminimal/eval/suites/mvp-smoke.yaml \
  --model-profiles mvp/anvilminimal/eval/model_profiles.yaml \
  --model-profile speed-cloud \
  --modes step-plan,minimal-loop \
  --runs 1 \
  --parallel 4 \
  --context-budget 65536 \
  --run-root /tmp/anvilminimal-eval-007-step-minimal-repeat \
  --timeout-sec 1800 \
  --binary mvp/anvilminimal/target/release/anvilminimal
```

### 横断 smoke

```bash
python3 mvp/anvilminimal/scripts/eval-run.py \
  --suite mvp/anvilminimal/eval/suites/mvp-smoke.yaml \
  --model-profiles mvp/anvilminimal/eval/model_profiles.yaml \
  --model-profile speed-cloud \
  --modes plan-run,ultra-plan-run,ultra-step-run \
  --runs 1 \
  --parallel 2 \
  --context-budget 65536 \
  --run-root /tmp/anvilminimal-eval-007-plan-ultra \
  --timeout-sec 1800 \
  --binary mvp/anvilminimal/target/release/anvilminimal
```

### TUI smoke

profile 未指定:

```bash
anvilminimal --yes --context-budget 65536 --model gpt-5.4-mini --provider openai
```

slash command:

```text
/ultra-plan-run --profile nextjs あなたが考える最高に面白くかっこいいスペースインベーダーゲームを3011ポートで起動可能なnext.jsアプリとして開発してください。
```

### 受け入れ条件

- `step-plan,minimal-loop` 2 runs で、今回の直接原因が同一形で再発しない。
  - Python unittest が dependency-order lint で落ちない。
  - Next.js profile task が required paths の存在だけで完了しない。
  - `npm run build` requirement が completion contract から完全に消えない。
  - verifier failure 後の feedback に command/target/excerpt が含まれる。
- `plan-run,ultra-plan-run` targeted smoke で、新しい profile/deferred contract が破綻しない。
- `ultra-step-run` dry-run または targeted smoke で phase replay 前提が破綻しない。
- TUI generic path と slash command path の両方で profile propagation が確認される。
- failure summary に新しい `unclassified_process_failure` が増えない。
- eval 成功率だけでなく、unit/fixture の negative tests が通っている。
- plan quality score が記録され、deterministic fixture baseline を下回らない。
- `anvildev` dry-run が通り、source binary 評価経路が壊れていない。

## 横断レビュー観点

実装後、次の観点でセルフレビューする。

| 観点 | 確認内容 |
|---|---|
| 設計思想 | small loop / deterministic verify / YAML plan を維持しているか |
| 不安定性 | network/dependency setup を runtime verify に強制していないか |
| 影響範囲 | plan-run / ultra-plan-run / TUI に profile/deferred contract が伝播するか |
| 複雑性 | source heavy-loop 全体を持ち込まず、必要最小の contract / feedback に留めているか |
| 原因深掘り | failure kind ではなく、直接原因と completion authority の欠落を test で固定しているか |
| 移植漏れ | source lint、profile verify、repair prompt、completion evidence のうち今回必要な最小要素を反映しているか |
| 過剰適応 | scenario id、特定ファイル名、provider 固有条件で分岐していないか |
| テスト妥当性 | eval smoke だけでなく unit / fixture / negative tests があるか |
| plan品質 | lint pass だけでなく、分解粒度、責務分界、verify coverage の score が退行していないか |
| source比較 | `anvildev` 経路の dry-run/targeted smoke と矛盾しないか |
| observability | event/report/redaction が failure diagnosis に必要な情報を安全に残しているか |

## 未完了判定

次のいずれかが残る場合は完了扱いにしない。

- `python3 -m unittest ...` が dependency-order lint で落ちる。
- shell control syntax や duplicate ownership が緩んで通る。
- Next.js profile task が required paths だけで `required_artifacts_satisfied_after_tool` になる。
- `npm run build` requirement が completion contract から完全に消える。
- dependency setup command が verify command に混入する。
- docs-only / generic task が不要な build/profile verify を要求される。
- verifier feedback が test assertion の弱体化を促す。
- retry prompt 変更が fixture で再現できず、live check でも効果が確認できない。
- `plan-run` / `ultra-plan-run` / TUI の影響確認が未実施。
- `ultra-step-run` の dry-run または targeted smoke が未実施。
- `anvildev` 比較用 dry-run が未実施。
- plan quality score が空、または deterministic baseline から退行している。
- report/redaction/schema の regression test が不足している。
- eval smoke は通るが、unit/fixture の negative tests が不足している。

## 実施後の成果物

実装完了時に残す成果物:

- 更新済み Rust / Python tests。
- `workspace/mvp/eval/007/` 配下の実行結果 summary。
- speed-cloud `step-plan,minimal-loop` 2 runs の結果。
- targeted `plan-run,ultra-plan-run` smoke の結果。
- `ultra-step-run` dry-run または targeted smoke の結果。
- `anvildev` dry-run または targeted comparison の結果。
- TUI smoke の確認結果。
- plan quality score / warning summary。
- live provider check を実施した場合は結果。key 不在で skipped の場合はその旨。
