# anvilminimal MVP Eval Design

## 目的

`anvilminimal` の以下の実行経路を、同じシナリオ群・同じ検証条件・同じ結果スキーマで比較できるようにする。

- minimal loop
- step plan
- plan run
- ultra plan run
- ultra step run

評価では「実行できたか」だけでなく、計画系の分解品質、処理時間、プロバイダ組み合わせ、ローカル LLM 使用有無を分けて記録する。

## レビュー反映方針

この設計は次の失敗を避けることを優先する。

- long-running server command を通常 postcheck として実行して harness が止まる
- ultra step replay が前 phase の workspace state に暗黙依存して、評価結果を読めなくなる
- 並列 queue の待ち時間と実処理時間が混ざり、cloud-only と local LLM の速度比較が歪む
- plan scorer が step 数や verify 数の水増しで高得点になる
- provider/model の typo や未提供 model が silent に別条件へ流れる

## 前提と用語

### モデル指定

eval harness では `provider:model` 形式を受け取り、`anvilminimal` 実行時には現在の CLI に合わせて `--provider` / `--model` と `--planner-provider` / `--planner-model` へ展開する。

主実行モデル。

| eval 指定 | anvilminimal 展開 |
|---|---|
| `ollama:qwen3.6:27b-coding-nvfp4` | `--provider ollama --model qwen3.6:27b-coding-nvfp4` |
| `openai:gpt-5.4-mini` | `--provider openai --model gpt-5.4-mini` |
| `gemini:gemini-3.1-flash` | `--provider gemini --model gemini-3.1-flash` |

計画モデル。

| eval 指定 | anvilminimal 展開 |
|---|---|
| `ollama:qwen3.6:27b-coding-nvfp4` | `--planner-provider ollama --planner-model qwen3.6:27b-coding-nvfp4` |
| `openai:gpt-5.4-mini` | `--planner-provider openai --planner-model gpt-5.4-mini` |
| `gemini:gemini-3.5-flash` | `--planner-provider gemini --planner-model gemini-3.5-flash` |

入力上の typo は harness 側で明示的に正規化する。`gollama:` は `ollama:`、`gemini:emini-3.5-flash` は `gemini:gemini-3.5-flash` として扱い、`warnings.jsonl` に記録する。未知の provider/model は preflight で失敗させる。

### 評価モード

| eval mode | anvilminimal 操作 | 評価内容 |
|---|---|---|
| `minimal-loop` | `--prompt <goal>` または TUI 通常入力 | 直接実行力、成功率、処理時間、反復回数、ツール使用 |
| `step-plan` | `--plan-steps <goal>` | YAML step plan の分解品質のみ |
| `plan-run` | `--plan-run <goal>` | step plan の分解品質と実行結果 |
| `ultra-plan-run` | `--ultra-plan-run --profile <profile> <goal>` | ultra phase 分解品質、phase ごとの step plan 品質、実行結果 |
| `ultra-step-run` | ultra plan run が生成した phase step plan を phase snapshot から `--run-plan <plan.yaml>` で replay | ultra の各 phase が、前提状態を明示したうえで実行可能な粒度か |

`ultra-step-run` は現時点の CLI では独立コマンドではなく、eval harness 上の replay mode として定義する。将来 CLI に追加するなら、`--run-ultra-step <ultra-plan.yaml> --phase <id>` のように phase id を明示できる形にする。

`ultra-step-run` は「完全に空の workdir から各 phase が成功するか」を測るものではない。phase は前 phase の成果物に依存してよい。その代わり、harness は phase 開始前 snapshot と phase 終了後 snapshot を保存し、replay は phase 開始前 snapshot から行う。snapshot が無い場合は `ultra-step-run` を `diagnostic_skipped` として扱い、成功率へ混ぜない。

## 評価観点

### minimal loop

必須メトリクス。

