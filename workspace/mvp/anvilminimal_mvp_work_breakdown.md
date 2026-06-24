# anvilminimal MVP 作業具体化

入力計画: `workspace/mvp/minimal_loop_ultra_plan_migration_plan.md`

この文書は、移植計画を実装タスクへ分解した作業台帳。成果物は `mvp/anvilminimal` 配下に独立 Rust crate として作る。既存 `src` 配下は参照だけに使い、変更しない。

## 作業方針

- core MVP と verification harness を分けて進める
- Phase 1 以降はテストを同じ変更単位で追加する
- default `cargo test` は network/API key なしで通るようにする
- live provider smoke は `ANVIL_LIVE_PROVIDER_TESTS=1` gate に閉じる
- provider 固有の response は provider 層で `AssistantReply` に正規化し、loop/planner へ漏らさない
- path confinement は tool / plan / REPL goal reference の全入口で同じ実装を使う

## 成果物

| 種別 | path | 内容 |
|---|---|---|
| core crate | `mvp/anvilminimal` | copy 可能な Rust CLI/TUI |
| tests | `mvp/anvilminimal/tests` | CLI/integration/script smoke |
| fixtures | `mvp/anvilminimal/tests/fixtures` | provider/YAML/profile/path fixture |
| benchmark | `mvp/anvilminimal/benchmarks` | `minimal-loop-expanded.yaml` 25 scenarios |
| scripts | `mvp/anvilminimal/scripts` | `bench.sh`, `compare.py`, script smoke |
| docs | `mvp/anvilminimal/README.md`, `mvp/anvilminimal/docs/mechanism-ledger.md` | build/run/symlink/copy/live smoke/mechanism tracking |

## 全体の実行順

1. Phase 0: crate scaffold と help 起動
2. Phase 1: CLI/config/state/mode
3. Phase 2: provider 正規化と fixture
4. Phase 3: tools と workspace confinement
5. Phase 4: minimal loop
6. Phase 5: step plan / plan run / deterministic verify
7. Phase 6: ultra plan run / profile verifier
8. Phase 7: REPL/TUI / slash command
9. Phase 8: E2E / copy validation / benchmark harness

依存関係。

| task | depends on |
|---|---|
| provider 実装 | Phase 1 の `Config`, provider settings, key resolution |
| tools 実装 | Phase 1 の `Mode`, workspace root resolution |
| minimal loop | Phase 2 provider trait, Phase 3 tool registry |
| plan run | Phase 4 minimal loop, Phase 3 path guard |
| ultra plan run | Phase 5 plan run |
| REPL/TUI | Phase 1 CLI/session, Phase 5/6 command handlers |
| benchmark harness | binary CLI shape, summary schema, fake/smoke fixtures |

## 共通ゲート

各 Phase の最後に `mvp/anvilminimal` で実行する。

```bash
cargo fmt --check
cargo test
```

Phase 2 以降で network/API を使う検証は通常テストへ混ぜない。

```bash
ANVIL_LIVE_PROVIDER_TESTS=1 cargo test live_ -- --ignored
```

Phase 8 では追加で実行する。

```bash
cargo clippy --all-targets -- -D warnings
cargo run -- --help
```

live/local smoke の前提。

- Ollama smoke は Ollama server が起動済みで、指定 model が pull 済みの場合だけ実施する
- OpenAI/Gemini smoke は `.env` または process env に key がある場合だけ実施する
- model ID の availability failure は unit test failure ではなく live smoke failure として記録する
- `ANVIL_OPENAI_SMOKE_MODEL` / `ANVIL_GEMINI_SMOKE_MODEL` が設定されている場合は smoke model を上書きする

## Phase 0: scaffold

目的: copy 可能な crate の空枠を作り、`cargo run -- --help` まで通す。

| ID | 作業 | 作成/変更 | 完了条件 |
|---|---|---|---|
| P0-T01 | crate を作成する | `mvp/anvilminimal/Cargo.toml`, `src/main.rs`, `src/lib.rs` | `cargo check` が通る |
| P0-T02 | 最小 CLI help を出す | `src/cli.rs` | `cargo run -- --help` が終了コード 0 |
| P0-T03 | README 初版を書く | `README.md` | build/run/symlink/copy の最小手順がある |

