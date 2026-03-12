# Phase 3.8 Design: Loop State and Provider I/O Stabilization

日付: 2026-03-12

目的:

- stale cache を防ぐ
- phase truth source を一元化する
- provider-native tool streaming を正しく活用する
- 単一巨大 prompt から message-structured layout へ移行する

設計原則:

- fixed workflow を増やして柔軟性を削らない
- `requirement`, `profile`, `stance` は制約として使い、具体的な tool sequence は model に委ねる
- same task type でも複数の valid path を許容する

## 1. Cache Invalidation Matrix

### 1.1 Cache 対象

read-only observation cache の対象は次に限定する。

- `read_file`
- `list_dir`
- `stat_path`
- `path_exists`
- `glob`
- `search`
- read-only `exec`

非対象:

- `write_file`
- `edit_file`
- `mkdir`
- non-read-only `exec`
- `diff`

### 1.2 Invalidation Rule

| tool | invalidate read_file | invalidate dir/meta (`list_dir/stat/path_exists/glob`) | invalidate search | notes |
|---|---:|---:|---:|---|
| `write_file` | yes if same path or under same output root | yes if path affects directory membership | yes | strongest invalidation |
| `edit_file` | yes if same path | no by default | yes | file content changed |
| `mkdir` | no file-content invalidation | yes for created dir and ancestors under output root | no | directory view changed |
| non-read-only `exec` | global invalidate inside cwd by default | global invalidate inside cwd by default | global invalidate inside cwd by default | fail-safe |
| read-only `exec` | no | no | no | cacheable |

### 1.3 Initial Simplification

Phase 3.8 の初期実装では、複雑な path-selective invalidation ではなく次の 2 段階で入れる。

1. `write_file`, `edit_file`, `mkdir`, non-read-only `exec` 成功後は read-only cache を全破棄
2. 後続で path-scoped invalidation に最適化する

理由:

- correctness を先に取り、stale read を確実に防ぐ
- local model の loop 安定化にまず効く
- path-scoped 最適化は後で入れても設計上後方互換

### 1.4 Path-Scoped Optimization Gate

global invalidation から path-scoped invalidation へ進める条件を先に固定する。

- cache key が `tool + normalized path + cwd snapshot` で一意になる
- output root と tool target path の正規化が終わっている
- verify/review の stale read 回帰テストが揃っている

上の条件が揃うまでは global invalidation を維持する。

### 1.5 Audit Event

cache invalidation が起きたら audit/debug trace に残す。

- `cache_invalidated`
- fields:
  - `reason_tool`
  - `scope` (`global_read_only` / `path_scoped`)
  - `path_hint`
  - `evicted_entries`

## 2. Phase Truth Source Migration

### 2.1 Source of Truth

create task における truth source は `RequirementState.remaining` とする。

`CreatePhase` は残すが、意味は以下へ縮小する。

- UI 表示
- prompt guidance label
- budget tuning の補助情報

`CreatePhase` は `RequirementState` から派生し、独立 heuristic を持たない。

### 2.2 Mapping

| remaining requirements | derived phase |
|---|---|
| contains `OutputRootExists` | `Prepare` |
| contains `DeliverableWritten` | `Write` |
| contains `EntryPointVerified` or `RuntimeVerified` or `RequestedOutputVerified` or `CoreLoopVerified` | `Verify` |
| contains `ReviewCompleted` | `Review` |
| none remaining | `Finalize` |

### 2.3 Migration Rule

Phase 3.8 では次を行う。

1. `create_phase_for_task()` を pure heuristic から `RequirementState -> CreatePhase` 変換関数へ変更
2. fallback heuristic は migration 中だけ残し、create task では使わない
3. `step_purpose`, `step_instruction`, `step_plan`, `build_loop_prompt` は派生 phase を引数で受け取る

### 2.4 Phase Transition Guard

`Verify -> Review` / `Verify -> Finalize` の遷移条件を固定する。