| 指標 | 内容 |
|---|---|
| `success` | scenario の deterministic check が通ったか |
| `elapsed_sec` | scheduler enqueue から postcheck 完了までの総 wall clock time |
| `queue_wait_sec` | parallel scheduler 上で実行開始まで待った時間 |
| `process_elapsed_sec` | anvilminimal 子プロセスの wall clock time |
| `exec_elapsed_sec` | `process_elapsed_sec + postcheck_elapsed_sec - dependency_elapsed_sec`。queue wait と依存取得を除いた比較用時間 |
| `model_elapsed_sec` | LLM 呼び出し時間合計 |
| `tool_elapsed_sec` | tool 実行時間合計 |
| `postcheck_elapsed_sec` | postcheck の時間。model 実行時間とは分ける |
| `iterations` | loop iteration 数 |
| `tool_calls` | tool call 数 |
| `files_changed` | 変更ファイル数 |
| `postcheck_passed` | scenario の postcheck 成否 |
| `timeout` | timeout 到達 |

タスク粒度は `small` / `medium` / `large` を必須分類にする。

| 粒度 | 目安 | 例 |
|---|---|---|
| `small` | 1-2 ファイル、単一関数/単一設定、5 分以内 | README 修正、date helper 修正、JSON normalizer 修正 |
| `medium` | 2-6 ファイル、簡単なテスト追加、10-20 分以内 | Rust CLI、CSV utility、Next.js 3011 skeleton |
| `large` | 6 ファイル以上、複数責務、30-60 分以内 | Next.js game、multi-file Rust library、data report app |

### step plan / plan run

plan 品質は実行前に scorer で採点する。主観評価に寄せすぎないため、まず deterministic scorer を使い、必要な場合だけ judge model を別枠で使う。

`plan_quality_score` は 100 点満点。

| 観点 | 点 | 採点 |
|---|---:|---|
| YAML validity | 10 | parse 可能、必須 field がある |
| Step count fit | 10 | scenario 粒度に対して step 数が過不足ない |
| Atomicity | 15 | 1 step が 1 目的に閉じている |
| Responsibility boundary | 15 | scaffold / implementation / verification / docs など責務が混ざりすぎない |
| Instruction clarity | 15 | 各 step の instruction が成果物・制約・禁止事項を含む |
| Expected paths | 10 | 成果物 path が具体的で workspace-relative |
| Verify commands | 10 | 実行可能な deterministic verify がある |
| Dependency order | 10 | 依存順が自然で、verify が作成前に来ない |
| Repairability | 5 | 失敗時にどこを直すべきか step から追える |

`plan-run` の総合スコアは次で算出する。

```text
plan_run_score = 0.35 * plan_quality_score + 0.55 * execution_score + 0.10 * time_score
```

`execution_score` は postcheck、verify、requested artifact、path confinement 違反の有無から 100 点満点で算出する。

Scorer guardrail。

- step 数は scenario の `plan_constraints.min_steps/max_steps` から外れるほど減点し、上限超過分は加点しない
- `expected_paths` は scenario の `expected_artifacts` と照合し、存在しない曖昧 path の羅列では加点しない
- `verify` は allowlist された deterministic command、または scenario の `required_verify_keywords` に一致する command だけ加点する
- instruction clarity は「成果物」「制約」「検証」の語彙を含むかを見て上限をかける。長文だけでは加点しない
- path confinement を破る absolute path / `..` / home path は plan-only でも減点する
- scorer は各観点の内訳を `events.jsonl` の `plan_score.details` に必ず出す

### ultra plan run

ultra は phase plan と phase 内 step plan の二層で評価する。

`ultra_phase_quality_score` は 100 点満点。

| 観点 | 点 | 採点 |
|---|---:|---|
| Phase count fit | 15 | 2-8 phase に収まり、large task でも過剰分割しない |
| Phase boundary | 20 | scaffold / implement / verify / polish など責務境界が明確 |
| Contract carryover | 15 | profile、port、required artifacts、verify 条件が後続 phase に引き継がれる |
| Phase prompt clarity | 15 | phase prompt が明確で、成果物と検証が読み取れる |
| Risk isolation | 10 | 大きな不確実性が単一 phase に閉じ込められている |
| Step plan quality average | 15 | phase ごとの step plan の平均 score |
| Final verification strength | 10 | 最終 phase が deterministic verify / repair を含む |

`ultra-plan-run` の総合スコア。

```text
ultra_score = 0.30 * ultra_phase_quality_score
            + 0.25 * mean(phase_step_plan_quality_score)
            + 0.35 * execution_score
            + 0.10 * time_score
```

### ultra step run