最初に置く依存候補。

- `clap`
- `serde`
- `serde_json`
- `serde_yaml`
- `thiserror` または `anyhow`
- `uuid`
- `chrono`
- HTTP client は Phase 2 で確定する

Phase 0 の targeted test。

```bash
cargo check
cargo run -- --help
```

## Phase 1: data model / config / state

目的: CLI 引数、`.env` / process env、session 保存、mode を MVP 用に固定する。

| ID | 作業 | 作成/変更 | テスト |
|---|---|---|---|
| P1-T01 | CLI option を定義する | `src/cli.rs` | `tests/cli_parse.rs` |
| P1-T02 | `--engine` を持たないことを固定する | `src/cli.rs` | `help_does_not_include_engine` |
| P1-T03 | `--sidecar-model` 非対応を固定する | `src/cli.rs` | `sidecar_model_is_rejected_by_default` |
| P1-T04 | config precedence を実装する | `src/config.rs` | `process_env_overrides_dotenv` |
| P1-T05 | key resolution と redaction helper を実装する | `src/config.rs` | `api_key_values_are_not_displayed` |
| P1-T06 | minimal session schema を定義する | `src/state.rs` | `state_root_is_anvilminimal` |
| P1-T07 | `--resume` confinement を実装する | `src/state.rs` | `resume_id_cannot_escape_state_root` |
| P1-T08 | Plan/Act mode を実装する | `src/mode.rs` | `mode_default_and_transitions` |
| P1-T09 | action flag と trailing goal の grammar を固定する | `src/cli.rs` | `ultra_plan_run_allows_profile_before_goal` |

CLI に出す flag。

- `--yes`
- `--context-budget <N>`
- `--model <MODEL>`
- `--provider <ollama|openai|gemini>`
- `--planner-model <MODEL>`
- `--planner-provider <ollama|openai|gemini>`
- `--prompt <TEXT>`
- `--plan-steps`
- `--plan-run`
- `--run-plan <PATH>`
- `--ultra-plan`
- `--ultra-plan-run`
- `--run-ultra-plan <PATH>`
- `--profile <NAME>`
- `--style <NAME>`
- `--resume <ID>`
- `--offline`

CLI action grammar。

- `--prompt <TEXT>` は値付き flag として扱う
- `--plan-steps`, `--plan-run`, `--ultra-plan`, `--ultra-plan-run` は action selector として扱い、goal は trailing positional から読む
- `--profile` / `--style` などの option は action selector と trailing goal の間に置ける
- `--run-plan <PATH>` / `--run-ultra-plan <PATH>` は path value を持つ
- action selector は同時に 1 つだけ許可する
- action selector があり goal/path が不足する場合は deterministic error を返す

必ず通す CLI parse 例。

```bash
anvilminimal --provider ollama --model qwen3.6:27b-coding-nvfp4 --planner-provider gemini --planner-model gemini-3.5-flash --ultra-plan-run --profile nextjs "3011 port app"
anvilminimal --plan-run "小さい Rust CLI を作る"
anvilminimal --run-plan .anvil/plans/plan-001.yaml
```

Phase 1 の targeted test。

```bash
cargo test cli_parse config state mode
```

## Phase 2: provider

目的: Ollama/OpenAI/Gemini を `ChatClient` / `PlannerLlm` 境界に閉じ、loop から provider 差分を消す。

