# Runtime Semantics Source Parity Audit

作成日: 2026-06-29

このディレクトリは、MVP `anvilminimal` が移植元 `anvildev` の runtime semantics をどこまで再現できているかを監査した結果をまとめる。

今回の `021-4` は実装修正ではなく、移植不備と意味論ズレの洗い出し、および対応順序の整理を目的とする。

## 1. 結論

現在の主問題は、個別の `Next.js` 失敗や `ultra-plan-run` 失敗ではなく、移植元が持っていた次の連鎖が MVP 側で分断されていること。

```text
task contract
  -> plan / phase contract
  -> step prompt
  -> tool execution
  -> verify
  -> bounded repair
  -> final acceptance
  -> diagnostic / recovery handoff
```

コード片は一部移植済みだが、この連鎖の意味が完全には戻っていない。そのため、スコア上は高く見えても、実際の成果物が静的 shell で止まる、capability が実装されない、失敗時の回復導線が弱い、といった問題が残っている。

## 2. 対応順序

### 021-5: phase/profile continuation semantics を戻す

対象 gap:

- RSSP-04: profile runtime contract prompt
- RSSP-03: profile repair session continuity
- RSSP-01: fallback scaffold continuation gap
- RSSP-09: boundedness / recursion guard gap
- RSSP-10: provider / LLM uncertainty

狙い:

- ultra phase prompt に profile-specific runtime contract を戻す。
- profile repair でも phase context / session context を維持する。
- scaffold / profile repair を完了扱いせず、task-specific implementation continuation へ戻す。
- repair を増やす前に boundedness を明示する。

理由:

ここは比較的小さい prompt/session/context plumbing で直せるうえ、手動 UAT の「buildable shell で止まる」問題に直結している。最初に戻すべき移植不備。

検証:

- phase prompt fixture
- profile repair が prior phase context を参照できる fixture
- scaffold-only app が成功扱いにならず continuation target を出す fixture
- OpenAI/Gemini/Ollama の小さい provider probe
- ultra-plan-run smoke

### 021-6: final acceptance repair と diagnostics を bounded に戻す

対象 gap:

- RSSP-02: final acceptance repair gap
- RSSP-07: diagnostics parity gap
- RSSP-08: eval metric lifecycle mapping gap
- RSSP-09: boundedness / recursion guard gap

狙い:

- final acceptance failure を即 handoff だけで終わらせず、1回など bounded repair に接続する。
- それでも失敗した場合は、明示的な recovery handoff に落とす。
- failure stage を planning/runtime/bridge/provider だけでなく lifecycle stage として記録する。

理由:

021-5 で continuation の土台を戻したあとに実施する。先に final repair を入れると、長い repair loop や false recovery のリスクが高い。

検証:

- final acceptance failure が bounded repair に入る fixture
- repair exhaustion で suggested recovery command が出る fixture
- TUI / CLI で lifecycle stage が見える fixture
- eval の main score / diagnostic score が混同されないこと

### 021-7: dependency/setup/build lifecycle parity を戻す

対象 gap:

- RSSP-05: dependency/setup/build lifecycle gap

狙い:

- dependency missing
- setup authority
- package manifest
- install/setup
- build rerun
- build verification

を一つの lifecycle として扱う。

理由:

MVP には `NodeDependencySetupAuthority` や `BuildVerifierLifecycleObservation` があるが、全 mode に同じ意味で接続されていない。高スコア plan が実行で落ちる原因になりやすい。

検証:

- dependency missing -> setup blocked
- dependency missing -> setup allowed -> build rerun
- setup-only で成功扱いしない
- package manifest がない状態で build/test verify を要求しない
- plan-run / ultra-plan-run の両方で同じ lifecycle event が出る

### 021-8: TaskContract-lite / obligation tracking を設計する

対象 gap:

- RSSP-06: TaskContract / obligation tracking gap

狙い:

- 「何を作るべきか」
- 「どの artifact がどの capability を満たすべきか」
- 「setup / scaffold / implementation / verify のどれが完了条件か」

を controller 側で薄く追跡する。

理由:

移植元の `TaskContract` 全体を丸ごと戻すと MVP の複雑性が急増する。一方で、今の `StepPlan` と `CompletionContract` だけでは、scaffold-only / setup-only / style-only を成功扱いしやすい。まずは TaskContract-lite として設計を固定する。

検証:

- setup-only は success にならない
- scaffold-only は success にならない
- docs-only implementation が app/game task を満たさない
- expected artifact と capability evidence の対応が追跡される

### 021-9: provider probe harness を整備する

対象 gap:

- RSSP-10: provider / LLM behavior uncertainty gap

狙い:

- prompt-sensitive fix の前後で OpenAI/Gemini/Ollama の挙動を小さく確認する。
- fake client の fixture だけで prompt 改修を完了扱いしない。