ultra plan run が生成した各 phase step plan を単体で replay する。目的は「ultra が分けた phase が、単体でも検証可能な作業単位になっているか」を見ること。

評価手順。

1. `ultra-plan-run` を実行し、ultra plan、phase step plan、phase 開始前 snapshot、phase 終了後 snapshot を保存する。
2. 各 phase step plan を phase 開始前 snapshot から `--run-plan <plan.yaml>` で実行する。
3. phase local checks と final scenario checks を分けて記録する。

初期実装では `ultra-plan-run` の実行後 artifact を使い、次の replay を行う。

```bash
anvilminimal --cwd "<phase-start-snapshot>" --run-plan "<absolute-phase-step-plan.yaml>" --yes ...
```

phase snapshot は `eval/runs/<run_id>/snapshots/<phase-id>/before` と `after` に保存する。初期実装で snapshot 保存が未対応の場合は、`ultra-step-run` を smoke 対象から外し、E4 の受け入れ条件で必須化する。

## シナリオ設計

既存の `mvp/anvilminimal/benchmarks/minimal-loop-expanded.yaml` は 25 本の regression seed として残す。性能比較用には、次の 3 階層に分けた eval suite を追加する。

| suite | 目的 | 本数 | 実行モード |
|---|---|---:|---|
| `mvp-smoke` | PR 前の短時間確認 | 6 | all modes, cloud-only speed profile |
| `mvp-balanced` | 標準比較 | 18 | all modes, selected model matrix |
| `mvp-full` | リリース前比較 | 30+25 | all modes, full model matrix + existing 25 regression |

最低限の scenario taxonomy。

| category | small | medium | large |
|---|---|---|---|
| fix-code | JS date helper | Python markdown linter | multi-module Rust parser |
| new-code | CSV utility | Rust CLI with tests | Next.js game on 3011 |
| config/profile | README command | Next.js alias/port | full Next.js app with build/dev verify |
| data/docs | schema check | data report | raw-data-protected analytics app |
| recovery | malformed tool call | missing expected artifact | repair-exhausted reporting |

各 scenario は以下の schema を持つ。

```yaml
id: nextjs-space-invaders-large
size: large
category: new-code
profile: nextjs
prompt: >
  3011 port で起動可能な Next.js スペースインベーダーゲームを作る。
expected_artifacts:
  - package.json
  - src/app/page.tsx
  - src/app/layout.tsx
postcheck:
  commands:
    - npm install --ignore-scripts
    - npm run build
  dev_server:
    command: env -u NODE_ENV npm run dev
    port: 3011
    readiness:
      url: http://127.0.0.1:3011/
      expect_status: 200
      timeout_sec: 60
    shutdown: signal
timeouts:
  total_sec: 3600
  model_call_sec: 300
plan_constraints:
  min_steps: 3
  max_steps: 8
  required_verify_keywords:
    - npm run build
    - 3011
```

`postcheck.commands` は終了する command だけを書く。`npm run dev` のような long-running process は `postcheck.dev_server` に分離し、readiness check 成功後に harness が必ず停止する。依存取得を伴う `npm install` はネットワーク時間を含むため、`dependency_elapsed_sec` として別途記録し、speed comparison の主指標には入れない。

## モデル行列

### local LLM を使用するケース

ローカル LLM を含む実行は GPU/メモリ競合を避けるため serial 実行を原則にする。

| profile | main | planner | 並列 |
|---|---|---|---|
| `local-only` | `ollama:qwen3.6:27b-coding-nvfp4` | `ollama:qwen3.6:27b-coding-nvfp4` | no |
| `local-main-gemini-plan` | `ollama:qwen3.6:27b-coding-nvfp4` | `gemini:gemini-3.5-flash` | no |
| `local-main-openai-plan` | `ollama:qwen3.6:27b-coding-nvfp4` | `openai:gpt-5.4-mini` | no |
| `openai-main-local-plan` | `openai:gpt-5.4-mini` | `ollama:qwen3.6:27b-coding-nvfp4` | no |
| `gemini-main-local-plan` | `gemini:gemini-3.1-flash` | `ollama:qwen3.6:27b-coding-nvfp4` | no |

### local LLM を使用しないケース

ローカル LLM を使わない組み合わせは、API rate limit と workspace isolation を守ったうえで並行測定する。

