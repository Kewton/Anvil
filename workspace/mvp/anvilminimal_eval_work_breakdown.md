# anvilminimal MVP Eval 作業具体化

入力計画: `workspace/mvp/anvilminimal_eval_design.md`

この文書は、eval 設計を実装可能な作業単位へ分解した作業台帳。成果物は `mvp/anvilminimal` 配下に置き、core runtime とは分離する。既存の `scripts/bench.sh` / `benchmarks/minimal-loop-expanded.yaml` 互換は壊さない。

## 作業方針

- eval harness は Python script と YAML fixture として実装し、Rust CLI 本体へ runtime dependency を増やさない
- 既存 `summary.tsv` は維持し、新評価は `summary.eval.tsv` / `events.jsonl` に出す
- local LLM を含む run は default serial、cloud-only run は provider semaphore 付きで parallel
- `npm run dev` のような long-running process は `dev_server` として扱い、readiness 後に必ず停止する
- plan score は deterministic scorer を primary とし、LLM judge は初期実装に含めない
- `ultra-step-run` は phase 開始前 snapshot から replay し、snapshot が無い場合は skipped として成功率に混ぜない
- live provider / network を使う検証は通常 unit test に混ぜず、明示コマンドで実行する

## 成果物

| 種別 | path | 内容 |
|---|---|---|
| eval config | `mvp/anvilminimal/eval/model_profiles.yaml` | local/cloud/speed/full の model matrix |
| suite | `mvp/anvilminimal/eval/suites/*.yaml` | `mvp-smoke`, `mvp-balanced`, `mvp-full` |
| scoring | `mvp/anvilminimal/eval/scoring_rules.yaml`, `plan_score_schema.yaml` | plan scoring weights/guardrails |
| fixtures | `mvp/anvilminimal/eval/fixtures/**` | scorer/postcheck/compare test data |
| scripts | `mvp/anvilminimal/scripts/eval-*.py` | eval entrypoints |
| script library | `mvp/anvilminimal/scripts/eval_lib/*.py` | shared implementation |
| script tests | `mvp/anvilminimal/tests/eval/*.py` | Python unit/smoke tests |
| docs | `mvp/anvilminimal/eval/README.md` | eval 実行方法 |
| optional Rust test | `mvp/anvilminimal/tests/eval_script_contract.rs` | script/suite schema smoke |

## ディレクトリ構造

```text
mvp/anvilminimal/
  eval/
    README.md
    model_profiles.yaml
    plan_score_schema.yaml
    scoring_rules.yaml
    suites/
      mvp-smoke.yaml
      mvp-balanced.yaml
      mvp-full.yaml
    fixtures/
      plans/
        good-step-plan.yaml
        bad-overlong-step-plan.yaml
        bad-path-escape-step-plan.yaml
        good-ultra-plan.yaml
      postcheck/
        nextjs-dev-server.yaml
      summaries/
        baseline.summary.eval.tsv
        experiment.summary.eval.tsv
  scripts/
    eval-run.py
    eval-preflight.py
    eval-postcheck.py
    eval-score-plan.py
    eval-compare.py
    eval-report.py
    eval_lib/
      __init__.py
      artifacts.py
      config.py
      matrix.py
      models.py
      plan_scoring.py
      postcheck.py
      process.py
      report.py
      run_summary.py
      suites.py
  tests/
    eval/
      test_eval_run_dry.py
      test_model_profiles.py
      test_plan_scoring.py
      test_postcheck_dev_server.py
      test_summary_schema.py
```

## 全体の実行順

1. Phase E0: eval scaffold / schema / docs
2. Phase E1: model profiles / suite loader / matrix 展開
3. Phase E2: preflight
4. Phase E3: dry-run / scheduler / artifact root
5. Phase E4: plan scoring
6. Phase E5: postcheck / dev server lifecycle
7. Phase E6: live execution / metrics / summary
8. Phase E7: ultra artifact / phase snapshot / ultra-step replay
9. Phase E8: report / compare
10. Phase E9: acceptance / regression / documentation

依存関係。