- 最低限必須:
  - `EntryPointVerified`
  - `RequestedOutputVerified`
- task type に応じて必須:
  - browser runnable task では `RuntimeVerified`
  - interactive/game task では `CoreLoopVerified`

これにより、requirement 粒度を上げた結果が phase 遷移へ反映される。

### 2.5 Review Requirement

`review_completed` は phase 依存でなく evidence 依存にする。

例:

- main deliverable の `read_file`
- generated output に対する `diff`
- review findings を含む final draft generation

のいずれかで充足可能とする。

### 2.6 Requirement Granularity

Phase 3.8 の create task では requirement を次に固定する。

- `OutputRootExists`
- `DeliverableWritten`
- `EntryPointVerified`
- `RuntimeVerified`
- `RequestedOutputVerified`
- `CoreLoopVerified`
- `ReviewCompleted`

意味:

- `EntryPointVerified`
  - main deliverable path が存在し、期待する entry file として読める
- `RuntimeVerified`
  - browser で直接実行可能であることを確認できた
- `RequestedOutputVerified`
  - requested output root, requested deliverable kind など task contract の出力条件を確認できた
- `CoreLoopVerified`
  - playable core loop など主要な振る舞いを確認できた

`DeliverableVerified` のような広すぎる単一 requirement は使わない。

### 2.7 Task-Type Requirement Profile

task type ごとに required requirement set を持つ。

例:

- `html_app`
  - `OutputRootExists`
  - `DeliverableWritten`
  - `EntryPointVerified`
  - `RequestedOutputVerified`
- `game`
  - `html_app` の requirement
  - `RuntimeVerified`
  - `CoreLoopVerified`
- `cli_tool`
  - `OutputRootExists`
  - `DeliverableWritten`
  - `RequestedOutputVerified`
- `refactor`
  - `RequestedOutputVerified`
  - `ReviewCompleted`

これは fixed workflow ではなく required evidence profile である。
同じ profile を複数の tool path で満たしてよい。

## 3. Provider Streaming Capability Model

### 3.1 Capability Enum

```rust
enum ToolStreamingCapability {
    Unknown,
    Supported,
    Unsupported,
}
```

provider ごとに runtime で保持する。

### 3.2 Provider Contract

provider 抽象は次を持つ。

- `chat_sync`
- `chat_text_stream`
- `chat_with_tools_sync`
- `chat_with_tools_stream`
- `tool_streaming_capability`
- `runtime_policy`

### 3.3 Provider Runtime Policy

tool streaming 可否と独立に、runtime tuning を provider contract に含める。

最低限の責務:

- tool call 時の temperature 上限制御
- keep-alive の有無
- context window の provider 反映方法
- stream を優先するか sync を優先するか
- timeout
- retry count
- backoff policy
- empty response handling

`vibe-local` の実運用上の優位は capability だけでなく runtime tuning にあるため、ここを abstraction に含める。

### 3.4 Runtime Policy Precedence

runtime policy の適用順は次に固定する。

1. tool mode
  - tool-call reliability を優先
  - temperature cap を適用
  - stream 可能なら stream 優先
2. text-only reasoning mode
  - user-configured temperature を優先
  - provider default stream policy を使う
3. creative final mode
  - quality guidance を優先
  - ただし provider safety / context budget を超えない

同一 turn 内で tool mode と final mode が混在する場合は tool mode を優先する。

### 3.5 Fallback Policy

| capability | requested mode | actual mode |
|---|---|---|
| `Supported` | tools + stream | tool streaming |
| `Unsupported` | tools + stream | sync tools |
| `Unknown` | tools + stream | probe once, then cache result |

### 3.6 Capability Cache Key

probe 結果の cache key は session 単位でなく、少なくとも次を含む。

- provider kind
- endpoint fingerprint
- model name
- provider-reported version if available

これにより、provider 設定変更や model 切替で stale capability を引きずらない。