理由:

MVP では provider ごとの tool call / YAML / repair prompt の揺れが実障害として出ている。runtime に LLM judge を入れるのではなく、修正前後の probe と smoke で不確実性を閉じる。

検証:

- OpenAI tool args shape
- Gemini function calling / schema behavior
- Ollama XML fallback / tool-like output
- retry / repair prompt への応答形

## 3. 優先順位の考え方

優先順位は次の基準で決めた。

1. 手動 UAT の失敗に直結しているか
2. 移植元との差分が明確か
3. 小さく戻せるか
4. repair loop 追加による不安定化リスクが低いか
5. 後続修正の土台になるか
6. eval スコアではなく accepted artifact の品質に効くか

このため、最初に `021-5` で prompt/session/continuation を戻す。`final acceptance repair` や `dependency lifecycle` は重要だが、先に boundedness と continuation を固めないと不安定な retry を増やすだけになる。

## 4. 単純なファイル移植では不十分だった理由

不十分だった。

理由は、移植元の強さが単一ファイルや単一関数ではなく、複数レイヤの連携によって成立しているため。

### 4.1 実行意味論が分散している

移植元では、completion / repair / fallback / verification / diagnostic の意味が複数箇所に分散している。

- `minimal_step_runner`
- `loop_run`
- `scaffold_pipeline`
- `task_contract`
- `profile`
- `repair`
- session / event log

そのため、あるファイルだけをコピーしても、別レイヤにある前提が欠けると意味が変わる。

例:

- fallback scaffold は移植元では「回復用 scaffold + 継続指示」。
- MVP では一時期「薄い計画または構造補修」として扱われ、完了に近い意味になった。

これはファイル単位では見えにくい意味論ズレ。

### 4.2 MVP は API を切り直している

MVP は元コードの丸ごと縮小版ではなく、小さい API と module に切り直している。

そのため、移植元の型や関数をそのまま持ち込むと、次の問題が起きる。

- 依存 graph が大きくなる
- source の actor loop 全体へ引き戻される
- MVP の単純な `minimal loop + YAML plan + deterministic verify` という境界が崩れる
- provider / TUI / eval の責任分界が曖昧になる

つまり、ファイルコピーではなく、source の lifecycle responsibility を MVP の小さい抽象へ写像する必要があった。

### 4.3 コピーできるものと写像すべきものが違う

そのまま寄せやすいもの:

- prompt に含めるべき contract
- session を渡す箇所
- retry / fail-fast の条件
- failure taxonomy
- boundedness の上限

MVP 向けに写像すべきもの:

- full `TaskContract`
- full `RepairJob`
- actor loop 全体の fallback graph
- source 固有の session/event 実装
- profile ごとの巨大 verifier

今回の監査では、後者を丸ごと移植するのではなく、`TaskContract-lite` や `RepairTarget` のような小さい契約に落とす方針にしている。

### 4.4 「コードがある」だけでは挙動が同じにならない

MVP にはすでに以下のような部品がある。

- profile verifier
- auto repair
- runtime acceptance
- dependency setup authority
- build verifier lifecycle
- eval events

しかし、これらが source と同じ lifecycle の順序・境界・停止条件で接続されていなければ、挙動は同じにならない。

今回の問題は、部品不足だけではなく、接続順序と completion authority のズレでもある。

### 4.5 複数関連ファイルをまとめて移植する案について

複数の関連ファイルをごっそり移植する案は、選択肢としてはあり得た。

ただし、今回の MVP では次の理由でそのまま採用しない方がよい。

1. 実際の関連範囲が非常に広い
   - `minimal_step_runner` だけでは足りず、`loop_run` 配下の contract / repair / verifier / scaffold / evidence / event / recovery まで波及する。
   - 依存を追うと、数ファイルではなく数十ファイル規模になる。
2. 移植元の型をそのまま持つと MVP の API 境界が崩れる
   - MVP は `minimal loop + YAML plan + deterministic verify` を小さい単位で切り出す方針。
   - full actor loop / full TaskContract / full RepairJob を戻すと、MVP ではなく移植元の縮小コピーになる。
3. 既に MVP 側に同じ責務の小さい部品がある
   - `RepairTarget`
   - `RuntimeAcceptanceReport`
   - `NodeDependencySetupAuthority`
   - `BuildVerifierLifecycleObservation`
   - eval oracle / acceptance scoring
   - これらを捨てて source 型をコピーすると、二重実装や責任重複が起きる。
4. 問題は「部品不足」より「意味論の接続不備」
   - source の挙動を戻すには、ファイルコピーよりも lifecycle responsibility を固定する必要がある。
   - たとえば fallback scaffold は、ファイル生成処理ではなく「回復 scaffold を作ったあと実装継続へ戻す」という意味が重要。