| task | depends on |
|---|---|
| suite loader | E0 schema |
| model matrix | E1 model profiles |
| preflight | E1 model/suite loader |
| dry-run | E1 matrix, E2 preflight result shape |
| live execution | E3 artifact root, process runner |
| plan scoring | E0 fixtures, E1 suite constraints |
| postcheck | E0 scenario schema |
| ultra-step-run | E6 run artifacts, E7 phase snapshot |
| report/compare | E6 summary/events, E4 score events |

## 共通ゲート

通常ゲート。

```bash
cd mvp/anvilminimal
cargo fmt --check
cargo test
python3 -m unittest discover -s tests/eval -p 'test_*.py'
```

script smoke。

```bash
python3 scripts/eval-preflight.py --suite eval/suites/mvp-smoke.yaml --model-profile speed-cloud --offline-ok
python3 scripts/eval-run.py --suite eval/suites/mvp-smoke.yaml --model-profile speed-cloud --modes minimal-loop,step-plan --runs 1 --dry-run
python3 scripts/eval-score-plan.py --plan eval/fixtures/plans/good-step-plan.yaml --scenario-id fixture-step
python3 scripts/eval-compare.py --baseline eval/fixtures/summaries/baseline.summary.eval.tsv --experiment eval/fixtures/summaries/experiment.summary.eval.tsv --out /tmp/anvilminimal-eval-compare.md
```

live smoke。

```bash
python3 scripts/eval-run.py --suite eval/suites/mvp-smoke.yaml --model-profile speed-cloud --modes step-plan --runs 1 --parallel 2
python3 scripts/eval-run.py --suite eval/suites/mvp-smoke.yaml --model-profile local-only --modes minimal-loop --runs 1 --parallel 1
```

## Phase E0: scaffold / schema / docs

目的: eval harness のファイル配置、schema、README を先に固定する。

| ID | 作業 | 作成/変更 | テスト/確認 |
|---|---|---|---|
| E0-T01 | eval ディレクトリを作る | `eval/`, `eval/suites`, `eval/fixtures` | `find eval -maxdepth 3` |
| E0-T02 | script entrypoint の空実装を置く | `scripts/eval-*.py` | `python3 scripts/eval-run.py --help` |
| E0-T03 | 共有 package を置く | `scripts/eval_lib/__init__.py` | import smoke |
| E0-T04 | README を書く | `eval/README.md` | preflight/dry-run/live/report/compare が記載済み |
| E0-T05 | summary schema を定義する | `eval/plan_score_schema.yaml` または `eval/summary_schema.yaml` | `test_summary_schema.py` |

受け入れ条件。

- `python3 scripts/eval-run.py --help` が exit 0
- `python3 scripts/eval-preflight.py --help` が exit 0
- script は `mvp/anvilminimal` を cwd として動く
- core binary は eval script を import しない

## Phase E1: model profiles / suite loader / matrix

目的: model profile と scenario suite を読み、run matrix を決定的に展開する。

| ID | 作業 | 作成/変更 | テスト |
|---|---|---|---|
| E1-T01 | model profile schema を作る | `eval/model_profiles.yaml` | `test_model_profiles.py` |
| E1-T02 | typo normalization を実装する | `eval_lib/models.py` | `gollama`, `gemini:emini-*` fixture |
| E1-T03 | provider:model を CLI args へ展開する | `eval_lib/models.py` | expected args test |
| E1-T04 | suite schema を実装する | `eval_lib/suites.py` | required field test |
| E1-T05 | `mvp-smoke.yaml` を作る | `eval/suites/mvp-smoke.yaml` | small/medium/large がある |
| E1-T06 | `mvp-balanced.yaml` を作る | `eval/suites/mvp-balanced.yaml` | 18 scenarios 以上 |
| E1-T07 | `mvp-full.yaml` を作る | `eval/suites/mvp-full.yaml` | 既存 25 regression を参照/取込 |
| E1-T08 | matrix 展開を実装する | `eval_lib/matrix.py` | mode/model/run count が一致 |

`model_profiles.yaml` 最小構造。