### 3.7 Probe Rule

Ollama / LM Studio で tool streaming 未知の場合:

1. 小さな no-op tool schema で 1 回 probe
2. stream 中に structured tool delta が来れば `Supported`
3. provider error or no tool delta なら `Unsupported`

同一 capability cache key につき 1 回だけ行う。

### 3.8 Fallback Reason Codes

`provider_stream_fallback` には reason code を持たせる。

- `unsupported_by_provider`
- `probe_failed`
- `tool_delta_missing`
- `stream_parse_error`

### 3.9 UI / Audit

開始時または debug に表示:

- `tool streaming: yes/no/unknown`
- fallback が起きたら `provider_stream_fallback` event を残す

### 3.10 Fallback Ladder

将来拡張として fallback ladder を持てる設計にする。

順序:

1. provider-native tool calling
2. normalized fallback extractor
3. clarification or deny

Phase 3.8 では 1 と deny を中心に実装するが、3 段階の拡張余地を残す。

## 4. Sidecar Compaction Policy

### 4.1 Goal

long-running session の compaction を main model に過度に依存させない。

### 4.2 Policy

- sidecar model が利用可能なら、古い transcript の summary は sidecar に委譲する
- sidecar が無い場合だけ main model へ fallback する
- sidecar summary は decision / evidence / changed paths / unresolved items に限定する

### 4.3 Selection Rule

- main model と別の軽量 model を優先する
- local provider に sidecar 候補が無い場合は sidecar なしでも動作可能にする
- sidecar の失敗は loop failure にしない

### 4.4 Sidecar Selection Strategy

sidecar は次の順で選ぶ。

1. installed models から preferred family を探す
2. main model より軽い tier を優先する
3. RAM / VRAM tier に収まるものを優先する
4. 候補が無ければ main model fallback

これにより、compaction の選定を再現可能にする。

### 4.5 Audit

- `compaction_started`
- `compaction_completed`
- `compaction_fallback_to_main`

を記録できるようにする。

### 4.6 Sidecar Compaction Input/Output Contract

sidecar へ渡す compaction input と output を固定する。

input:

- 長さ上限を持つ transcript excerpt
- raw tool output は全文を渡さず、要点または切り詰めた断片のみ
- tool call は `tool name + key path/arg` へ圧縮してよい

output:

- `decisions`
- `evidence`
- `changed_paths`
- `unresolved_items`

の 4 区分に収まる summary schema とする。

sidecar summary は自由文だけにせず、将来の carryover で再利用しやすい形に寄せる。

### 4.7 Compaction Quality Guard

compaction 後に最低限残るべき情報を確認する。

- `changed_paths`
- `unresolved_items`
- `last_successful_write_target` if any
- current task の required requirement profile

これが失われる compaction は採用しない。

## 5. Message-Structured Prompt Layout

### 5.1 Goal

単一巨大 prompt をやめ、役割ごとに message を分割する。

### 5.2 Message Layout

1. `system`
- core agent policy
- tool usage rules
- safety rules

2. `system`
- project instructions from `ANVIL.md`

3. `system`
- memory summary from `ANVIL-MEMORY.md`

4. `system` or `assistant`
- session carryover summary

5. `user`
- task objective

6. `assistant` / `tool`
- prior tool transcript

7. `system`
- task contract / requirement state / creative guidance

8. `system`
- task-class execution stance

### 5.3 Task-Class Execution Stance

task class ごとに行動原則を contract 化する。

- `inspect`
  - evidence-first
- `edit`
  - minimal-change-first
- `create`
  - deliverable-first
- `review`
  - finding-first

これは `vibe-local` の強い system prompt を、より構造化して持つための層である。

### 5.4 Dynamic Execution Stance

stance は static でなく、次の要素で更新可能にする。

- task class
- current phase
- evidence delta
- remaining budget

例:

- `create + no_progress + low budget`
  - deliverable-first を強める