| profile | main | planner | 並列 |
|---|---|---|---|
| `openai-only` | `openai:gpt-5.4-mini` | `openai:gpt-5.4-mini` | yes |
| `gemini-only` | `gemini:gemini-3.1-flash` | `gemini:gemini-3.5-flash` | yes |
| `openai-main-gemini-plan` | `openai:gpt-5.4-mini` | `gemini:gemini-3.5-flash` | yes |
| `gemini-main-openai-plan` | `gemini:gemini-3.1-flash` | `openai:gpt-5.4-mini` | yes |

### speed profile

スピード重視時は local LLM を完全に除外し、`mvp-smoke` の small/medium を中心に実行する。

```bash
anvilminimal-eval run \
  --suite mvp-smoke \
  --model-profile speed-cloud \
  --modes minimal-loop,step-plan,plan-run,ultra-plan-run \
  --parallel 4
```

`speed-cloud` の実体。

| main | planner |
|---|---|
| `openai:gpt-5.4-mini` | `gemini:gemini-3.5-flash` |
| `gemini:gemini-3.1-flash` | `openai:gpt-5.4-mini` |

## 並列実行ポリシー

| 条件 | 実行方式 |
|---|---|
| main または planner が `ollama:*` | serial。`--parallel-local` を明示した場合だけ並列可 |
| local LLM なし | `--parallel N` で並列。default は 4 |
| 同一 provider の cloud API | provider 別 semaphore。default は provider ごと 2 |
| 同一 scenario の複数 mode | workdir を必ず分離 |
| `npm run dev` のような port 使用 | scenario ごとに port offset か static port reservation を使う。3011固定 scenario は mutex を取り、並列同時実行しない |
| time score | queue wait を除いた `exec_elapsed_sec` だけで計算する |

## Harness 構成

runtime とは分離する。設定・suite・fixture は `mvp/anvilminimal/eval/`、実行 entrypoint と共通 Python code は `mvp/anvilminimal/scripts/`、script test は `mvp/anvilminimal/tests/eval/` に置く。

