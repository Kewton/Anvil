# Provider Tool Call Eval Failure Response Plan

対象: `mvp/anvilminimal`

関連 eval:

- run root: `workspace/eval-artifacts/anvilminimal-mvp/20260625-103022-236001`
- profile: `speed-cloud`
- modes: `minimal-loop,step-plan,plan-run,ultra-plan-run`
- local LLM: 未使用

## 目的

`speed-cloud` eval で minimal-loop が全敗した原因を修正し、同種の provider / tool-call / eval 証跡不足が再発しない状態にする。

対象障害:

1. OpenAI Responses API の `function_call.arguments` が JSON encoded string の場合に object へ decode できていない
2. Gemini Interactions API の endpoint / model 指定が現行 API と噛み合っていない
3. `ANVIL_EVAL_EVENTS` が runtime 側で未実装のため、raw provider / tool-call 証跡が残らない

追加観点:

- なぜこのレベルの障害がこの時点で初めて露出したのか
- 単体テスト、live smoke、受け入れ条件に不足がなかったか
- 類似バグが他 provider / planner / plan-run / ultra-plan-run に潜んでいないか

## レビュー結果

この計画の初版は方向性として妥当だが、実装計画としては次の点が不足していた。

| 指摘 | 問題 | 反映内容 |
|---|---|---|
| 証跡保全が最初にない | 修正後に同じ run artifact を再分析しづらい | Phase 0 を追加し、現 eval artifact から regression fixture と失敗分類 snapshot を固定する |
| Gemini model 対策が固定候補に寄りすぎ | model availability は変わるため、再び silent mismatch が起きる | model discovery / live smoke / unsupported model skip を必須化する |
| `ANVIL_EVAL_EVENTS` が Phase 3 で遅い | Gemini や tool validation の修正時も証跡不足が続く | Phase 0 で暫定 artifact analyzer、Phase 3 で runtime event、本番 eval 前の gate にする |
| recoverable tool validation の範囲が広い | path escape / dangerous command まで soft failure にすると安全性が落ちる | recoverable / hard failure の分類表を明確化する |
| live smoke の合否基準が弱い | 「実行したが全敗」でも作業完了に見える | 全敗 gate、provider unavailable gate、failure kind 必須化を追加する |
| 類似バグ探索の成果物が曖昧 | 横展開レビューが実施済みか判断しにくい | cross review 文書とチェックリストを受け入れ条件に追加する |
| planner 系との関連が薄い | plan-run / ultra-plan-run も同じ provider/tool-call 問題を踏む | minimal-loop 修正後、plan-run / ultra-plan-run の selected smoke を必須にする |

## 現象整理

minimal-loop の結果。

| main provider | 件数 | 結果 | 代表エラー |
|---|---:|---|---|
| `openai:gpt-5.4-mini` | 6 | 0 success / 6 failed | `missing string argument pattern`, `missing string argument command` |
| `gemini:gemini-3.1-flash` | 6 | 0 success / 6 failed | `Gemini interactions API failed: 404 Not Found` |

mode 別では `step-plan` のみ成功しており、実行系で失敗している。

| mode | success |
|---|---:|
| minimal-loop | 0/12 |
| step-plan | 12/12 |
| plan-run | 0/12 |
| ultra-plan-run | 0/12 |

## 一次原因

### OpenAI

現行の `parse_openai_response` は `function_call.arguments` を `serde_json::Value` として受け取り、そのまま `ToolCall.arguments` に渡している。

実 API では `arguments` が次のような JSON encoded string で返る。

```json
{
  "type": "function_call",
  "name": "Grep",
  "arguments": "{\"pattern\":\"TODO\"}"
}
```

この場合、Rust 側では `Value::String("{\"pattern\":\"TODO\"}")` のまま registry に渡る。`ToolRegistry::execute` は `arguments.get("pattern")` を期待しているため、`missing string argument pattern` で停止する。