したがって、方針は次のどちらかになる。

| 方針 | 内容 | 評価 |
| --- | --- | --- |
| bulk source port | source の関連ファイル群を依存ごと持ち込む | source parity は上げやすいが、MVP の小ささと責任分界を失いやすい |
| semantics adapter | source の lifecycle responsibility を MVP の小さい型へ写像する | 実装は慎重になるが、MVP の設計を維持できる |

今回の `021-5`〜`021-9` は後者を採用する。ただし、各 phase では source の該当ファイルを必ず参照し、「何をコピーしないか」ではなく「どの挙動を MVP のどの責務に写像するか」を明示する。

## 5. 今後の進め方

一度に大きく戻すと 021 の rollback 前と同じリスクがある。次は次の順序で小さく進める。

1. `021-5`: prompt/session/continuation に限定して戻す
2. 評価と手動 UAT で accepted artifact を確認する
3. `021-6`: bounded final acceptance repair と diagnostics を戻す
4. `021-7`: dependency/setup/build lifecycle を戻す
5. `021-8`: TaskContract-lite を設計して、scaffold-only / setup-only を completion から外す
6. `021-9`: provider probe harness で prompt-sensitive fix の不確実性を下げる

この順序であれば、MVP の小ささを維持しつつ、移植元の runtime semantics を段階的に戻せる。

## 6. Phase別参照ファイルと Codex 指示

以下は、各 phase を別タスクとして Codex に渡すための参照ファイルと指示文。

### 6.1 021-5: phase/profile continuation semantics

参照ファイル:

Source:

- `/Users/maenokota/share/work/github_kewton/Anvil-develop/src/agent/minimal_step_runner.rs`
- `/Users/maenokota/share/work/github_kewton/Anvil-develop/src/agent/minimal_step_runner/profile.rs`
- `/Users/maenokota/share/work/github_kewton/Anvil-develop/src/agent/minimal_step_runner/repair.rs`
- `/Users/maenokota/share/work/github_kewton/Anvil-develop/src/agent/loop_run/actor_loop_flow.rs`
- `/Users/maenokota/share/work/github_kewton/Anvil-develop/src/agent/loop_run/scaffold_pipeline.rs`
- `/Users/maenokota/share/work/github_kewton/Anvil-develop/src/agent/loop_run/deterministic_fallback_plan.rs`
- `/Users/maenokota/share/work/github_kewton/Anvil-develop/src/agent/loop_run/recovery_messages.rs`
- `/Users/maenokota/share/work/github_kewton/Anvil-develop/src/agent/loop_run/working_memory_messages.rs`

MVP:

- `/Users/maenokota/share/work/github_kewton/Anvil-develop/mvp/anvilminimal/src/planner/runner.rs`
- `/Users/maenokota/share/work/github_kewton/Anvil-develop/mvp/anvilminimal/src/planner/profiles/nextjs.rs`
- `/Users/maenokota/share/work/github_kewton/Anvil-develop/mvp/anvilminimal/src/planner/repair.rs`
- `/Users/maenokota/share/work/github_kewton/Anvil-develop/mvp/anvilminimal/src/minimal_loop/repair_target.rs`
- `/Users/maenokota/share/work/github_kewton/Anvil-develop/mvp/anvilminimal/src/minimal_loop/feedback.rs`
- `/Users/maenokota/share/work/github_kewton/Anvil-develop/mvp/anvilminimal/src/eval_events.rs`

Tests / eval:

- `/Users/maenokota/share/work/github_kewton/Anvil-develop/mvp/anvilminimal/tests`
- `/Users/maenokota/share/work/github_kewton/Anvil-develop/mvp/anvilminimal/eval/fixtures`
- `/Users/maenokota/share/work/github_kewton/Anvil-develop/mvp/anvilminimal/scripts/eval_lib/runtime_scoring.py`
- `/Users/maenokota/share/work/github_kewton/Anvil-develop/mvp/anvilminimal/scripts/eval_lib/source_semantic_oracle.py`

Codex 指示:

```text
workspace/mvp/eval/021-4/README.md の 021-5 に従って、phase/profile continuation semantics を MVP anvilminimal に戻してください。

目的:
- ultra phase prompt に profile-specific runtime contract を戻す。
- profile repair でも phase/session context を維持する。
- scaffold/profile repair を完了扱いせず、task-specific implementation continuation に戻す。
- repair 増加前に boundedness を明示する。

必ず参照する source:
- /Users/maenokota/share/work/github_kewton/Anvil-develop/src/agent/minimal_step_runner.rs
- /Users/maenokota/share/work/github_kewton/Anvil-develop/src/agent/minimal_step_runner/profile.rs
- /Users/maenokota/share/work/github_kewton/Anvil-develop/src/agent/minimal_step_runner/repair.rs
- /Users/maenokota/share/work/github_kewton/Anvil-develop/src/agent/loop_run/actor_loop_flow.rs
- /Users/maenokota/share/work/github_kewton/Anvil-develop/src/agent/loop_run/scaffold_pipeline.rs

主な編集候補:
- /Users/maenokota/share/work/github_kewton/Anvil-develop/mvp/anvilminimal/src/planner/runner.rs
- /Users/maenokota/share/work/github_kewton/Anvil-develop/mvp/anvilminimal/src/planner/profiles/nextjs.rs
- /Users/maenokota/share/work/github_kewton/Anvil-develop/mvp/anvilminimal/src/planner/repair.rs
- /Users/maenokota/share/work/github_kewton/Anvil-develop/mvp/anvilminimal/src/eval_events.rs

制約:
- source の actor loop / TaskContract / RepairJob 全体を丸ごと移植しない。
- MVP の小さい API 境界を維持する。
- scenario 固有の Space Invaders 対応にしない。
- automatic repair を増やす場合は必ず max attempt と handoff を定義する。

受入条件:
- phase prompt fixture で profile-specific runtime contract が確認できる。
- profile repair が prior phase context を参照できる。
- scaffold-only app が success にならず continuation target を出す。
- cargo test --manifest-path mvp/anvilminimal/Cargo.toml が通る。
- eval smoke で ultra-plan-run の failure が悪化した場合は、correct failure detection と regression を分けて分析する。
```

### 6.2 021-6: bounded final acceptance repair and diagnostics

参照ファイル:

Source:

- `/Users/maenokota/share/work/github_kewton/Anvil-develop/src/agent/minimal_step_runner.rs`
- `/Users/maenokota/share/work/github_kewton/Anvil-develop/src/agent/minimal_step_runner/repair.rs`
- `/Users/maenokota/share/work/github_kewton/Anvil-develop/src/agent/loop_run/actor_loop_flow.rs`
- `/Users/maenokota/share/work/github_kewton/Anvil-develop/src/agent/loop_run/verifier.rs`
- `/Users/maenokota/share/work/github_kewton/Anvil-develop/src/agent/loop_run/verifier_driver.rs`
- `/Users/maenokota/share/work/github_kewton/Anvil-develop/src/agent/loop_run/verifier_repair_targeting.rs`
- `/Users/maenokota/share/work/github_kewton/Anvil-develop/src/agent/loop_run/repair_job.rs`
- `/Users/maenokota/share/work/github_kewton/Anvil-develop/src/agent/loop_run/repair_lifecycle.rs`
- `/Users/maenokota/share/work/github_kewton/Anvil-develop/src/agent/loop_run/emit_verifier_events.rs`
- `/Users/maenokota/share/work/github_kewton/Anvil-develop/src/agent/loop_run/failure_packet.rs`

MVP:

- `/Users/maenokota/share/work/github_kewton/Anvil-develop/mvp/anvilminimal/src/planner/runner.rs`
- `/Users/maenokota/share/work/github_kewton/Anvil-develop/mvp/anvilminimal/src/planner/repair.rs`
- `/Users/maenokota/share/work/github_kewton/Anvil-develop/mvp/anvilminimal/src/minimal_loop/evidence.rs`
- `/Users/maenokota/share/work/github_kewton/Anvil-develop/mvp/anvilminimal/src/minimal_loop/completion.rs`
- `/Users/maenokota/share/work/github_kewton/Anvil-develop/mvp/anvilminimal/src/minimal_loop/repair_target.rs`
- `/Users/maenokota/share/work/github_kewton/Anvil-develop/mvp/anvilminimal/src/minimal_loop/repair_progress.rs`
- `/Users/maenokota/share/work/github_kewton/Anvil-develop/mvp/anvilminimal/src/eval_events.rs`

Tests / eval:

- `/Users/maenokota/share/work/github_kewton/Anvil-develop/mvp/anvilminimal/tests/eval/test_failure_classification.py`
- `/Users/maenokota/share/work/github_kewton/Anvil-develop/mvp/anvilminimal/tests/eval/test_runtime_scoring.py`
- `/Users/maenokota/share/work/github_kewton/Anvil-develop/mvp/anvilminimal/tests/eval/test_source_semantic_oracle.py`
- `/Users/maenokota/share/work/github_kewton/Anvil-develop/mvp/anvilminimal/scripts/eval_lib/failure_classification.py`
- `/Users/maenokota/share/work/github_kewton/Anvil-develop/mvp/anvilminimal/scripts/eval_lib/runtime_scoring.py`
- `/Users/maenokota/share/work/github_kewton/Anvil-develop/mvp/anvilminimal/scripts/eval_lib/source_semantic_oracle.py`