| ID | 作業 | 作成/変更 | テスト |
|---|---|---|---|
| P2-T01 | provider 共通型を定義する | `src/providers/mod.rs` | `assistant_reply_normalization` |
| P2-T02 | Ollama client を移植/縮約する | `src/providers/ollama.rs` | Ollama request fixture |
| P2-T03 | OpenAI tool call client を実装する | `src/providers/openai.rs` | OpenAI request/response fixture |
| P2-T04 | Gemini native function calling client を実装する | `src/providers/gemini.rs`, `gemini_function_calling.rs` | Gemini round trip fixture |
| P2-T05 | XML fallback を Ollama/OpenAI fallback 限定で残す | `src/providers/xml_fallback.rs` | Gemini では fallback 無効 test |
| P2-T06 | schema sanitizer を作る | `src/providers/parsing.rs` | unsupported keyword drop snapshot |
| P2-T07 | `llm-io.jsonl` redaction を実装する | provider logging helper | auth/key redaction fixture |
| P2-T08 | live smoke を gated test にする | `tests/live_provider.rs` | ignored/gated test |
| P2-T09 | planner/execution provider の model default を固定する | `src/config.rs`, `src/providers/mod.rs` | cross-provider planner model error |

Gemini REST/function-calling contract。

公式参照: <https://ai.google.dev/gemini-api/docs/function-calling?hl=ja#rest>

- `POST https://generativelanguage.googleapis.com/v1beta/interactions` を使う
- header は `x-goog-api-key: $GEMINI_API_KEY` と `Content-Type: application/json`
- planner no-tool request は `model: "gemini-..."`, `input: "<text>"` の simple input 形式にする。`models/` prefix、`tools`、`store`、`generation_config`、array input は送らない
- execution tool-calling request は MVP の local session store 側で履歴を持つため `store: false` にする
- `ToolSpec` は Gemini function declaration へ変換する
- `input` は user input、model `thought` / `function_call` steps、local `function_result` steps を順序通りに再送する
- `function_call.id` と `function_result.call_id` を保持し、provider round trip に使う
- final text は `output_text` または text content step から抽出する
- Gemini execution provider では XML fallback を default 無効にする

Gemini fixture の必須ケース。

- normal `function_call` -> local tool result -> `function_result` -> final text
- missing `thought`
- multiple `function_call`
- unknown tool
- schema mismatch arguments
- missing/mismatched `call_id`
- provider error
- timeout

model ID fixture。

- OpenAI request に `gpt-5.4-mini` が保持される
- Gemini request に `gemini-3.5-flash` が保持される
- Gemini request に `gemini-3.1-flash-lite` が保持される

live smoke の必須範囲。

- `ANVIL_LIVE_PROVIDER_TESTS=1 cargo test live_ -- --ignored` は request shape だけで完了扱いにしない
- Gemini planner no-tool Interactions API を実HTTPで叩き、`400 Bad Request` を検出できる ignored test を含める

planner model default rule。

- `--planner-provider` が未指定の場合は `--provider` と同じ provider を使う
- `--planner-model` が未指定かつ planner/execution provider が同じ場合は `--model` を使う
- `--planner-model` が未指定かつ planner/execution provider が異なる場合は起動時に明示エラーにする

Phase 2 の targeted test。

```bash
cargo test providers
cargo test provider_fixtures
ANVIL_LIVE_PROVIDER_TESTS=1 cargo test live_ -- --ignored
```

## Phase 3: tools / workspace policy

目的: 6 tools だけの registry と、全入口で共通の path confinement を作る。

| ID | 作業 | 作成/変更 | テスト |
|---|---|---|---|
| P3-T01 | workspace root resolver を作る | `src/tools/workspace_policy.rs` | `workspace_root_is_canonical` |
| P3-T02 | path guard を作る | `src/tools/path_guard.rs` | escape rejection matrix |
| P3-T03 | tool registry を作る | `src/tools/registry.rs` | Plan/Act allowlist |
| P3-T04 | Read/Write/Edit を実装する | `read.rs`, `write.rs`, `edit.rs` | read/write/edit fixtures |
| P3-T05 | Glob/Grep を実装する | `glob.rs`, `grep.rs` | confinement + hidden dirs policy |
| P3-T06 | Bash policy を実装する | `bash.rs` | dangerous/offline/local script |
| P3-T07 | tool catalog wording を実装する | `registry.rs` | Write does not require mkdir wording |
| P3-T08 | plan/REPL から使う path resolver API を公開する | `path_guard.rs` | expected_paths and goal reference use same guard |

path guard rejection matrix。