```yaml
profiles:
  speed-cloud:
    runs:
      - main: openai:gpt-5.4-mini
        planner: gemini:gemini-3.5-flash
      - main: gemini:gemini-3.1-flash
        planner: openai:gpt-5.4-mini
  local-only:
    serial: true
    runs:
      - main: ollama:qwen3.6:27b-coding-nvfp4
        planner: ollama:qwen3.6:27b-coding-nvfp4
```

受け入れ条件。

- unknown provider/model は invalid config として fail
- typo normalization は warning を返す
- local LLM を含む run は `serial_lane=true`
- 3011 固定 scenario は `port_mutex=3011`

## Phase E2: preflight

目的: 実行前に依存と認証情報を検査し、失敗条件を早く明示する。

| ID | 作業 | 作成/変更 | テスト |
|---|---|---|---|
| E2-T01 | dependency check を実装する | `eval-preflight.py`, `eval_lib/process.py` | missing command fixture |
| E2-T02 | env/.env key check を実装する | `eval_lib/config.py` | redacted output test |
| E2-T03 | Ollama model check を実装する | `eval_lib/models.py` | offline skip test |
| E2-T04 | port availability check を実装する | `eval_lib/postcheck.py` | occupied port fixture |
| E2-T05 | preflight output を固定する | `preflight.json`, `warnings.jsonl` | schema test |
| E2-T06 | `--offline-ok` を実装する | `eval-preflight.py` | no API key でも dry-run 可 |

exit code。

| code | 意味 |
|---:|---|
| 0 | success |
| 2 | missing local dependency |
| 3 | missing credential/model |
| 4 | port unavailable |

受け入れ条件。

- speed-cloud で `OPENAI_API_KEY` / `GEMINI_API_KEY` 不足を検出できる
- local-only で Ollama model 不足を skip/fail として記録できる
- secrets は stdout/stderr/preflight に出ない

## Phase E3: dry-run / scheduler / artifact root

目的: LLM を呼ばずに matrix、command、artifact layout、parallel lane を確認できるようにする。

| ID | 作業 | 作成/変更 | テスト |
|---|---|---|---|
| E3-T01 | run root 作成を実装する | `eval_lib/artifacts.py` | path layout test |
| E3-T02 | run_id 生成を実装する | `eval_lib/artifacts.py` | unsafe char normalization |
| E3-T03 | command rendering を実装する | `eval_lib/matrix.py` | minimal/plan/ultra args test |
| E3-T04 | dry-run を実装する | `eval-run.py` | command.txt exists |
| E3-T05 | queue lane を実装する | `eval_lib/matrix.py` | local serial/cloud parallel |
| E3-T06 | provider semaphore config を実装する | `eval_lib/matrix.py` | provider limit test |

dry-run の期待出力。

```text
workspace/eval-artifacts/anvilminimal-mvp/<timestamp>/
  preflight.json
  matrix.json
  warnings.jsonl
  runs/<run_id>/command.txt
  runs/<run_id>/meta.json
```

受け入れ条件。

- `--dry-run` は LLM/API/Ollama を呼ばない
- all modes の command が生成される
- `--parallel 4` でも local LLM run は serial lane に入る
- 3011 scenario は mutex 情報を `matrix.json` に持つ

## Phase E4: plan scoring

目的: step plan / ultra plan を deterministic に採点し、score details を出す。

| ID | 作業 | 作成/変更 | テスト |
|---|---|---|---|
| E4-T01 | scoring rule を YAML 化する | `eval/scoring_rules.yaml` | weights sum test |
| E4-T02 | step plan parser adapter を実装する | `eval_lib/plan_scoring.py` | good-step-plan |
| E4-T03 | ultra plan parser adapter を実装する | `eval_lib/plan_scoring.py` | good-ultra-plan |
| E4-T04 | guardrail を実装する | `eval_lib/plan_scoring.py` | overlong/path escape fixture |
| E4-T05 | scenario constraints と照合する | `eval_lib/plan_scoring.py` | expected artifacts/verify keyword |
| E4-T06 | standalone CLI を実装する | `eval-score-plan.py` | `--plan` smoke |
| E4-T07 | run root 再採点を実装する | `eval-score-plan.py` | `--run-root` fixture |