### Gemini

`gemini-3.1-flash` を main model として Interactions API に投げているが、現行の Gemini model list / Interactions API 対応 model と一致していない可能性が高い。

`404 Not Found` は API key や auth よりも、endpoint / model availability / API family mismatch の疑いが強い。preflight は static allowlist と env 存在確認だけで、実際の model/tool-call smoke を行っていなかったため、eval 本番まで検出できなかった。

### Eval Events

eval runner は child process に `ANVIL_EVAL_EVENTS` を渡しているが、Rust runtime はこの env を読んでいない。

そのため今回の分析では以下が artifact に残っていない。

- provider request kind
- provider HTTP status
- provider error body の redacted snippet
- raw tool call name
- raw arguments type
- parsed argument keys
- registry validation failure kind

stderr だけでは原因が粗く、再現と分類に時間がかかる。

## なぜ今ここで露出したか

### 1. 既存 unit test が実 API shape を代表していなかった

OpenAI parser test は object-form の `arguments` だけを検証していた。

不足していた test:

- `arguments` が JSON string の正常系
- `arguments` が object の互換系
- `arguments` が malformed JSON string の異常系
- `arguments` が string だが object でない場合の異常系
- missing required arg を runtime feedback にできるか

### 2. Preflight が provider/model の疎通を検証していなかった

preflight は次を確認していた。

- API key がある
- binary がある
- port が空いている
- model 名が harness allowlist にある

しかし次を確認していなかった。

- Gemini model が現在の endpoint で実際に利用可能か
- tool declaration 付き request が通るか
- function call response が parser で読めるか
- no-tool planner call と tool-call execution call の両方が通るか

### 3. dry-run / unit tests が eval harness の配線検証に偏っていた

dry-run は matrix と command 生成の確認には有効だが、provider semantics は検証できない。

今回「eval harness ができた」ことと「cloud provider を使った実行が成立する」ことが分離されていなかった。

### 4. 受け入れ条件が live smoke の失敗分類まで要求していなかった

計画上は live smoke があるが、完了判定として以下が不足していた。

- minimal-loop cloud-only で少なくとも small scenario が成功すること
- provider ごとの no-tool / tool-call smoke が成功すること
- 失敗時に `provider_error_kind` / `tool_error_kind` が summary に出ること
- eval 結果が全敗した場合は完了不可とする gate

### 5. runtime が provider schema drift を吸収できていなかった

provider API は戻り値 shape が SDK / REST / model family で差分を持つ。現行実装は最小 shape だけを仮定しており、Responses API の JSON encoded arguments のような仕様差分を吸収していなかった。

## 類似バグが潜む可能性

### Provider parser 周辺

| 領域 | 潜在バグ | 確認方法 |
|---|---|---|
| OpenAI tool result input | function call output の role/type が Responses API の期待形と違う | multi-turn function-call fixture |
| OpenAI `output` variants | `message`, `reasoning`, `tool_search_call` 等を無視している | unknown output item fixture |
| Gemini response parser | `steps` / `output` / `functionCall` / SDK形式差分に弱い | REST sample fixtures |
| Gemini tool declaration | schema sanitizer が Gemini 非対応 keyword を残す | request snapshot test |
| provider errors | HTTP body を捨てて status だけにしている | redacted error event test |

### Tool registry 周辺

| 領域 | 潜在バグ | 確認方法 |
|---|---|---|
| required arg validation | 1つの missing arg で loop 全体が abort する | recoverable tool validation test |
| type coercion | number/bool/string の型ゆれを許容しない | argument coercion fixture |
| unknown tool | provider が類似名を返すと即 abort | unknown tool feedback test |
| duplicate tool calls | parallel tool calls の ordering / result attachment | multi-call test |
| Bash command | `command` が object/string encoded で来る | Bash string decode test |

### Planner / plan-run / ultra-plan-run 周辺

