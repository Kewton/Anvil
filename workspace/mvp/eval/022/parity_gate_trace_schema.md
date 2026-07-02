# Parity Gate Trace Schema

作成日: 2026-06-29

## 1. 目的

runtime semantics parity gate では、コード読解だけで移植完了を判断しない。source `anvildev` と MVP `anvilminimal` の実行 trace を同じ schema で保存し、lifecycle stage の意味論差分を比較できる状態を完了条件にする。

## 2. Trace Manifest 必須項目

各 trace entry は以下を必須にする。

| Field | 必須 | 内容 |
| --- | --- | --- |
| `trace_id` | yes | stable id。例: `mvp-0219-smoke` |
| `subject` | yes | `source-anvildev` / `mvp-anvilminimal` |
| `commit_sha` | yes | 実行対象 commit |
| `binary_path` | yes | 実行 binary または command |
| `binary_kind` | yes | `anvildev` / `anvilminimal` |
| `command` | yes | 再実行可能な command。secret は含めない |
| `cwd` | yes | 実行 workspace |
| `run_root` | yes | eval / run の保存先 |
| `events_path` | conditional | `events.jsonl` または `ANVIL_EVAL_EVENTS` 出力 |
| `summary_path` | conditional | eval summary |
| `suite` | conditional | eval suite 名 |
| `modes` | conditional | minimal-loop / step-plan / plan-run / ultra-plan-run |
| `provider_model_pairs` | yes | provider / model / planner provider / planner model |
| `env_redaction_status` | yes | `redacted` / `no-secrets` / `unknown` |
| `artifact_paths` | no | plan yaml、repair prompt、acceptance report など |
| `normalized_event_sequence_path` | conditional | lifecycle stage 比較用の正規化 sequence |
| `known_gaps` | yes | trace 不足、diagnostic 不足、browser evidence 不足 |

## 3. Normalized Event Sequence

source と MVP の event 名は完全一致しないため、比較用に以下の normalized stage へ写像する。

| Normalized Stage | 代表 event / evidence |
| --- | --- |
| `request_understood` | intent/profile/required artifact/capability extraction |
| `contract_loaded` | TaskContract / CompletionContract / TaskContract-lite |
| `plan_generated` | StepPlan / UltraPlan generated and parsed |
| `plan_linted` | schema / verify command / ownership lint |
| `phase_started` | ultra phase start |
| `phase_context_attached` | prior phase summary / failure / repair target |
| `step_prompt_built` | overall goal / expected paths / verify / expected result |
| `tool_requested` | raw tool name / args shape |
| `tool_executed` | Write/Edit/Bash/Read/Glob result |
| `dependency_boundary_checked` | manifest/probe/setup authority |
| `dependency_setup_attempted` | install/setup command with bounded authority |
| `verify_started` | deterministic verify command or profile verifier |
| `verify_failed` | concrete failure kind |
| `repair_target_classified` | missing entrypoint, capability missing, dependency missing, etc. |
| `repair_attempted` | bounded repair turn |
| `repair_exhausted` | max repair reached |
| `recovery_handoff_saved` | `.anvil/repairs/repair-*.md` and suggested command |
| `acceptance_started` | final acceptance / postcheck |
| `acceptance_failed` | artifact/capability/browser/postcheck failure |
| `acceptance_passed` | accepted artifact with evidence |
| `diagnostic_emitted` | summary failure kind / layer / lifecycle stage |

## 4. Redaction Rules

Trace artifact に以下を保存してはいけない。

- `OPENAI_API_KEY`, `GEMINI_API_KEY`, provider API key
- raw Authorization header
- full `.env` content
- provider raw response body that may include sensitive prompt payload, unless redacted

保存してよいもの:

- provider 名、model 名
- tool call name
- arguments shape summary
- failure kind
- lifecycle stage
- relative artifact paths
- redacted command line

## 5. Gate Implication

`source_trace_manifest.md` にこの schema を満たさない trace がある場合、該当 gate は `partial` 以下とする。source/MVP の片方だけ trace がある状態で `pass` にしてはいけない。