Codex 指示:

```text
workspace/mvp/eval/021-4/README.md の 021-6 に従って、final acceptance repair と diagnostics を bounded に戻してください。

目的:
- final acceptance failure を即 handoff だけで終わらせず、bounded repair に接続する。
- repair で回復できない場合は明示的な recovery handoff に落とす。
- failure を lifecycle stage として event/eval に記録する。

必ず参照する source:
- /Users/maenokota/share/work/github_kewton/Anvil-develop/src/agent/minimal_step_runner/repair.rs
- /Users/maenokota/share/work/github_kewton/Anvil-develop/src/agent/loop_run/verifier.rs
- /Users/maenokota/share/work/github_kewton/Anvil-develop/src/agent/loop_run/verifier_driver.rs
- /Users/maenokota/share/work/github_kewton/Anvil-develop/src/agent/loop_run/verifier_repair_targeting.rs
- /Users/maenokota/share/work/github_kewton/Anvil-develop/src/agent/loop_run/repair_lifecycle.rs
- /Users/maenokota/share/work/github_kewton/Anvil-develop/src/agent/loop_run/emit_verifier_events.rs

主な編集候補:
- /Users/maenokota/share/work/github_kewton/Anvil-develop/mvp/anvilminimal/src/planner/runner.rs
- /Users/maenokota/share/work/github_kewton/Anvil-develop/mvp/anvilminimal/src/planner/repair.rs
- /Users/maenokota/share/work/github_kewton/Anvil-develop/mvp/anvilminimal/src/minimal_loop/evidence.rs
- /Users/maenokota/share/work/github_kewton/Anvil-develop/mvp/anvilminimal/src/eval_events.rs
- /Users/maenokota/share/work/github_kewton/Anvil-develop/mvp/anvilminimal/scripts/eval_lib/failure_classification.py
- /Users/maenokota/share/work/github_kewton/Anvil-develop/mvp/anvilminimal/scripts/eval_lib/runtime_scoring.py

制約:
- 無制限 repair loop を入れない。
- runtime に LLM judge を入れない。
- main success score と diagnostic score を混同しない。
- final acceptance を甘くしない。false positive を減らす方向を優先する。

受入条件:
- final acceptance failure が 1 回など bounded repair に入る fixture がある。
- bounded repair 失敗時に recovery handoff と suggested command が保存される。
- lifecycle stage が event / eval summary で確認できる。
- pytest mvp/anvilminimal/tests/eval が通る。
- cargo test --manifest-path mvp/anvilminimal/Cargo.toml が通る。
```

### 6.3 021-7: dependency/setup/build lifecycle parity

参照ファイル:

Source:

- `/Users/maenokota/share/work/github_kewton/Anvil-develop/src/agent/loop_run/node_request_helpers.rs`
- `/Users/maenokota/share/work/github_kewton/Anvil-develop/src/agent/loop_run/node_runner_manifest.rs`
- `/Users/maenokota/share/work/github_kewton/Anvil-develop/src/agent/loop_run/package_manifest_summary.rs`
- `/Users/maenokota/share/work/github_kewton/Anvil-develop/src/agent/loop_run/project_probe.rs`
- `/Users/maenokota/share/work/github_kewton/Anvil-develop/src/agent/loop_run/project_profile.rs`
- `/Users/maenokota/share/work/github_kewton/Anvil-develop/src/agent/loop_run/project_verifier.rs`
- `/Users/maenokota/share/work/github_kewton/Anvil-develop/src/agent/loop_run/verifier_command_policy.rs`
- `/Users/maenokota/share/work/github_kewton/Anvil-develop/src/agent/loop_run/setup_artifact_validation.rs`
- `/Users/maenokota/share/work/github_kewton/Anvil-develop/src/agent/loop_run/recovery_targets.rs`

MVP:

- `/Users/maenokota/share/work/github_kewton/Anvil-develop/mvp/anvilminimal/src/minimal_loop/dependency_setup.rs`
- `/Users/maenokota/share/work/github_kewton/Anvil-develop/mvp/anvilminimal/src/minimal_loop/build_verifier.rs`
- `/Users/maenokota/share/work/github_kewton/Anvil-develop/mvp/anvilminimal/src/minimal_loop/import_scan.rs`
- `/Users/maenokota/share/work/github_kewton/Anvil-develop/mvp/anvilminimal/src/minimal_loop/verifier_bootstrap.rs`
- `/Users/maenokota/share/work/github_kewton/Anvil-develop/mvp/anvilminimal/src/minimal_loop/repair_target.rs`
- `/Users/maenokota/share/work/github_kewton/Anvil-develop/mvp/anvilminimal/src/planner/runner.rs`
- `/Users/maenokota/share/work/github_kewton/Anvil-develop/mvp/anvilminimal/src/planner/verify.rs`
- `/Users/maenokota/share/work/github_kewton/Anvil-develop/mvp/anvilminimal/src/planner/profiles/nextjs.rs`