```text
mvp/anvilminimal/
  eval/
    README.md
    suites/
      mvp-smoke.yaml
      mvp-balanced.yaml
      mvp-full.yaml
    model_profiles.yaml
    plan_score_schema.yaml
    scoring_rules.yaml
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

責務。

| component | 責務 |
|---|---|
| `eval-run.py` | matrix 展開、workdir 作成、parallel/serial 制御、anvilminimal 起動 |
| `eval-preflight.py` | provider/model/API key/Ollama model/npm availability の事前確認 |
| `eval-postcheck.py` | expected artifact、command、dev server、HTTP readiness、shutdown |
| `eval-score-plan.py` | step plan / ultra plan artifact の deterministic scoring |
| `eval-compare.py` | baseline vs experiment の統計比較 |
| `eval-report.py` | markdown/html summary 作成 |
| `eval_lib/*` | entrypoint から共有する実装。CLI 本体には import しない |
| `model_profiles.yaml` | local/cloud/speed profile 定義 |
| suite YAML | scenario と postcheck 定義 |
| `tests/eval/*` | Python script の unit/smoke tests |

Script は Rust crate の runtime dependency にしない。`cargo test` から最低限の schema smoke を呼ぶ場合も、Python script 自体は外部 process として実行する。

## Eval Script 実行方法

### 事前準備

```bash
cd mvp/anvilminimal
cargo build --release
ln -sfn "$(pwd)/target/release/anvilminimal" "$HOME/.local/bin/anvilminimal"
```

cloud provider を使う場合。

```bash
export OPENAI_API_KEY=...
export GEMINI_API_KEY=...
```

local LLM を使う場合。

```bash
export OLLAMA_HOST="${OLLAMA_HOST:-http://localhost:11434}"
ollama list | rg 'qwen3.6:27b-coding-nvfp4'
```

出力先を明示する場合。

```bash
export ANVIL_EVAL_ROOT="$PWD/../../workspace/eval-artifacts/anvilminimal-mvp"
```

### Preflight

```bash
python3 scripts/eval-preflight.py \
  --suite eval/suites/mvp-smoke.yaml \
  --model-profile speed-cloud
```

期待動作。

- `.env` / process env の `OPENAI_API_KEY` と `GEMINI_API_KEY` を確認する
- `ollama:*` が matrix に含まれる場合だけ Ollama model を確認する
- `npm`, `node`, `curl`, `anvilminimal` の存在を確認する
- 3011 固定 scenario が並列実行されない設定か確認する
- 結果を `preflight.json` と `warnings.jsonl` に出す

### Dry Run

```bash
python3 scripts/eval-run.py \
  --suite eval/suites/mvp-smoke.yaml \
  --model-profile speed-cloud \
  --modes minimal-loop,step-plan,plan-run,ultra-plan-run \
  --runs 1 \
  --parallel 4 \
  --dry-run
```

期待動作。

- 実際の LLM call は行わない
- 展開済み matrix を `matrix.json` に保存する
- 各 run の `command.txt` を生成する
- local LLM を含む run が serial queue に入ることを確認できる

### Speed Cloud Eval

ローカル LLM を使わず、速度重視で比較する。

```bash
python3 scripts/eval-run.py \
  --suite eval/suites/mvp-smoke.yaml \
  --model-profile speed-cloud \
  --modes minimal-loop,step-plan,plan-run,ultra-plan-run \
  --runs 1 \
  --parallel 4 \
  --timeout-sec 1800
```

### Local Eval

ローカル LLM を含むため serial 実行を default にする。

```bash
python3 scripts/eval-run.py \
  --suite eval/suites/mvp-smoke.yaml \
  --model-profile local-only \
  --modes minimal-loop,step-plan,plan-run,ultra-plan-run \
  --runs 1 \
  --parallel 1 \
  --timeout-sec 3600
```

### Full Matrix Eval

リリース前だけ実行する。

```bash
python3 scripts/eval-run.py \
  --suite eval/suites/mvp-full.yaml \
  --model-profile full \
  --modes minimal-loop,step-plan,plan-run,ultra-plan-run,ultra-step-run \
  --runs 3 \
  --parallel 4 \
  --timeout-sec 3600
```

`full` に local LLM が含まれる場合、harness は local run だけ serial lane に分離する。cloud-only run は provider semaphore の範囲で並列実行する。

### Plan Scoring Only

既存 run artifact に対して再採点する。

```bash
python3 scripts/eval-score-plan.py \
  --run-root workspace/eval-artifacts/anvilminimal-mvp/<timestamp> \
  --rules eval/scoring_rules.yaml
```

### Report

```bash
python3 scripts/eval-report.py \
  --run-root workspace/eval-artifacts/anvilminimal-mvp/<timestamp>
```

### Compare

```bash
python3 scripts/eval-compare.py \
  --baseline workspace/eval-artifacts/anvilminimal-mvp/<baseline>/summary.eval.tsv \
  --experiment workspace/eval-artifacts/anvilminimal-mvp/<experiment>/summary.eval.tsv \
  --out workspace/eval-artifacts/anvilminimal-mvp/<experiment>/compare.md
```

## Script Entry Point Contract

| command | required input | primary output | exit code |
|---|---|---|---|
| `eval-preflight.py` | suite, model profile | `preflight.json`, `warnings.jsonl` | 0 success, 2 missing dependency, 3 missing credential/model |
| `eval-run.py --dry-run` | suite, model profile, modes | `matrix.json`, `runs/*/command.txt` | 0 success, 2 invalid config |
| `eval-run.py` | suite, model profile, modes | `summary.eval.tsv`, `events.jsonl`, run artifacts | 0 all required runs complete, 1 one or more runs failed, 2 invalid config |
| `eval-postcheck.py` | scenario YAML, workdir | `postcheck/events.jsonl` | 0 pass, 1 postcheck fail, 2 invalid scenario |
| `eval-score-plan.py` | run root or plan file | `plan_score` events / updated summary | 0 success, 1 parse/score failure if standalone |
| `eval-report.py` | run root | `report.md` | 0 success |
| `eval-compare.py` | baseline summary, experiment summary | `compare.md` | 0 success |

## Eval Artifact 構造

`eval-run.py` は run root を 1 つ作り、その中に全 run を格納する。

```text
workspace/eval-artifacts/anvilminimal-mvp/<timestamp>/
  preflight.json
  matrix.json
  summary.eval.tsv
  events.jsonl
  warnings.jsonl
  report.md
  compare.md
  runs/
    <run_id>/
      command.txt
      meta.json
      stdout.log
      stderr.log
      workdir/
      state/
      plans/
        ultra-plan.yaml
        step-plan.yaml
        phase-<phase-id>.step-plan.yaml
      snapshots/
        <phase-id>/
          before/
          after/
      postcheck/
        events.jsonl
        dev-server.stdout.log
        dev-server.stderr.log