| case | expected |
|---|---|
| `/etc/passwd` | reject |
| `../secret.txt` | reject |
| `a/../../secret.txt` | reject |
| path containing NUL | reject |
| `C:\Users\x` | reject |
| `\\server\share` | reject |
| symlink inside workspace to outside | reject |
| new file whose existing parent escapes workspace | reject |
| valid missing child under existing workspace parent | allow for Write |

Bash policy matrix。

| command class | default | offline |
|---|---|---|
| `cargo check/test/build` in workspace | allow | allow |
| `npm/pnpm/yarn run build/test` in workspace | allow | allow if dependencies already exist |
| local script under workspace | allow after path guard | allow if no network/setup command |
| package install / network fetch | approval or block by policy | reject |
| credential dump / absolute destructive command / `rm -rf` risky target | reject | reject |

Phase 3 の targeted test。

```bash
cargo test tools
cargo test path_guard
cargo test bash_policy
```

## Phase 4: minimal loop

目的: fake chat client で deterministic に通る最小 tool loop を作る。

| ID | 作業 | 作成/変更 | テスト |
|---|---|---|---|
| P4-T01 | loop state と iteration limit を実装する | `src/minimal_loop/loop_run.rs` | max iteration test |
| P4-T02 | prompt builder を移植/縮約する | `prompt.rs` | prompt snapshot |
| P4-T03 | feedback classifier を実装する | `feedback.rs` | no-tool/missing artifact fixtures |
| P4-T04 | compaction hook を最小化する | `compact.rs` | context budget behavior |
| P4-T05 | tool result -> next turn を実装する | `loop_run.rs` | fake Write -> final |
| P4-T06 | Gemini malformed call handling を接続する | provider error handling | no XML fallback test |
| P4-T07 | session/log persistence を接続する | `state.rs`, `loop_run.rs` | messages/tool results are saved with redaction |

loop が Done として受け入れてはいけないもの。

- requested artifact が未作成
- required evidence が未実行
- no-tool progress statement だけ
- provider malformed function call の final prose

Phase 4 の targeted test。

```bash
cargo test minimal_loop
cargo test fake_chat_client_loop
```

## Phase 5: planner / step plan / plan run

目的: YAML step plan の生成、validate/lint、deterministic verify、repair loop を実装する。

| ID | 作業 | 作成/変更 | テスト |
|---|---|---|---|
| P5-T01 | `StepPlan` schema を実装する | `src/planner/step_plan.rs` | YAML round trip |
| P5-T02 | plan lint を実装する | `lint.rs` | invalid id/path/verify cases |
| P5-T03 | verifier result 型を実装する | `verify.rs` | result classification |
| P5-T04 | expected_paths hard gate を実装する | `verify.rs` | missing path before command |
| P5-T05 | quality gates を実装する | `verify.rs` | Node 0 tests/Rust/docs/data |
| P5-T06 | repair prompt/report を実装する | `repair.rs` | repair report fixture |
| P5-T07 | plan runner を実装する | `runner.rs` | fake planner/client plan-run |
| P5-T08 | CLI command を接続する | `main.rs` / command dispatcher | CLI integration |
| P5-T09 | intent detection を移植/縮約する | `intent.rs` | create/fix/research intent fixture |
| P5-T10 | plan artifact store を実装する | `runner.rs` | `.anvil/plans/plan-*.yaml` save |
| P5-T11 | exhausted repair artifact を保存する | `repair.rs` | `.anvil/repairs/repair-*.md` save |
| P5-T12 | plan file path confinement を実装する | `runner.rs` | workspace 外 absolute/`..` reject |
| P5-T13 | strict verify command 実行を実装する | `verify.rs`, `tools/bash.rs` | nonzero exit -> `CommandFailed` |

quality gate の最小 fixture。

- Node: test script が存在しても 0 tests を success にしない
- Shell: verify command の exit status 非0を success にしない
- Rust: `cargo test --manifest-path` が対象 `Cargo.toml` に束縛される
- Rust: parent workspace の test success を子 crate success にしない
- docs: required heading / command / code block を検査する
- data: schema key / row count / raw data modification を検査する

Phase 5 の targeted test。