score details の必須 field。

```json
{
  "yaml_validity": 10,
  "step_count_fit": 8,
  "atomicity": 12,
  "responsibility_boundary": 13,
  "instruction_clarity": 11,
  "expected_paths": 8,
  "verify_commands": 7,
  "dependency_order": 10,
  "repairability": 4,
  "penalties": []
}
```

受け入れ条件。

- parse failure は score 0
- `..` / absolute path / home path は plan-only でも減点
- step 数、verify 数、path 数の水増しだけでは高得点にならない
- scorer unit test は good/bad fixture を含む

## Phase E5: postcheck / dev server lifecycle

目的: expected artifacts、終了する command、long-running dev server、HTTP readiness を安全に検証する。

| ID | 作業 | 作成/変更 | テスト |
|---|---|---|---|
| E5-T01 | expected artifacts check を実装する | `eval_lib/postcheck.py` | missing artifact fixture |
| E5-T02 | command postcheck を実装する | `eval_lib/postcheck.py` | rc/elapsed capture |
| E5-T03 | dependency command 分類を実装する | `eval_lib/postcheck.py` | npm install elapsed separated |
| E5-T04 | dev server 起動を実装する | `eval_lib/postcheck.py` | fake server fixture |
| E5-T05 | HTTP readiness を実装する | `eval_lib/postcheck.py` | 200/500/timeout test |
| E5-T06 | shutdown を実装する | `eval_lib/postcheck.py` | process remains absent |
| E5-T07 | standalone CLI を実装する | `eval-postcheck.py` | scenario+workdir smoke |

dev server contract。

- `postcheck.dev_server.command` は foreground process として起動する
- readiness 成功または timeout 後に必ず signal shutdown する
- stdout/stderr は `postcheck/dev-server.*.log` へ保存する
- 3011 固定 scenario は mutex を取る

受け入れ条件。

- `npm run dev` が harness を止めない
- readiness 200 を postcheck pass として記録する
- readiness 500 は postcheck fail として記録する
- timeout 後に子プロセスが残らない

## Phase E6: live execution / metrics / summary

目的: 実際に `anvilminimal` を起動し、summary/events/run artifacts を保存する。

| ID | 作業 | 作成/変更 | テスト |
|---|---|---|---|
| E6-T01 | process runner を実装する | `eval_lib/process.py` | timeout kill test |
| E6-T02 | stdout/stderr capture を実装する | `eval_lib/process.py` | logs written |
| E6-T03 | `ANVIL_EVAL_EVENTS` env を渡す | `eval-run.py` | meta contains path |
| E6-T04 | missing metrics null policy を実装する | `eval_lib/run_summary.py` | null metric test |
| E6-T05 | files changed count を実装する | `eval_lib/run_summary.py` | fixture workdir diff |
| E6-T06 | summary row writer を実装する | `eval_lib/run_summary.py` | header exact match |
| E6-T07 | events writer を実装する | `eval_lib/run_summary.py` | JSONL valid |
| E6-T08 | required artifacts copy を実装する | `eval_lib/artifacts.py` | state/plans copied |

summary header。

```text
run_id	suite	scenario	size	category	mode	main_provider	main_model	planner_provider	planner_model	local_llm_used	rc	success	queue_wait_sec	process_elapsed_sec	exec_elapsed_sec	model_elapsed_sec	tool_elapsed_sec	postcheck_elapsed_sec	dependency_elapsed_sec	iterations	tool_calls	files_changed	plan_quality_score	ultra_phase_quality_score	execution_score	time_score	overall_score	workdir	plan_artifacts	extras_json
```

受け入れ条件。

- timeout 時に child process と dev server が残らない
- `summary.eval.tsv` は header 順序が固定
- `events.jsonl` は全行 JSON parse 可能
- `model_elapsed_sec` 等が取れない場合は空文字/`null` policy が一貫する

## Phase E7: ultra artifacts / phase snapshot / ultra-step-run

目的: ultra plan run の phase artifact を保存し、phase 開始前 snapshot から replay する。