```

`run_id` は次の要素で安定生成する。

```text
<suite>__<scenario>__<mode>__<main-provider>-<main-model>__<planner-provider>-<planner-model>__r<run-index>
```

ファイル名に使えない文字は `_` に正規化し、元の model 名は `meta.json` に保存する。

## 実行コマンド設計

### minimal loop

```bash
anvilminimal --yes \
  --provider <provider> --model <model> \
  --planner-provider <planner_provider> --planner-model <planner_model> \
  --context-budget 65536 \
  --prompt "<scenario prompt>"
```

### step plan

```bash
anvilminimal --yes \
  --provider <provider> --model <model> \
  --planner-provider <planner_provider> --planner-model <planner_model> \
  --context-budget 65536 \
  --plan-steps "<scenario prompt>"
```

### plan run

```bash
anvilminimal --yes \
  --provider <provider> --model <model> \
  --planner-provider <planner_provider> --planner-model <planner_model> \
  --context-budget 65536 \
  --plan-run "<scenario prompt>"
```

### ultra plan run

```bash
anvilminimal --yes \
  --provider <provider> --model <model> \
  --planner-provider <planner_provider> --planner-model <planner_model> \
  --context-budget 65536 \
  --ultra-plan-run --profile <profile> "<scenario prompt>"
```

### ultra step run

```bash
anvilminimal --yes \
  --provider <provider> --model <model> \
  --planner-provider <planner_provider> --planner-model <planner_model> \
  --context-budget 65536 \
  --run-plan "<phase-step-plan.yaml>"
```

## 結果スキーマ

既存 `summary.tsv` との互換を壊さないため、新評価は `summary.eval.tsv` と `events.jsonl` を追加する。既存 harness の `summary.tsv` は変更しない。

`summary.eval.tsv`。

```text
run_id	suite	scenario	size	category	mode	main_provider	main_model	planner_provider	planner_model	local_llm_used	rc	success	queue_wait_sec	process_elapsed_sec	exec_elapsed_sec	model_elapsed_sec	tool_elapsed_sec	postcheck_elapsed_sec	dependency_elapsed_sec	iterations	tool_calls	files_changed	plan_quality_score	ultra_phase_quality_score	execution_score	time_score	overall_score	workdir	plan_artifacts	extras_json
```

`events.jsonl` は 1 event 1 JSON。

```json
{"event":"model_call","run_id":"...","provider":"gemini","model":"gemini-3.5-flash","elapsed_sec":8.2,"prompt_tokens":1234,"completion_tokens":456}
{"event":"tool_call","run_id":"...","tool":"Write","elapsed_sec":0.02,"ok":true}
{"event":"plan_score","run_id":"...","plan":"step","score":82,"details":{"atomicity":12,"verify_commands":8}}
{"event":"postcheck","run_id":"...","command":"npm run build","rc":0,"elapsed_sec":7.1}
{"event":"dev_server","run_id":"...","command":"env -u NODE_ENV npm run dev","port":3011,"ready":true,"status":200,"elapsed_sec":1.2,"shutdown":"signal"}
```

`extras_json` には以下を入れる。

```json
{
  "bench_seed": 12345,
  "bench_seed_enabled": true,
  "timeout": false,
  "provider_rate_limit_retries": 0,
  "provider_error_kind": null,
  "normalized_model_warnings": [],
  "profile": "nextjs",
  "style": "default",
  "postcheck_network_used": true,
  "anvilminimal_version": "0.1.0",
  "git_sha": "..."
}
```

`model_elapsed_sec` / `tool_elapsed_sec` / `iterations` は、`anvilminimal` が `ANVIL_EVAL_EVENTS=<path>` に構造化 event を出す場合はそれを primary source にする。未実装の間は stdout/stderr から推定せず、`null` として扱う。推定値を入れる場合は `extras_json.metric_source = "derived"` を必ず付ける。

## スコア集計

### time score

同じ scenario/mode/local_llm_used 内で比較する。fastest を 100 点、timeout を 0 点にする。queue wait と dependency install time は除外し、`exec_elapsed_sec` を使う。`postcheck.dev_server` の readiness 時間は、生成物が実際に起動可能かを測るため `exec_elapsed_sec` に含める。

```text
time_score = clamp(100 * fastest_exec_elapsed_sec / exec_elapsed_sec, 0, 100)
```

### overall score

| mode | overall |
|---|---|
| `minimal-loop` | `0.80 * execution_score + 0.20 * time_score` |
| `step-plan` | `plan_quality_score` |
| `plan-run` | `0.35 * plan_quality_score + 0.55 * execution_score + 0.10 * time_score` |
| `ultra-plan-run` | `0.30 * ultra_phase_quality_score + 0.25 * mean_phase_step_score + 0.35 * execution_score + 0.10 * time_score` |
| `ultra-step-run` | `0.45 * plan_quality_score + 0.45 * execution_score + 0.10 * time_score` |

## レポート

出力先。

```text
workspace/eval-artifacts/anvilminimal-mvp/<timestamp>/
  summary.eval.tsv
  events.jsonl
  warnings.jsonl
  report.md
  matrix.json
  runs/<run_id>/
    command.txt
    stdout.log
    stderr.log
    workdir/
    state/
    plans/
    postcheck/