| 領域 | 潜在バグ | 確認方法 |
|---|---|---|
| step plan parser | LLM の markdown fenced YAML / extra prose で parse 低下 | fixture matrix |
| ultra plan parser | missing phase prompt で fallback できないケース | malformed ultra fixture |
| profile repair | final phase のみ repair 前提で中間 phase が落ちる | phase failure fixture |
| postcheck | artifact 未生成と provider failure が同じ process failure 扱い | failure classification test |

## 対応方針

優先順位は次の通り。

1. 現 eval artifact を証跡として固定し、失敗分類 fixture を作る
2. OpenAI arguments decode を修正し、minimal-loop OpenAI 側の実行を成立させる
3. `ANVIL_EVAL_EVENTS` を入れ、以降の失敗を raw evidence 付きで分類できるようにする
4. Gemini model discovery / live preflight を実装し、使えない model を eval 本番に入れない
5. provider/parser/tool registry の横展開テストを追加する
6. speed-cloud minimal-loop small smoke を再実行し、全敗状態を解消する

## Phase 0: 証跡固定と回帰 fixture 化

目的: 修正前の失敗を再現可能な fixture として固定し、以後の修正が本当に今回の障害を潰したか判断できるようにする。

### 実装

対象:

- `workspace/eval-artifacts/anvilminimal-mvp/20260625-103022-236001`
- `mvp/anvilminimal/eval/fixtures/provider_failures/`
- `mvp/anvilminimal/tests/eval/`

作業:

- minimal-loop 12件の `summary.eval.tsv`, `stderr.log`, `command.txt`, `meta.json` から failure snapshot を作る
- failure kind を手動分類した fixture を追加する
- `process_failure` に潰れていた失敗を次の分類へ分解する
  - `provider_http_status`
  - `tool_validation_error`
  - `provider_parse_or_model_error`
- OpenAI の `missing string argument pattern/command` を regression case として残す
- Gemini の `404 Not Found` を unsupported model / endpoint mismatch candidate として残す

fixture 例:

```json
{
  "run_id": "mvp-smoke__fix-js-date-helper-small__minimal-loop__openai-gpt-5.4-mini__gemini-gemini-3.5-flash__r1",
  "mode": "minimal-loop",
  "provider": "openai",
  "stderr": "error: missing string argument `pattern`",
  "expected_failure_kind": "tool_validation_error",
  "expected_root_cause": "openai_function_call_arguments_json_string_not_decoded"
}
```

### テスト

追加:

- `test_failure_snapshot_classification.py`
- fixture の全 entry が known failure kind に分類されること
- unknown stderr は `unclassified_process_failure` として fail すること

受け入れ条件:

- 今回の minimal-loop 12失敗を分類できる
- 修正後の eval report が同じ失敗を `process_failure` のみで出したら test failure
- 証跡 fixture に API key / prompt full text / file content full text を含めない

## Phase 1: OpenAI Responses arguments decode

### 実装

対象:

- `mvp/anvilminimal/src/providers/openai.rs`

作業:

- `OpenAiOutput.arguments` を `Option<Value>` のまま受ける
- `normalize_function_arguments(value: Option<Value>) -> anyhow::Result<Value>` を追加
- `Value::String(s)` の場合は `serde_json::from_str::<Value>(&s)` する
- decode 結果が object でない場合は error にする
- `Value::Object(_)` は互換としてそのまま通す
- `None` / `Null` は provider parse error とする
  - 理由: tool の required arg 不足と provider response malformed を分けるため
  - ただし Phase 2 以降、runtime feedback で recoverable に変換できる余地を残す
- decode 関数は provider 層に閉じ、`ToolRegistry` には常に object を渡す

### テスト

追加:

- `parses_function_call_arguments_json_string`
- `parses_function_call_arguments_object_for_compat`
- `rejects_malformed_function_call_arguments_string`
- `rejects_non_object_function_call_arguments`

受け入れ条件:

- OpenAI fixture で `ToolCall.arguments["pattern"]` が取得できる
- malformed JSON string は `provider_parse_error` として分類できる
- `cargo test providers::openai` が通る
- OpenAI main の `mvp-provider-smoke` / `minimal-loop` が少なくとも 1 本成功する

## Phase 2: Recoverable tool validation failure

### 実装

対象:

- `mvp/anvilminimal/src/minimal_loop/loop_run.rs`
- `mvp/anvilminimal/src/tools/registry.rs`

作業:

- `registry.execute` の missing arg / unknown tool / invalid type を即 return Err にしない選択肢を作る
- minimal-loop では tool validation error を `ConversationMessage::tool_result` または pending feedback としてモデルへ返し、次 iteration で修正 tool call を促す
- hard error と recoverable error を分ける
  - recoverable: missing arg, invalid arg type, unknown tool
  - hard: dangerous command, path escape, approval required, interrupted

hard / recoverable 分類。

| error | handling | 理由 |
|---|---|---|
| missing required arg | recoverable feedback | LLM が修正 tool call を出せる |
| invalid arg type | recoverable feedback | 型指定をやり直せる |
| unknown tool | recoverable feedback | tool 名を修正できる |
| malformed provider arguments | provider parse error, then recoverable only if safe | raw string が object 化できないためまず parser で分類 |
| path escape | hard error | workspace confinement 違反 |
| dangerous command | hard error | safety policy 違反 |
| approval required | hard error in non-interactive eval | eval は `--yes` 前提 |
| interrupted | hard error | user/system intent |

### テスト

追加:

- fake model が first reply で `Grep` `{}` を返し、feedback 後に `Write` する test
- missing `command` が loop abort ではなく feedback になる test
- dangerous command は引き続き abort する test

受け入れ条件:

- OpenAI tool argument decode 修正後も、将来の不完全 tool call で即全敗しない
- max iteration に到達した場合は失敗理由に recover attempts が残る

## Phase 3: `ANVIL_EVAL_EVENTS`

### 実装

対象:

- `mvp/anvilminimal/src/config.rs`
- `mvp/anvilminimal/src/providers/*.rs`
- `mvp/anvilminimal/src/minimal_loop/loop_run.rs`
- `mvp/anvilminimal/src/tools/registry.rs`

作業:

- `Config` に `eval_events_path: Option<PathBuf>` を追加
- `ANVIL_EVAL_EVENTS` を読む
- JSONL append helper を追加
- provider call events を出す
- tool call events を出す
- registry validation events を出す
- planner / plan-run / ultra-plan-run でも同じ event helper を使う
- event write failure は main flow を落とさず、stderr に redacted warning を出す

出力 event 例:

```json
{"event":"provider_request","provider":"openai","model":"gpt-5.4-mini","tools":6}
{"event":"provider_error","provider":"gemini","model":"gemini-3.1-flash","status":404,"error_kind":"http_status","body_snippet":"..."}
{"event":"tool_call_raw","name":"Grep","arguments_type":"string","argument_keys":[]}
{"event":"tool_call_parsed","name":"Grep","argument_keys":["pattern"]}
{"event":"tool_validation_error","name":"Grep","error_kind":"missing_arg","missing_arg":"pattern"}
```

禁止:

- API key を出さない
- prompt full text を出さない
- file content full text を出さない
- tool arguments は truncate する

### テスト

追加:

- temp file に eval events が JSONL として append される
- provider error body が redacted/truncated される
- tool call arguments shape が記録される
- secrets が出力されない
- event path が書けない場合でも main flow が継続する

受け入れ条件:

- eval artifact の `events.jsonl` で provider/tool failure が分類できる
- stderr だけに依存しない原因分析が可能になる
- `summary.eval.tsv` の `extras_json.metric_source` が `events` または `events+process` になる

## Phase 4: Gemini endpoint / model validation

### 実装

対象:

- `mvp/anvilminimal/src/providers/gemini.rs`
- `mvp/anvilminimal/src/providers/gemini_function_calling.rs`
- `mvp/anvilminimal/eval/model_profiles.yaml`
- `mvp/anvilminimal/scripts/eval-preflight.py`
- `mvp/anvilminimal/scripts/eval_lib/models.py`

作業:

- Gemini main model を static allowlist だけで決めない
  - `models` API または live no-tool smoke で実利用可能性を確認する
  - 利用不可なら eval 本体へ進まず `provider_model_unavailable` として preflight failure
  - profile の候補は `gemini-3.5-flash` と `gemini-3.1-flash-lite` を優先する
- preflight に Gemini live smoke を追加する
  - no-tool smoke
  - tool declaration smoke
  - model not found の場合は eval 本体を実行しない
- provider error body を取り込み、404 の詳細を events に残す
- Interactions API の payload を docs sample と照合する snapshot test を追加
- Interactions API が対象 model で使えない場合の fallback 方針を明記する
  - fallback 候補: generateContent API の functionDeclarations
  - fallback を入れる場合も native tool path と XML fallback path を混ぜない

### テスト

追加:

- Gemini request shape snapshot
- Gemini function_call parser fixture
- Gemini provider error body parser fixture
- `eval-preflight.py --live-provider-smoke gemini` 追加
- unavailable model fixture
- Interactions API unsupported fixture

受け入れ条件:

- Gemini model mismatch は eval 本番前に preflight failure になる
- Gemini main minimal-loop small scenario が少なくとも 1 本成功する、または provider-specific skip として分類される
- 404/400 が `process_failure` ではなく `provider_model_unavailable` / `provider_http_status` に分類される

## Phase 5: Eval harness failure classification

### 実装

対象:

- `mvp/anvilminimal/scripts/eval-run.py`
- `mvp/anvilminimal/scripts/eval_lib/run_summary.py`
- `mvp/anvilminimal/scripts/eval_lib/report.py`

作業:

- `events.jsonl` から failure kind を summary に反映する
- child run の `anvil-events.jsonl` と harness `events.jsonl` を merge する
- `extras_json.provider_error_kind`
- `extras_json.tool_error_kind`
- `extras_json.provider_http_status`
- `extras_json.diagnostic_reason`
- `extras_json.failure_kind`
- report に failure breakdown を追加する

分類:

- `provider_http_status`
- `provider_parse_error`
- `tool_argument_decode_error`
- `tool_validation_error`
- `path_confinement_error`
- `postcheck_failure`
- `timeout`
- `diagnostic_skipped`

### テスト

追加:

- fixture events から summary failure kind が出る
- report が provider/tool/postcheck failure を別表に出す
- 全敗 run は report で blocking と表示する

受け入れ条件:

- 今回のような 36 failed が `process_failure` だけに潰れない
- minimal-loop 失敗が provider 起因か tool 起因か TSV だけで判定できる
- `success=false` の row は必ず `extras_json.failure_kind` を持つ
- eval 全体が all failed の場合、`eval-run.py` は non-zero exit を返し、report に `blocking: all required runs failed` を出す

## Phase 6: Live smoke gate の再設計

### 実装

対象:

- `mvp/anvilminimal/eval/README.md`
- `workspace/mvp/anvilminimal_eval_design.md`
- `workspace/mvp/anvilminimal_eval_work_breakdown.md`

作業:

- 完了条件に provider semantic smoke を追加
- `speed-cloud` 全量前に `mvp-provider-smoke` を必須化
- smoke を次の段階に分ける
  1. parser fixture unit
  2. provider no-tool live
  3. provider tool-call live
  4. minimal-loop small live
  5. plan-run small live
  6. ultra-plan-run selected live

### 追加 suite

`mvp-provider-smoke.yaml` を追加する。

内容:

- `write-one-file-small`
- `grep-existing-file-small`
- `bash-echo-small`