| ID | 作業 | 作成/変更 | テスト |
|---|---|---|---|
| E7-T01 | ultra plan artifact 検出を実装する | `eval_lib/artifacts.py` | plan path fixture |
| E7-T02 | phase step plan 検出を実装する | `eval_lib/artifacts.py` | phase plan fixture |
| E7-T03 | phase before/after snapshot を実装する | `eval_lib/artifacts.py` | snapshot exists |
| E7-T04 | snapshot copy exclude を実装する | `eval_lib/artifacts.py` | excludes node_modules/target |
| E7-T05 | ultra-step matrix expansion を実装する | `eval_lib/matrix.py` | phase replay rows |
| E7-T06 | `--run-plan` replay command を生成する | `eval_lib/matrix.py` | absolute plan path |
| E7-T07 | snapshot 不在時 skipped を実装する | `eval_lib/run_summary.py` | diagnostic_skipped not success |

snapshot exclude。

```text
node_modules/
target/
.next/
.git/
```

受け入れ条件。

- ultra phase ごとに `plans/phase-<id>.step-plan.yaml` が保存される
- replay は phase 開始前 snapshot を cwd にする
- snapshot 不在は failure ではなく `diagnostic_skipped`
- `ultra-step-run` は総合 success rate と diagnostic rate を分けて report する

## Phase E8: report / compare

目的: eval 結果を人間が判断できる report と、baseline 比較にする。

| ID | 作業 | 作成/変更 | テスト |
|---|---|---|---|
| E8-T01 | summary loader を実装する | `eval_lib/run_summary.py` | fixture TSV |
| E8-T02 | aggregate を実装する | `eval_lib/report.py` | success/p50/p90 test |
| E8-T03 | score ranking を実装する | `eval_lib/report.py` | top/bottom test |
| E8-T04 | failure classification を実装する | `eval_lib/report.py` | timeout/provider/postcheck |
| E8-T05 | markdown report を生成する | `eval-report.py` | report contains sections |
| E8-T06 | baseline compare を実装する | `eval-compare.py` | fixture compare |
| E8-T07 | regression threshold を実装する | `eval-compare.py` | fail-on-regression option |

report 必須セクション。

- mode 別 success / p50 / p90
- size 別 success / p50 / p90
- model profile 別 success / elapsed
- local LLM 使用あり/なし比較
- plan_quality_score 上位/下位
- ultra_phase_quality_score 上位/下位
- speed-cloud fastest
- timeout/interrupted/provider/postcheck failure 内訳
- skipped diagnostics

受け入れ条件。

- `eval-report.py --run-root <root>` が `report.md` を生成する
- `eval-compare.py` が baseline/experiment の差分を生成する
- compare fixture で success drop / elapsed regression を検出できる

## Phase E9: acceptance / regression / documentation

目的: 実装後の受け入れ条件、実行コマンド、残リスクを固定する。

| ID | 作業 | 作成/変更 | テスト |
|---|---|---|---|
| E9-T01 | eval README を完成させる | `eval/README.md` | command list present |
| E9-T02 | Rust script contract test を追加する | `tests/eval_script_contract.rs` | suite count/header |
| E9-T03 | Python tests を CI 対象にする | README / CI plan | unittest command |
| E9-T04 | speed-cloud smoke を実行する | run artifact | summary/report exists |
| E9-T05 | local-only smoke を実行する | run artifact | serial lane confirmed |
| E9-T06 | Next.js 3011 live scenario を実行する | run artifact | build + HTTP 200 |
| E9-T07 | known limitations を記録する | `eval/README.md` | local/API/rate limit notes |

最終受け入れコマンド。

```bash
cd mvp/anvilminimal
cargo fmt --check
cargo clippy --all-targets -- -D warnings
cargo test
python3 -m unittest discover -s tests/eval -p 'test_*.py'
python3 scripts/eval-run.py --suite eval/suites/mvp-smoke.yaml --model-profile speed-cloud --modes minimal-loop,step-plan,plan-run,ultra-plan-run --runs 1 --parallel 4
python3 scripts/eval-report.py --run-root "$ANVIL_EVAL_ROOT/<timestamp>"
```

local acceptance。