Tests / eval:

- `/Users/maenokota/share/work/github_kewton/Anvil-develop/mvp/anvilminimal/tests`
- `/Users/maenokota/share/work/github_kewton/Anvil-develop/mvp/anvilminimal/tests/eval/test_plan_verify_coverage.py`
- `/Users/maenokota/share/work/github_kewton/Anvil-develop/mvp/anvilminimal/tests/eval/test_runtime_scoring.py`
- `/Users/maenokota/share/work/github_kewton/Anvil-develop/mvp/anvilminimal/eval/fixtures`

Codex 指示:

```text
workspace/mvp/eval/021-4/README.md の 021-7 に従って、dependency/setup/build lifecycle parity を戻してください。

目的:
- dependency missing -> setup authority -> install/setup -> build rerun -> verification を一つの lifecycle として扱う。
- package manifest がない状態で build/test verify を要求しない。
- setup-only / manifest-only を success 扱いしない。
- plan-run / ultra-plan-run / minimal-loop で同じ dependency lifecycle event を出す。

必ず参照する source:
- /Users/maenokota/share/work/github_kewton/Anvil-develop/src/agent/loop_run/node_request_helpers.rs
- /Users/maenokota/share/work/github_kewton/Anvil-develop/src/agent/loop_run/node_runner_manifest.rs
- /Users/maenokota/share/work/github_kewton/Anvil-develop/src/agent/loop_run/project_probe.rs
- /Users/maenokota/share/work/github_kewton/Anvil-develop/src/agent/loop_run/project_verifier.rs
- /Users/maenokota/share/work/github_kewton/Anvil-develop/src/agent/loop_run/verifier_command_policy.rs

主な編集候補:
- /Users/maenokota/share/work/github_kewton/Anvil-develop/mvp/anvilminimal/src/minimal_loop/dependency_setup.rs
- /Users/maenokota/share/work/github_kewton/Anvil-develop/mvp/anvilminimal/src/minimal_loop/build_verifier.rs
- /Users/maenokota/share/work/github_kewton/Anvil-develop/mvp/anvilminimal/src/minimal_loop/repair_target.rs
- /Users/maenokota/share/work/github_kewton/Anvil-develop/mvp/anvilminimal/src/planner/runner.rs
- /Users/maenokota/share/work/github_kewton/Anvil-develop/mvp/anvilminimal/src/planner/verify.rs

制約:
- network install を無条件に実行しない。
- dependency setup は authority と workspace state に基づいて制御する。
- shell control syntax を許可する方向に緩めない。
- Next.js 専用の個別条件に閉じない。JS framework lifecycle として整理する。

受入条件:
- dependency missing -> setup blocked の fixture がある。
- dependency missing -> setup allowed -> build rerun の fixture がある。
- setup-only は success にならない。
- plan-run / ultra-plan-run / minimal-loop の event が同じ lifecycle taxonomy を使う。
- cargo test --manifest-path mvp/anvilminimal/Cargo.toml が通る。
```

### 6.4 021-8: TaskContract-lite / obligation tracking

参照ファイル:

Source:

- `/Users/maenokota/share/work/github_kewton/Anvil-develop/src/agent/loop_run/task_contract.rs`
- `/Users/maenokota/share/work/github_kewton/Anvil-develop/src/agent/loop_run/task_contract_core.rs`
- `/Users/maenokota/share/work/github_kewton/Anvil-develop/src/agent/loop_run/task_contract_completion_policy.rs`
- `/Users/maenokota/share/work/github_kewton/Anvil-develop/src/agent/loop_run/task_contract_artifact_contract.rs`
- `/Users/maenokota/share/work/github_kewton/Anvil-develop/src/agent/loop_run/task_contract_deliverable_lifecycle.rs`
- `/Users/maenokota/share/work/github_kewton/Anvil-develop/src/agent/loop_run/task_contract_obligation_planning.rs`
- `/Users/maenokota/share/work/github_kewton/Anvil-develop/src/agent/loop_run/task_contract_path_context.rs`
- `/Users/maenokota/share/work/github_kewton/Anvil-develop/src/agent/loop_run/objective_contract_projection.rs`
- `/Users/maenokota/share/work/github_kewton/Anvil-develop/src/agent/loop_run/artifact_completion_record.rs`
- `/Users/maenokota/share/work/github_kewton/Anvil-develop/src/agent/loop_run/artifact_state_projection.rs`
- `/Users/maenokota/share/work/github_kewton/Anvil-develop/src/agent/loop_run/evidence_binding.rs`

MVP:

- `/Users/maenokota/share/work/github_kewton/Anvil-develop/mvp/anvilminimal/src/minimal_loop/completion.rs`
- `/Users/maenokota/share/work/github_kewton/Anvil-develop/mvp/anvilminimal/src/minimal_loop/evidence.rs`
- `/Users/maenokota/share/work/github_kewton/Anvil-develop/mvp/anvilminimal/src/minimal_loop/repair_target.rs`
- `/Users/maenokota/share/work/github_kewton/Anvil-develop/mvp/anvilminimal/src/planner/step_plan.rs`
- `/Users/maenokota/share/work/github_kewton/Anvil-develop/mvp/anvilminimal/src/planner/runner.rs`
- `/Users/maenokota/share/work/github_kewton/Anvil-develop/mvp/anvilminimal/src/planner/profiles/nextjs.rs`
- `/Users/maenokota/share/work/github_kewton/Anvil-develop/mvp/anvilminimal/scripts/eval_lib/plan_capability_contract.py`
- `/Users/maenokota/share/work/github_kewton/Anvil-develop/mvp/anvilminimal/scripts/eval_lib/acceptance_contract.py`
- `/Users/maenokota/share/work/github_kewton/Anvil-develop/mvp/anvilminimal/scripts/eval_lib/plan_output_adherence.py`

Tests / eval:

- `/Users/maenokota/share/work/github_kewton/Anvil-develop/mvp/anvilminimal/tests/eval/test_plan_capability_contract.py`
- `/Users/maenokota/share/work/github_kewton/Anvil-develop/mvp/anvilminimal/tests/eval/test_acceptance_contract.py`
- `/Users/maenokota/share/work/github_kewton/Anvil-develop/mvp/anvilminimal/tests/eval/test_plan_output_adherence.py`
- `/Users/maenokota/share/work/github_kewton/Anvil-develop/mvp/anvilminimal/tests/eval/test_false_positive_regression.py`

Codex 指示:

```text
workspace/mvp/eval/021-4/README.md の 021-8 に従って、TaskContract-lite / obligation tracking を設計・実装してください。

目的:
- full TaskContract を丸ごと移植せず、MVP に必要な obligation tracking を薄く導入する。
- setup / scaffold / implementation / verify / acceptance の役割を分ける。
- scaffold-only / setup-only / style-only を completion から外す。
- expected artifact と capability evidence の対応を controller 側で追跡する。

必ず参照する source:
- /Users/maenokota/share/work/github_kewton/Anvil-develop/src/agent/loop_run/task_contract.rs
- /Users/maenokota/share/work/github_kewton/Anvil-develop/src/agent/loop_run/task_contract_core.rs
- /Users/maenokota/share/work/github_kewton/Anvil-develop/src/agent/loop_run/task_contract_completion_policy.rs
- /Users/maenokota/share/work/github_kewton/Anvil-develop/src/agent/loop_run/task_contract_artifact_contract.rs
- /Users/maenokota/share/work/github_kewton/Anvil-develop/src/agent/loop_run/task_contract_deliverable_lifecycle.rs
- /Users/maenokota/share/work/github_kewton/Anvil-develop/src/agent/loop_run/evidence_binding.rs

主な編集候補:
- /Users/maenokota/share/work/github_kewton/Anvil-develop/mvp/anvilminimal/src/minimal_loop/completion.rs
- /Users/maenokota/share/work/github_kewton/Anvil-develop/mvp/anvilminimal/src/minimal_loop/evidence.rs
- /Users/maenokota/share/work/github_kewton/Anvil-develop/mvp/anvilminimal/src/planner/step_plan.rs
- /Users/maenokota/share/work/github_kewton/Anvil-develop/mvp/anvilminimal/src/planner/runner.rs
- /Users/maenokota/share/work/github_kewton/Anvil-develop/mvp/anvilminimal/scripts/eval_lib/plan_capability_contract.py
- /Users/maenokota/share/work/github_kewton/Anvil-develop/mvp/anvilminimal/scripts/eval_lib/acceptance_contract.py

制約:
- full TaskContract graph を移植しない。
- 新しい大きな抽象を作る前に、CompletionContract / RuntimeAcceptanceReport / RepairTarget の拡張で済むか検討する。
- eval の特定 scenario 文字列に過適応しない。
- role は最小限にする。例: setup, scaffold, implementation, verification, acceptance evidence。

受入条件:
- setup-only は success にならない。
- scaffold-only は success にならない。
- app/game task に対する docs-only output は acceptance を満たさない。
- required capability と artifact evidence の対応が fixture で確認できる。
- pytest mvp/anvilminimal/tests/eval が通る。
- cargo test --manifest-path mvp/anvilminimal/Cargo.toml が通る。
```

### 6.5 021-9: provider probe harness

参照ファイル:

Source:

- `/Users/maenokota/share/work/github_kewton/Anvil-develop/src/ollama/client.rs`
- `/Users/maenokota/share/work/github_kewton/Anvil-develop/src/ollama/xml_fallback.rs`
- `/Users/maenokota/share/work/github_kewton/Anvil-develop/src/agent/minimal_step_runner.rs`
- `/Users/maenokota/share/work/github_kewton/Anvil-develop/src/agent/minimal_step_runner/repair.rs`
- `/Users/maenokota/share/work/github_kewton/Anvil-develop/src/agent/minimal_loop/prompt.rs`

MVP:

- `/Users/maenokota/share/work/github_kewton/Anvil-develop/mvp/anvilminimal/src/providers/openai.rs`
- `/Users/maenokota/share/work/github_kewton/Anvil-develop/mvp/anvilminimal/src/providers/gemini.rs`
- `/Users/maenokota/share/work/github_kewton/Anvil-develop/mvp/anvilminimal/src/providers/ollama.rs`
- `/Users/maenokota/share/work/github_kewton/Anvil-develop/mvp/anvilminimal/src/providers/xml_fallback.rs`
- `/Users/maenokota/share/work/github_kewton/Anvil-develop/mvp/anvilminimal/src/planner/runner.rs`
- `/Users/maenokota/share/work/github_kewton/Anvil-develop/mvp/anvilminimal/src/planner/step_plan.rs`
- `/Users/maenokota/share/work/github_kewton/Anvil-develop/mvp/anvilminimal/src/planner/ultra_plan.rs`

Tests / eval:

- `/Users/maenokota/share/work/github_kewton/Anvil-develop/mvp/anvilminimal/tests/live_provider.rs`
- `/Users/maenokota/share/work/github_kewton/Anvil-develop/mvp/anvilminimal/tests/eval/test_planner_provider_request_fixtures.py`
- `/Users/maenokota/share/work/github_kewton/Anvil-develop/mvp/anvilminimal/eval/fixtures/planner_requests`
- `/Users/maenokota/share/work/github_kewton/Anvil-develop/mvp/anvilminimal/eval/suites/mvp-provider-smoke.yaml`
- `/Users/maenokota/share/work/github_kewton/Anvil-develop/mvp/anvilminimal/scripts/eval-run.py`
- `/Users/maenokota/share/work/github_kewton/Anvil-develop/mvp/anvilminimal/scripts/eval-report.py`

Codex 指示:

```text
workspace/mvp/eval/021-4/README.md の 021-9 に従って、provider probe harness を整備してください。

目的:
- prompt-sensitive fix の前後で OpenAI/Gemini/Ollama の挙動を小さく検証できるようにする。
- fake client fixture だけで prompt / repair / tool-call 変更を完了扱いしない。
- provider probe は runtime の LLM judge ではなく、修正前後の不確実性を下げるための小さい検証とする。

必ず参照する source:
- /Users/maenokota/share/work/github_kewton/Anvil-develop/src/ollama/client.rs
- /Users/maenokota/share/work/github_kewton/Anvil-develop/src/ollama/xml_fallback.rs
- /Users/maenokota/share/work/github_kewton/Anvil-develop/src/agent/minimal_step_runner.rs
- /Users/maenokota/share/work/github_kewton/Anvil-develop/src/agent/minimal_step_runner/repair.rs

主な編集候補:
- /Users/maenokota/share/work/github_kewton/Anvil-develop/mvp/anvilminimal/src/providers/openai.rs
- /Users/maenokota/share/work/github_kewton/Anvil-develop/mvp/anvilminimal/src/providers/gemini.rs
- /Users/maenokota/share/work/github_kewton/Anvil-develop/mvp/anvilminimal/src/providers/ollama.rs
- /Users/maenokota/share/work/github_kewton/Anvil-develop/mvp/anvilminimal/tests/live_provider.rs
- /Users/maenokota/share/work/github_kewton/Anvil-develop/mvp/anvilminimal/eval/suites/mvp-provider-smoke.yaml
- /Users/maenokota/share/work/github_kewton/Anvil-develop/mvp/anvilminimal/scripts/eval-run.py

制約:
- provider abstraction を不必要に増やさない。
- probe は任意実行にし、通常 test が API key や network を必須にしない。
- `.env` の OPENAI_API_KEY / GEMINI_API_KEY がある場合だけ live probe を実行する。
- provider 固有の揺れを runtime 成功扱いにしない。schema/tool args/repair prompt の観測として記録する。

受入条件:
- OpenAI tool args shape の probe がある。
- Gemini function calling / schema の probe がある。
- Ollama XML fallback / tool-like output の probe がある。
- API key がない環境では skip される。
- prompt-sensitive fix の作業計画に provider probe 要否を明記できる。
```