```

`report.md` に必ず載せる表。

- mode 別 success rate / p50 elapsed / p90 elapsed
- size 別 success rate / p50 elapsed
- model profile 別 success rate / cost proxy / elapsed
- plan_quality_score 上位/下位
- ultra_phase_quality_score 上位/下位
- local LLM 使用あり/なし比較
- speed-cloud の最短結果
- timeout / interrupted / provider error / postcheck failure の内訳

## 受け入れ条件

Phase E1: harness skeleton。

- `eval-run.py --dry-run` が suite/mode/model matrix を展開できる
- `provider:model` を CLI args に変換できる
- typo normalization warning が `warnings.jsonl` に出る
- local LLM を含む run は serial queue に入る
- local LLM を含まない run は `--parallel` で並列 queue に入る
- `eval-preflight.py` が API key、Ollama model、npm、port availability を確認できる

Phase E2: scenario and postcheck。

- small/medium/large を含む `mvp-smoke.yaml` を用意する
- `mvp-balanced.yaml` に 18 本以上を用意する
- 既存 25 本 benchmark を `mvp-full` に取り込める
- postcheck command / expected artifact / http status を記録できる
- 3011 固定 scenario は同時並列されない
- long-running dev server は readiness 後に停止され、harness process が残らない

Phase E3: plan scoring。

- step plan YAML を deterministic score できる
- ultra plan YAML を deterministic score できる
- score details が `events.jsonl` に残る
- plan parse failure は score 0 かつ実行 failure として集計される
- scorer の unit test が small/medium/large の fixture を持つ
- scorer guardrail により、step/verify/path の水増しでは高得点にならない fixture を持つ

Phase E4: execution metrics。

- minimal loop / step plan / plan run / ultra plan run / ultra step run を起動できる
- queue/process/exec/model/tool/postcheck/dependency elapsed、iteration、tool call、files changed を記録できる
- stdout/stderr/session/plans/workdir を run ごとに保存できる
- ultra-step-run は phase 開始前 snapshot から replay できる
- `ANVIL_EVAL_EVENTS` 未実装の場合、未取得 metric は `null` として出力される
- timeout 時に子プロセスを確実に停止できる

Phase E5: report and compare。

- `summary.eval.tsv` と `events.jsonl` が生成される
- `eval-report.py` が `report.md` を生成する
- baseline と experiment の success/elapsed/score 差分を比較できる
- speed-cloud profile のみを実行できる
- local-only profile のみを実行できる

Phase E6: live acceptance。

- `.env` の `OPENAI_API_KEY` / `GEMINI_API_KEY` を利用できる
- Ollama model が無い場合は preflight で skip できる
- `mvp-smoke` を speed-cloud で完走できる
- `mvp-smoke` を local-only で serial 完走できる
- Next.js large scenario が `npm run build` と `http://127.0.0.1:3011` の 200 応答まで確認される