postcheck:

- expected artifact check
- no network command
- no dev server

受け入れ条件:

- `speed-cloud` を full smoke する前に provider smoke が green
- provider smoke が失敗した場合は balanced/full を実行しない
- `mvp-provider-smoke` が失敗した状態で `mvp-smoke` を実行するには `--allow-provider-smoke-failure` の明示 opt-in が必要
- CI / local acceptance の通常 path では `--allow-provider-smoke-failure` を使わない

## Phase 7: 横展開レビュー

### 対象コード

- `src/providers/openai.rs`
- `src/providers/gemini.rs`
- `src/providers/gemini_function_calling.rs`
- `src/providers/ollama.rs`
- `src/providers/xml_fallback.rs`
- `src/providers/parsing.rs`
- `src/tools/registry.rs`
- `src/minimal_loop/loop_run.rs`
- `src/planner/runner.rs`

### 観点

- provider response の unknown variant を許容できるか
- function call arguments の string/object/null を全 provider で整理しているか
- tool call id が multi-turn で維持されるか
- tool result の role/type が各 provider の期待形式か
- planner no-tool call と execution tool-call の request shape が混ざっていないか
- XML fallback と native function calling の切替条件が正しいか
- missing arg が recoverable か hard failure か整理されているか

### 成果物

- `workspace/mvp/eval/001/provider_toolcall_cross_review.md`
- 指摘があれば issue / work item 化

### 受け入れ条件

- provider ごとの request / response parser matrix を表にする
- tool-call arguments の string/object/null/malformed の扱いを provider ごとに明記する
- hard / recoverable の分類が `ToolRegistry` と minimal-loop の実装に一致している
- plan-run / ultra-plan-run が minimal-loop と同じ provider event / tool validation event を出すことを確認する

## 実行順

1. Phase 0 の証跡 fixture を追加
2. OpenAI arguments decode を修正
3. OpenAI parser unit test を追加
4. minimal-loop OpenAI small scenario を再実行
5. `ANVIL_EVAL_EVENTS` を実装
6. event unit test を追加
7. failure classification を summary/report に反映
8. Gemini model discovery / profile 修正
9. Gemini preflight live smoke を追加
10. provider smoke suite を追加
11. `speed-cloud` minimal-loop のみ再実行
12. plan-run / ultra-plan-run に横展開
13. cross review 文書を作成
14. design / work breakdown の完了条件を更新

## 最終受け入れ条件

必須 unit:

```bash
cd mvp/anvilminimal
cargo fmt --check
cargo clippy --all-targets -- -D warnings
cargo test
python3 -m unittest discover -s tests/eval -p 'test_*.py'
```

provider parser:

```bash
cargo test providers::openai::tests::parses_function_call_arguments_json_string
cargo test providers::gemini_function_calling::tests
```

provider smoke:

```bash
python3 scripts/eval-preflight.py \
  --suite eval/suites/mvp-provider-smoke.yaml \
  --model-profile speed-cloud \
  --live-provider-smoke
```

OpenAI minimal-loop smoke:

```bash
python3 scripts/eval-run.py \
  --suite eval/suites/mvp-provider-smoke.yaml \
  --model-profile openai-main-gemini-plan \
  --modes minimal-loop \
  --runs 1 \
  --parallel 1 \
  --timeout-sec 600
```

Gemini availability smoke:

```bash
python3 scripts/eval-preflight.py \
  --suite eval/suites/mvp-provider-smoke.yaml \
  --model-profile gemini-main-openai-plan \
  --live-provider-smoke
```

minimal-loop smoke:

```bash
python3 scripts/eval-run.py \
  --suite eval/suites/mvp-provider-smoke.yaml \
  --model-profile speed-cloud \
  --modes minimal-loop \
  --runs 1 \
  --parallel 2 \
  --timeout-sec 600
```

判定:

- OpenAI main の minimal-loop small smoke が成功する
- Gemini main の minimal-loop small smoke が成功する、または preflight で provider/model unavailable として本体実行前に停止する
- `summary.eval.tsv` に failure kind が入る
- `events.jsonl` に raw tool-call shape と provider error kind が残る
- 全敗の eval を success として扱わない
- `provider_toolcall_cross_review.md` が作成され、類似バグ探索が完了している
- `workspace/mvp/anvilminimal_eval_design.md` と `workspace/mvp/anvilminimal_eval_work_breakdown.md` の完了条件が更新されている

## リスクと対策

| リスク | 対策 |
|---|---|
| OpenAI arguments decode 後も tool call が不完全 | recoverable tool validation feedback を入れる |
| Gemini Interactions API の仕様がさらに変わる | preflight live smoke と request snapshot で早期検知 |
| eval event に機密情報が混ざる | redaction / truncation test を必須にする |
| live smoke が API cost / rate limit に引っかかる | provider smoke は small 3本以下、parallel 2、retry count を記録 |
| model availability が日々変わる | static allowlist だけでなく live preflight を必須化 |
| report が失敗を粗く分類する | events-derived failure kind を summary に昇格する |

## 完了後の再評価

再実行対象:

1. `mvp-provider-smoke` / `minimal-loop`
2. `mvp-smoke` / `minimal-loop`
3. `mvp-smoke` / `plan-run`
4. `mvp-smoke` / `ultra-plan-run` の selected scenario

期待:

- minimal-loop が 0/12 から脱する
- OpenAI tool argument 起因の `missing string argument` が消える
- Gemini 404 が preflight で検出され、本体 eval の process failure に混ざらない
- report の failure breakdown が provider/tool/postcheck を分離する

## 実施結果: 2026-06-25

Phase0〜Phase7 を実装済み。

主な変更。

- Phase0: `mvp/anvilminimal/eval/fixtures/provider_failures/minimal_loop_20260625.json` と `test_failure_snapshot_classification.py` を追加
- Phase1: OpenAI Responses `function_call.arguments` の JSON encoded string decode を追加
- Phase2: missing arg / unknown tool を recoverable feedback にし、dangerous command/path confinement は hard error のまま維持
- Phase3: runtime `ANVIL_EVAL_EVENTS` writer と provider/tool events を追加
- Phase4: Gemini model profile を `gemini-3.1-flash-lite` へ更新し、live provider smoke と Gemini stateful `previous_interaction_id` 継続を追加
- Phase5: `eval-run.py` が child `anvil-events.jsonl` を merge し、failed row の `extras_json.failure_kind` を記録
- Phase6: `mvp-provider-smoke.yaml` と provider smoke summary gate を追加
- Phase7: `workspace/mvp/eval/001/provider_toolcall_cross_review.md` を追加

検証結果。

```text
cargo fmt --check: pass
cargo clippy --all-targets -- -D warnings: pass
cargo test: pass (84 passed, ignored live tests excluded)
python3 -m unittest discover -s tests/eval -p 'test_*.py': pass (15 tests, 1 skipped)
eval-preflight.py --live-provider-smoke all: pass
mvp-provider-smoke minimal-loop speed-cloud: 6/6 success
mvp-provider-smoke selected plan-run/ultra-plan-run speed-cloud: 4/4 success
```

live artifact。

- `/tmp/anvilminimal-provider-smoke-live-preflight`
- `/tmp/anvilminimal-provider-smoke-minimal-loop-2`
- `/tmp/anvilminimal-provider-smoke-plan-selected`

補足。

- 最初の Gemini minimal-loop smoke は初回 tool call 後の 2 回目 request で `400 invalid_request` になった。
- 原因は stateless replay に必要な Gemini `steps` の完全再送を満たしていなかったこと。
- 対策として Gemini client が interaction id を保持し、tool result 継続時に `previous_interaction_id` を使う stateful path に切り替えた。