```bash
cargo test planner
cargo test verify
cargo test plan_run
cargo test verify_command_nonzero_fails
cargo test run_plan_path_confinement
```

## Phase 6: ultra plan run / profiles

目的: ultra phase を step plan の連鎖として実行し、Next.js/data profile contract を verifier に持たせる。

| ID | 作業 | 作成/変更 | テスト |
|---|---|---|---|
| P6-T01 | `UltraPlan` schema を実装する | `src/planner/ultra_plan.rs` | YAML round trip |
| P6-T02 | ultra lint を実装する | `ultra_plan.rs` | 2-8 phase/focused goal |
| P6-T03 | profile trait を実装する | `profile.rs` | profile dispatch |
| P6-T04 | Next.js verifier を実装する | `profiles/nextjs.rs` | package/build/alias/tailwind/port |
| P6-T05 | data verifier を実装する | `profiles/data.rs` | raw file protection |
| P6-T06 | ultra runner を実装する | `runner.rs` | fake ultra-plan-run |
| P6-T07 | CLI command を接続する | command dispatcher | CLI integration |
| P6-T08 | phase ごとの plan artifact 保存を固定する | `runner.rs` | ultra phase creates step plan YAML |
| P6-T09 | phase failure report を構造化する | `runner.rs` | phase/step/invariant reported |
| P6-T10 | final profile contract failure を失敗扱いにする | `runner.rs` | fake ultra-plan-run final profile failure |

Next.js verifier fixture。

- `package.json` に `next`, `react`, `react-dom`
- `scripts.build == next build`
- 3011 要求時に `dev` script が `next dev -p 3011` 相当
- final phase 後に 3011/build/dependency/alias/tailwind 契約が未達なら `ultra-plan-run complete` を返さない
- Tailwind 使用時に dependency/config が揃う
- `@/*` 使用時に `tsconfig.json` paths が揃う
- `rootDir: ./src` が `app/` を除外しない
- dependency 未 install は `dependency_missing`

Phase 6 の targeted test。

```bash
cargo test ultra_plan
cargo test nextjs_profile
cargo test data_profile
```

## Phase 7: REPL/TUI

目的: engine 指定なしで TUI/REPL を起動し、slash command から plan/ultra run を使えるようにする。

| ID | 作業 | 作成/変更 | テスト |
|---|---|---|---|
| P7-T01 | REPL loop を実装する | `src/repl.rs` | non-tty behavior |
| P7-T02 | slash command parser を実装する | `repl.rs` | quoted prompt/profile/style/path |
| P7-T03 | `/plan-*` を接続する | `repl.rs` | fake command integration |
| P7-T04 | `/ultra-*` を接続する | `repl.rs` | fake command integration |
| P7-T05 | spinner/env disable を実装する | `repl.rs` or `ui.rs` | `ANVIL_NO_SPINNER` |
| P7-T06 | goal reference expansion を実装する | `repl.rs` + `path_guard.rs` | `$(cat relative/path)` confinement |
| P7-T07 | approval prompt と raw mode の干渉を避ける | `repl.rs` | approval prompt fixture |
| P7-T08 | CLI/REPL action parity を固定する | `cli.rs`, `repl.rs` | profile/style/path interpretation parity |

slash command fixture。

- `/plan-steps "goal with spaces"`
- `/plan-run --style default "goal"`
- `/run-plan .anvil/plans/plan-001.yaml`
- `/ultra-plan --profile nextjs --style default "goal"`
- `/ultra-plan-run --profile nextjs "3011 port app"`
- `/run-ultra-plan .anvil/plans/ultra-plan-001.yaml`
- CLI の `--profile` / `--style` と REPL slash command の `--profile` / `--style` が同じ `Config` へ反映される

Phase 7 の targeted test。

```bash
cargo test repl
cargo test slash_command
```

## Phase 8: E2E / copy validation / harness

目的: 実行可能性、copy 可能性、benchmark/recheck/compare 互換を確認する。