## リスクと対策

| リスク | 対策 |
|---|---|
| plan scorer が実行結果と相関しない | plan score と execution score を別列で保持し、総合点でも execution を過半にする |
| local LLM 並列で結果が不安定になる | local LLM を含む profile は default serial。並列は明示 opt-in |
| cloud API の rate limit で比較不能になる | provider semaphore、retry count 記録、provider error を failure と別集計 |
| Next.js 3011 scenario が port 競合する | 3011 固定 scenario は mutex を持たせる。他 scenario は port offset |
| `npm run dev` が終わらず harness が止まる | long-running command は `dev_server` として起動し、readiness 後に signal shutdown する |
| dependency install のネットワーク時間で速度比較が歪む | `dependency_elapsed_sec` として分離し、time score からは除外する |
| parallel queue 待ち時間で cloud/local 比較が歪む | `queue_wait_sec` と `exec_elapsed_sec` を分け、time score は exec のみで計算する |
| plan-only mode が実行性能と混同される | `step-plan` は plan_quality_score のみ、実行性能は `plan-run` / `ultra-step-run` で評価 |
| model 名の typo で silent に別評価になる | normalization は warning 必須。未知 model は preflight failure |
| ultra-step-run が前 phase 依存を無視して不当に失敗する | phase 開始前 snapshot を保存し、その snapshot から replay する。snapshot 不在時は skipped |
| harness が core runtime と密結合する | scripts/eval 配下に分離し、binary 本体から import しない |
| LLM judge が自己採点になる | deterministic scorer を primary にする。judge は optional secondary に限定 |

## 初期実装順

1. `eval/model_profiles.yaml` と `eval/suites/mvp-smoke.yaml` を作る。
2. `scripts/eval-preflight.py` で provider/model/API key/Ollama/npm/port を確認する。
3. `scripts/eval-run.py --dry-run` で matrix 展開と command 生成を実装する。
4. `scripts/eval-score-plan.py` で step plan / ultra plan scorer と guardrail fixtures を実装する。
5. `scripts/eval-postcheck.py` で command / dev server / HTTP readiness / shutdown を実装する。
6. `minimal-loop` / `step-plan` / `plan-run` の live smoke を通す。
7. `ultra-plan-run` の artifact 保存、phase snapshot、scorer を接続する。
8. `ultra-step-run` replay を phase 開始前 snapshot + `--run-plan` で実装する。
9. `summary.eval.tsv` / `events.jsonl` / `report.md` を固定する。
10. speed-cloud と local-only の受け入れテストを追加する。

## 2026-06-25 追補: Provider Tool Call Failure 対策

`speed-cloud` の minimal-loop 全敗を受け、eval の完了条件に次を追加する。

- `ANVIL_EVAL_EVENTS` を runtime が読み、provider/tool validation の JSONL evidence を出力する
- `eval-run.py` は child run の `anvil-events.jsonl` を harness の `events.jsonl` に merge する
- failed row の `summary.eval.tsv.extras_json` には必ず `failure_kind` を含める
- OpenAI/Gemini の function call arguments は object と JSON encoded string の両方を parser unit test で検証する
- `mvp-provider-smoke.yaml` を speed-cloud full/smoke 前の semantic smoke として使用する
- cloud eval 前に `eval-preflight.py --live-provider-smoke all` で no-tool/tool-declaration の疎通を確認する
- provider smoke が failed の summary を渡した本体 eval は `--allow-provider-smoke-failure` なしでは実行しない

追加受け入れコマンド。

```bash
cd mvp/anvilminimal
python3 scripts/eval-preflight.py --suite eval/suites/mvp-provider-smoke.yaml --model-profile speed-cloud --live-provider-smoke all
python3 scripts/eval-run.py --suite eval/suites/mvp-provider-smoke.yaml --model-profile speed-cloud --modes minimal-loop,plan-run,ultra-plan-run --runs 1 --parallel 4
python3 scripts/eval-run.py --suite eval/suites/mvp-smoke.yaml --model-profile speed-cloud --modes minimal-loop,step-plan,plan-run,ultra-plan-run --provider-smoke-summary <provider-smoke-run>/summary.eval.tsv
```

横展開レビュー成果物。

- `workspace/mvp/eval/001/provider_toolcall_cross_review.md`