```bash
python3 scripts/eval-run.py --suite eval/suites/mvp-smoke.yaml --model-profile local-only --modes minimal-loop,step-plan --runs 1 --parallel 1
```

Next.js acceptance。

```bash
python3 scripts/eval-run.py --suite eval/suites/mvp-smoke.yaml --model-profile speed-cloud --modes ultra-plan-run --scenario nextjs-space-invaders-large --runs 1 --parallel 1
```

## 実装時の注意

| 注意 | 内容 |
|---|---|
| `.env` | `OPENAI_API_KEY` / `GEMINI_API_KEY` は読み込むが、ログへ出さない |
| `NODE_ENV` | dev server postcheck は `env -u NODE_ENV` を使う |
| network | dependency install と API call は live smoke。unit test へ混ぜない |
| ports | 3011 固定 scenario は mutex 必須 |
| paths | workdir / snapshot / plan path は workspace-relative または run-root 配下に閉じる |
| process | timeout / interrupt / failure で child process を必ず cleanup |
| metrics | 推定不能な metric は推定せず null。推定する場合は `metric_source` を明示 |
| backward compatibility | 既存 `scripts/bench.sh` と `summary.tsv` は変更しない |

## Definition of Done

- eval scripts の directory structure がこの文書通りに存在する
- `--dry-run` が API/Ollama なしで matrix と command を生成できる
- `summary.eval.tsv` / `events.jsonl` / `warnings.jsonl` / `report.md` が生成される
- plan scorer が good/bad fixture を区別できる
- dev server postcheck が readiness 後に確実に shutdown する
- local LLM を含む run と cloud-only run の scheduler policy が分かれる
- speed-cloud smoke と local-only smoke の実行手順が README にある
- Next.js 3011 scenario は `npm run build` と HTTP 200 まで確認できる

## 2026-06-25 追補作業: Provider Tool Call Failure Response

| ID | 作業 | 作成/変更 | テスト |
|---|---|---|---|
| E10-T01 | 2026-06-25 minimal-loop failure snapshot を fixture 化 | `eval/fixtures/provider_failures/minimal_loop_20260625.json` | `test_failure_snapshot_classification.py` |
| E10-T02 | OpenAI Responses `function_call.arguments` の JSON string decode | `src/providers/openai.rs` | parser unit tests |
| E10-T03 | Gemini function call arguments の object/string/null 扱いを整理 | `src/providers/gemini_function_calling.rs` | parser unit tests |
| E10-T04 | recoverable tool validation feedback を minimal loop に追加 | `src/minimal_loop/loop_run.rs`, `src/tools/registry.rs` | missing arg retry / dangerous command hard error |
| E10-T05 | `ANVIL_EVAL_EVENTS` writer を runtime に追加 | `src/eval_events.rs`, providers, loop | JSONL/redaction unit tests |
| E10-T06 | eval failure classification を summary/report に反映 | `scripts/eval_lib/failure_classification.py`, `eval-run.py`, `report.py` | Python unittest |
| E10-T07 | Gemini/OpenAI live provider smoke preflight を追加 | `scripts/eval-preflight.py`, `eval_lib/models.py` | offline unit tests + manual live smoke |
| E10-T08 | provider semantic smoke suite と gate を追加 | `eval/suites/mvp-provider-smoke.yaml`, `eval/README.md` | dry-run / provider-smoke summary gate |
| E10-T09 | provider/tool-call 横展開レビューを記録 | `workspace/mvp/eval/001/provider_toolcall_cross_review.md` | review doc present |

追加 Definition of Done。

- `cargo test` と `python3 -m unittest discover -s tests/eval -p 'test_*.py'` が通る
- `eval-run.py --dry-run` が `mvp-provider-smoke` と `mvp-smoke` の両方で matrix を生成できる
- `success=false` の eval row は `extras_json.failure_kind` を持つ
- provider smoke summary が failed の場合、本体 eval は `--allow-provider-smoke-failure` なしで停止する
- live provider smoke は unit test には混ぜず、`.env` の `OPENAI_API_KEY` / `GEMINI_API_KEY` がある環境で明示実行する