| ID | 作業 | 作成/変更 | テスト |
|---|---|---|---|
| P8-T01 | copy validation script を README に記載する | `README.md` | manual copy smoke |
| P8-T02 | symlink 手順を README に記載する | `README.md` | `anvilminimal --help` |
| P8-T03 | 25 scenario YAML を配置する | `benchmarks/minimal-loop-expanded.yaml` | scenario count test |
| P8-T04 | `bench.sh` を anvilminimal 用に縮約する | `scripts/bench.sh` | script smoke |
| P8-T05 | `summary.tsv` schema を固定する | `scripts/bench.sh` | header test |
| P8-T06 | recheck mode を実装する | `scripts/bench.sh` | original summary unchanged |
| P8-T07 | seed metadata を実装する | bench script/session/meta | seed smoke |
| P8-T08 | `compare.py` を移植する | `scripts/compare.py` | compare output fixture |
| P8-T09 | mechanism-ledger template を置く | `docs/mechanism-ledger.md` | required fields check |
| P8-T10 | final E2E commands を README に記録する | `README.md` | command list present |
| P8-T11 | harness を core runtime から分離する | scripts/docs only, optional `src/eval` | binary does not depend on harness modules |
| P8-T12 | clean copy validation を固定する | README / manual smoke | `target/` excluded copy -> cargo test/help |

`summary.tsv` header。

```text
run	model	case	pam_variant	rc	elapsed_sec	workdir	session_copied	extras_json
```

`summary.recheck.tsv` header。

```text
run	model	case	pam_variant	rc	elapsed_sec	workdir	session_copied	extras_json	recheck_success_check_success	recheck_success_check_reason
```

Phase 8 の targeted test。

```bash
cargo fmt --check
cargo clippy --all-targets -- -D warnings
cargo test
cargo run -- --help
bash scripts/bench.sh minimal-loop-expanded --model qwen3.6:27b-coding-nvfp4 --runs 1 --max-iterations 12 --bench-no-debug
bash scripts/bench.sh minimal-loop-expanded --recheck-root <BENCH_ROOT>
python3 scripts/compare.py <baseline-summary.tsv> <experiment-summary.tsv>
```

copy validation。

```bash
tmp=$(mktemp -d)
rsync -a --exclude target mvp/anvilminimal/ "$tmp/anvilminimal/"
cd "$tmp/anvilminimal"
cargo test
cargo run -- --help
```

mechanism-ledger required fields。

- mechanism id
- default state / off flag
- target scenarios
- admission benchmark
- before/after result
- final audit date
- rollback condition

## 実装時の source 参照順

既存実装を読む順序。

1. `src/agent/minimal_loop/*`
2. `src/agent/minimal_step_runner.rs`
3. `src/agent/minimal_step_runner/{verify,repair,plan_lint,profile,profiles}*`
4. `src/agent/minimal_repl.rs`
5. `src/agent/minimal_llm.rs`
6. `src/agent/planner_llm.rs`
7. `src/tools/{registry,bash,read,write,edit,glob,grep}.rs`
8. `src/safety/path_guard.rs`
9. `src/util/workspace_paths.rs`
10. `src/{openai,gemini,api_keys}.rs`
11. `src/ollama/*`
12. `scripts/bench.sh`
13. `scripts/compare.py`
14. `tests/scripts/test_bench_smoke.sh`

## file mapping