- `review + conflicting_evidence`
  - finding-first を強める
- `inspect + enough evidence`
  - finalization readiness を強める

ただし、stance は tool choice を強制しない。
model への guidance として使い、柔軟性は維持する。

### 5.4 Working Transcript と Carryover Summary

`message-structured layout` は transcript を単に残すのではなく、二層に分ける。

- `working transcript`
  - 直近の tool call / tool result / assistant reasoning summary
  - raw に近い形で保持する
- `carryover summary`
  - 古い turn を evidence と decision のみへ圧縮した summary

昇格規則:

1. 直近 `N` turn は `working transcript` に残す
2. それ以前は `carryover summary` へ昇格する
3. `carryover summary` には raw tool output を持たず、decision / evidence / changed paths のみを持つ

### 5.5 Transcript Integrity Rules

transcript を圧縮しても tool/result pairing を壊さない。

不変条件:

- assistant `tool_calls` を残すなら対応する tool result も残す
- 対応する tool result を落とすなら assistant `tool_calls` も summary 化する
- compacted transcript の先頭が orphaned tool result にならない
- provider ごとの差分正規化後も pairing identity が保たれる

### 5.6 Pair Replacement Rule

compaction 時に tool_call/tool_result pair を落とす場合は、pair 全体を 1 件の summary note に置き換える。

summary note に最低限含める項目:

- tool name
- key path or key argument
- result kind (`success` / `error`)
- evidence delta
- changed path summary if any

pair の片側だけを残すことはしない。

### 5.7 PermissionState Snapshot

permission を単なる mode ではなく、loop context に見える state として扱う。

最低限含める項目:

- current permission mode
- session allow
- session deny
- dangerous confirm still required

### 5.8 Permission Lifetime Policy

permission state の寿命を固定する。

- session allow / deny は同一 session 内のみ有効
- resume 時に持ち越すのは explicit に許可された場合のみ
- dangerous tool の blanket allow は永続化しない
- permission state は loop context に見せるが、task requirement の代替にはしない

### 5.9 Permission Event Retention Policy

permission event は transcript と carryover summary の両方で扱えるようにする。

raw transcript に残す:

- permission requested
- permission granted / denied
- approval scope (`once` / `session` / `mode`)

carryover summary に残す:

- 危険操作で deny が続いたこと
- session allow / deny の重要な変更
- task 完遂に影響した permission decision

### 5.10 Transcript Retention Policy

保持方針を次に固定する。

- raw のまま優先保持:
  - 最新の assistant tool_calls
  - 最新の tool results
  - current phase に直接関係する evidence
- summary へ昇格:
  - 2 phase 以上前の tool results
  - 同じ path に対する古い read-only observations
  - no-progress の連続結果
- finalize 前に優先保持:
  - main deliverable の verification evidence
  - review findings
  - changed paths summary

### 5.11 Constraints

- `ANVIL.md` と memory は毎回文字列結合せず、loaded snapshot を turn 内で再利用
- long tool output は transcript へ入れる前に truncation / summary を行う
- `quality_targets` / `stretch_goals` は dedicated contract message に置く
- Phase 3.7 の `creative guidance` は contract message に残し、base policy へは混ぜない

### 5.12 Initial Transition

Phase 3.8 では full message replay まで一気に行わない。

初期段階:

1. `system`: base policy
2. `system`: project instructions
3. `system`: memory
4. `system`: requirement/creative contract
5. `user`: objective
6. `system`: task-class execution stance / permission snapshot
6. `assistant/tool`: compacted transcript

これで単一巨大 string よりかなり安定する。

### 5.13 Assistant Transition Note

tool result 後の短い summary は UI 用だけでなく loop context にも残せるようにする。

含める内容:

- 何が分かったか
- remaining requirements がどう減ったか
- 次の step で狙うこと

これは assistant-visible state transition note として扱う。

### 5.14 Next Action Hint Schema