| 現行 | MVP |
|---|---|
| `src/agent/minimal_loop/*` | `mvp/anvilminimal/src/minimal_loop/*` |
| `src/agent/minimal_repl.rs` | `mvp/anvilminimal/src/repl.rs` |
| `src/agent/minimal_step_runner.rs` | `mvp/anvilminimal/src/planner/{runner,step_plan,ultra_plan}.rs` |
| `src/agent/minimal_step_runner/verify.rs` | `mvp/anvilminimal/src/planner/verify.rs` |
| `src/agent/minimal_step_runner/repair.rs` | `mvp/anvilminimal/src/planner/repair.rs` |
| `src/agent/minimal_step_runner/plan_lint.rs` | `mvp/anvilminimal/src/planner/lint.rs` |
| `src/agent/minimal_step_runner/intent.rs` | `mvp/anvilminimal/src/planner/intent.rs` |
| `src/agent/minimal_step_runner/profile.rs` | `mvp/anvilminimal/src/planner/profile.rs` |
| `src/agent/minimal_step_runner/profiles/*` | `mvp/anvilminimal/src/planner/profiles/*` |
| `src/agent/minimal_llm.rs` | `mvp/anvilminimal/src/providers/mod.rs` |
| `src/agent/planner_llm.rs` | `mvp/anvilminimal/src/providers/mod.rs` |
| `src/ollama/*` | `mvp/anvilminimal/src/providers/ollama.rs` |
| `src/openai.rs` | `mvp/anvilminimal/src/providers/openai.rs` |
| `src/gemini.rs` | `mvp/anvilminimal/src/providers/gemini.rs` |
| `src/api_keys.rs` | `mvp/anvilminimal/src/config.rs` |
| `src/tools/*` | `mvp/anvilminimal/src/tools/*` |
| `src/session/store.rs` | `mvp/anvilminimal/src/state.rs` |
| `src/modes/plan_act.rs` | `mvp/anvilminimal/src/mode.rs` |
| `src/safety/path_guard.rs` | `mvp/anvilminimal/src/tools/path_guard.rs` |
| `src/util/workspace_paths.rs` | `mvp/anvilminimal/src/tools/workspace_policy.rs` |

## 実装しないもの

- `Engine::Minimal`
- `--engine`
- sidecar execution
- photon
- PAM
- legacy PlanStage scope
- tester/tmp-tests 専用 policy
- old Anvil session migration
- Gemini XML fallback default
- benchmark harness as core runtime dependency

## milestone checklist

| milestone | 完了判定 |
|---|---|
| M0 | `cargo run -- --help` が通る |
| M1 | CLI/config/state/mode の unit test が通る |
| M2 | provider fixture と redaction test が通る |
| M3 | tools/path_guard/bash policy test が通る |
| M4 | fake chat client で minimal loop が通る |
| M5 | fake planner/client で plan-run が通る |
| M6 | fake planner/client で ultra-plan-run nextjs が通る |
| M7 | REPL slash command parser/integration が通る |
| M8 | copy validation と harness smoke が通る |

## レビュー反映メモ

- CLI action flag は値付き flag ではなく selector + trailing goal として固定した
- `intent.rs`、plan artifact store、repair artifact store、ultra phase artifact save を明示タスクに追加した
- Gemini REST/function-calling contract を Phase 2 に移し、fixture の判定対象を明確にした
- Bash policy と live smoke 前提を分け、default test と外部依存 smoke の境界を明確にした
- mechanism-ledger と harness/core 分離の完了条件を追加した

## final acceptance command set

`mvp/anvilminimal` で実行する。

```bash
cargo fmt --check
cargo clippy --all-targets -- -D warnings
cargo test
cargo run -- --help
cargo run -- --yes --context-budget 65536 --model qwen3.6:27b-coding-nvfp4 --planner-model gemini-3.5-flash --planner-provider gemini --provider ollama
```

`.env` に key があり、live smoke を実施する場合。

```bash
ANVIL_LIVE_PROVIDER_TESTS=1 cargo test live_ -- --ignored
cargo run -- --yes --provider openai --model gpt-5.4-mini --prompt "notes/openai-smoke.md を作成してください"
cargo run -- --yes --provider gemini --model gemini-3.1-flash-lite --prompt "notes/gemini-smoke.md を作成してください"
```

`ANVIL_LIVE_PROVIDER_TESTS=1 cargo test live_ -- --ignored` は OpenAI/Gemini/Ollama の実HTTP smoke を含む。provider request payload の shape 検査だけで成功扱いにしない。

REPL UAT。

```text
anvilminimal --yes --context-budget 65536 --model qwen3.6:27b-coding-nvfp4 --planner-model gemini-3.5-flash --planner-provider gemini --provider ollama
/ultra-plan-run --profile nextjs あなたが考える最高に面白くかっこいいスペースインベーダーゲームを3011ポートで起動可能なnext.jsアプリとして開発してください。
```