transition note は次の schema を持てるようにする。

- `what_changed`
- `why_it_matters`
- `next_action`

`next_action` は loop 継続のヒントとして必須にできる。

### 5.15 Transition Note Policy

transition note は次の二層で扱う。

- user-visible note
  - UI 表示に最適化
  - 省略可能
- model-visible note
  - loop context に必ず残す
  - evidence delta と next action を含む

初期実装では両者の内容を同一にしてよいが、将来分離可能な設計にする。

### 5.16 User-Facing Summary Hook

tool loop 内では raw tool result だけでなく、短い assistant summary を残せるようにする。

- tool result 後の 1-2 文 summary
- remaining requirements の変化
- 次の step で狙うこと

これは `vibe-local` の tool result 後 summary 運用を取り込むためで、model の継続判断と UX の両方に効く。

## 6. Evidence Delta と Repeat Recovery

### 5.1 Evidence Delta

progress 判定の基礎として evidence delta を次の 4 種に分ける。

- `new_evidence`
- `strengthened_evidence`
- `no_evidence`
- `conflicting_evidence`

`no_progress` は `tool identity` ではなく evidence delta に基づいて判定する。

### 6.2 Repeat Recovery と Fail-Safe

state redesign 後も repeat detector は残す。

対象:

- same read-only call repeated without new evidence
- repeated finalize probe
- same-tool repeated after `no_progress`

方針:

- duplicate detection は fail-safe として維持
- stop 条件は `tool identity` だけでなく `new evidence produced` を加味する
- `finalize` 中の repeated read-only call は `return final now` を優先する

### 6.3 Streaming Fallback Retry Rule

streaming parse failure や tool delta 欠落時の fallback は次に固定する。

1. 同一 request で streaming path を 1 回試す
2. `stream_parse_error` または `tool_delta_missing` なら sync fallback を 1 回だけ試す
3. sync fallback でも失敗したら provider error として上位へ返す

同一 turn で streaming/sync を何度も往復しない。

### 6.4 Safe Fallback Scope

normalized fallback extractor は安全側に限定する。

- 対象:
  - `read-only`
  - `inspect`
  - `low-risk clarification`
- 非対象:
  - `write`
  - `edit`
  - `mkdir`
  - `exec`

これにより、`vibe-local` の実利を取り込みつつ、Anvil の安全性を維持する。

## 7. Clarification Escape Hatch

verification ambiguity や requirement conflict で loop が止まりそうな場合は、将来の clarification path を持てるようにする。

Phase 3.8 では実装しない。将来拡張として次を確保する。

- unmet requirement が相互に矛盾する場合
- repeated `conflicting_evidence` が続く場合
- verification 不能で finalize できない場合

この場合に user clarification tool を差し込める余地を残す。

## 8. Plan / Execution Artifact Split

長時間タスクでは plan artifact と execution artifact を分離できるようにする。

- `plan artifact`
  - plan summary
  - task contract
  - quality targets
- `execution artifact`
  - evidence
  - changed paths
  - review findings
  - unresolved items

この分離により、Plan/Act 的な長時間運用で transcript の責務を減らせる。

## 9. Acceptance Criteria

- write/edit/mkdir の後に stale read-only result を使わない
- create task で `phase` と `remaining requirements` が矛盾しない
- provider が tool streaming を使える場合は sync fallback へ落ちにくい
- prompt/message 構成の責務が明確になり、保守時に差分を追いやすい
- provider runtime policy により tool use の tuning point が abstraction へ乗る
- transcript から carryover summary への昇格規則が固定される
- repeat recovery が state redesign 後も fail-safe として残る
- sidecar compaction の有無で loop correctness が変わらない
- transcript compaction 後も tool/result pairing が壊れない
- profile / stance / requirement が fixed workflow ではなく constraint として機能する
- `vibe-local` よりも task type / permission / compaction の責務分離が明確になる
