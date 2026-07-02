# UltraPlan Generation Source Parity Plan

作成日: 2026-06-29

## 0. 目的

`/ultra-plan-run` の品質を一度に広く直すのではなく、第一段階として **UltraPlan 生成だけ** を移植元 anvil の挙動に寄せて戻す。

対象は以下に限定する。

- UltraPlan 生成 prompt
- profile rules の注入
- invalid plan retry
- deterministic fallback の扱い
- planner failure / degraded plan の診断

対象外は以下とする。

- phase-aware verification の導入
- runtime recovery / repair handoff の導入
- TUI run event 常時保存
- capability acceptance oracle の強化
- Next.js / Space Invaders 専用テンプレート追加
- UltraPlan 保存形式の互換性を壊す schema 変更

この分割により、021 で広く入れすぎた変更の反省を踏まえ、小さく戻して評価する。

## 0.1 レビュー反映サマリ

本計画を以下の観点でレビューし、指摘を反映した。

- Source parity: 移植元の `system prompt / profile rules / retry / fail-fast / tool-call rejection` を UltraPlan 生成契約として明示する。
- Scope control: phase verification、runtime recovery、TUI run log、capability oracle は 021-1 に混ぜない。
- Compatibility: MVP の既存 UltraPlan YAML 保存形式を壊さない。`degraded: true` のような新 field を plan YAML に追加する案は採用しない。
- Safety: planner が invalid な場合は workspace mutation 前に止める。薄い deterministic fallback を通常成功として実行しない。
- Overfitting prevention: Next.js / Space Invaders 固有の hard lint は入れない。profile 固有の内容は prompt rules と retryable quality guidance に留める。
- Testability: fake planner による retry / fail-fast / no-workspace-mutation の検証を必須にする。
- Source normalization: 移植元同様、planner が返した `goal/profile/style/intent` は要求側の値で正規化してから validate / lint する。metadata の echo 揺れだけで good phase plan を過剰拒否しない。

主な修正点:

- UltraPlan 生成の出力形式は **MVP 既存の YAML shape を正** とする。移植元は JSON 生成だが、021-1 では parser 変更リスクを避け、source の「意味論」を YAML prompt に写す。
- deterministic fallback diagnostic を保存する場合でも、UltraPlan YAML に `degraded` field を増やさない。eval event または別 diagnostic artifact で区別する。
- Next.js の setup/build/implementation topology は hard lint ではなく、まず profile generation rules と retry prompt で誘導する。
- planner が tool call を返した場合も invalid output として retry/fail-fast する条件に追加する。
- `emit_planner_schema_repaired` のような「修復済み」に読める event を UltraPlan fallback/fail-fast に流用しない。retry / failed / degraded を明示する。

## 1. 現状

### 1.1 MVP の current behavior

MVP の UltraPlan 生成は `mvp/anvilminimal/src/planner/runner.rs::generate_ultra_plan_with_ui` にある。

現在の主な挙動:

- user message 1本だけで planner に UltraPlan YAML を要求する。
- system prompt を使っていない。
- profile-specific generation rules を UltraPlan 生成時に渡していない。
- parse / lint に失敗した場合、`UltraPlan::deterministic(...)` を返して実行を続ける。
- deterministic fallback plan が通常の成功 plan とほぼ同じ扱いで保存・実行される。

該当コード:

- `mvp/anvilminimal/src/planner/runner.rs:623`
- `mvp/anvilminimal/src/planner/runner.rs:632`
- `mvp/anvilminimal/src/planner/runner.rs:663`
- `mvp/anvilminimal/src/planner/runner.rs:689`
- `mvp/anvilminimal/src/planner/ultra_plan.rs:19`

### 1.2 実 UAT で確認された状態

対象ディレクトリ:

- `/Users/maenokota/share/work/localwork/commandagent_mvp/01/test0628_001`
- `/Users/maenokota/share/work/localwork/commandagent_mvp/01/test0628_002`

確認結果:

- 両方とも保存された UltraPlan が `scaffold / implement / verify` の薄い3 phase だった。
- この形は `UltraPlan::deterministic` と一致する。
- `test0628_001` は `src/app/page.tsx` が未作成で、ゲーム本体が存在しない。
- `test0628_002` は `src/app/page.tsx` はあるが、plan が要求した highscore / audio / particle / screen shake / power-up / mobile controls などは十分に実装されていない。

重要な点:

- 現象は「Next.js game 実装が下手」という単発問題ではない。
- planner が profile-aware な良い UltraPlan を生成できず、fallback plan で実行されている。
- fallback plan が通常成功 plan として流れているため、eval 上の診断や人間の期待とずれる。

### 1.3 移植元 anvil の behavior

移植元の UltraPlan 生成は `src/agent/minimal_step_runner.rs::generate_ultra_plan` にある。

移植元の主な挙動:

- system prompt と user prompt を分ける。
- planner は tool を使わない。
- JSON shape を明示する。
- profile generation rules を system prompt に入れる。
- style rules と work intent を入れる。
- invalid plan は補正リトライする。
- parse 後に `goal/profile/style/intent` を要求側の値で上書きする。
- retry 後も invalid なら `invalid generated ultra plan` として fail-fast する。
- deterministic fallback を planner success として扱わない。

該当コード:

- `src/agent/minimal_step_runner.rs:538`
- `src/agent/minimal_step_runner.rs:546`
- `src/agent/minimal_step_runner.rs:553`
- `src/agent/minimal_step_runner.rs:571`
- `src/agent/minimal_step_runner.rs:577`
- `src/agent/minimal_step_runner.rs:687`
- `src/agent/minimal_step_runner/profile.rs:19`
- `src/agent/minimal_step_runner/profiles/nextjs.rs:6`

注意:

- 移植元は generation 時に JSON を要求し、保存時に YAML へ render する。
- MVP は現時点で `parse_ultra_plan` / `render_ultra_plan` が YAML shape を前提にしている。
- 021-1 では JSON parser 移行は行わず、system prompt の出力 shape を MVP YAML に合わせる。ここで戻す対象は format ではなく、planner contract である。

## 2. あるべき挙動

UltraPlan 生成は、以下の契約を満たすべきである。

### 2.1 planner prompt contract

- system prompt で UltraPlan planner の役割を固定する。
- planner は tool call を出さない。
- 出力形式は MVP 既存の UltraPlan YAML shape に固定する。
  - `goal`
  - `profile`
  - `style`
  - `intent`
  - `phases[].id`
  - `phases[].prompt`
- phase は 2〜6 程度を基本とし、最大 8 を超えない。
- 各 phase prompt は shell command ではなく、`/plan-run` に渡せる自然言語タスクにする。
- 各 phase prompt は concrete outcome と verification expectation を含む。
- Required final artifacts があれば、phase 全体で維持する。
- profile-specific generation rules を必ず含める。
- parse 後は、planner 出力の metadata ではなく、呼び出し元の `goal/profile/style/intent` を canonical value として採用する。

### 2.2 profile-aware planning contract

Next.js profile では、少なくとも以下を UltraPlan 生成時に planner へ渡す。

- real Next.js app contract を保つ。
- `next`, `react`, `react-dom` dependencies を含める。
- `scripts.build = "next build"` を維持する。
- dependency setup と build verification を分離する。
- `node_modules` がない状態で build verify を先に置かない。
- Tailwind utility / `@tailwind` を使う場合は Tailwind dependency と config を含める。
- Tailwind toolchain を用意しないなら plain CSS に寄せる。
- dev script は port 要求があれば `3011` を含める。

これは `test0628_002` のように Tailwind class 風の記述だけが入り、Tailwind dependency/config が弱い状態を防ぐためにも必要である。

### 2.3 retry / fail-fast contract

- 1回目の planner output が invalid なら、エラー理由を含めて補正リトライする。
- planner が tool call を返した場合も invalid output として扱う。
- parse に成功した UltraPlan は、validate / lint 前に `goal/profile/style/intent` を request context で正規化する。
- retry 上限後も invalid なら、通常 plan として実行しない。
- fallback plan を作る場合でも `degraded_planner_fallback` として明示し、成功 path として扱わない。
- eval / TUI / CLI で `planner_schema_error`, `planner_lint_error`, `ultra_plan_generation_failed`, `ultra_plan_degraded_fallback` を区別できるようにする。

### 2.4 deterministic fallback contract

deterministic fallback は以下のどちらかに限定する。

1. **fail artifact**
   - 実行は止める。
   - degraded diagnostic を保存する場合も、実行対象ではなく診断成果物として扱う。
   - 既存 UltraPlan YAML shape には `degraded` field を追加しない。

2. **明示的な degraded mode**
   - ユーザーまたは eval が opt-in した場合のみ実行する。
   - `ok=false` または `degraded=true` を event に残す。
   - 成功率には通常成功として混ぜない。

今回の第一段階では、原則として 1 の fail-fast を採用する。

## 3. 問題点

### P1. UltraPlan 生成が profile-unaware

現状の MVP は UltraPlan 生成時に profile rules を渡していない。  
そのため、Next.js project としての setup/build 順序、Tailwind 整合性、port 要件、final artifact preservation が phase plan の段階で弱い。

影響:

- phase が `scaffold / implement / verify` の抽象語だけになりやすい。
- `page.tsx` などの重要成果物が phase plan 上で責任分界されにくい。
- 実行モデルが「何をどこまで完成させるか」を取り違えやすい。

### P2. invalid plan が成功経路へ落ちる

parse/lint failure 後に deterministic UltraPlan を返しているため、planner が失敗しても `/ultra-plan-run` は進む。

影響:

- 実際には planner failure なのに、runtime failure や成果物品質 failure として見える。
- eval の成功率・失敗分類が歪む。
- TUI ユーザーには「ちゃんと計画されて実行された」ように見える。

### P3. deterministic fallback が薄すぎる

fallback plan は `scaffold / implement / verify` だけで、profile-specific outcome や capability decomposition を持たない。

影響:

- `test0628_001` のように scaffold だけ残り、主要成果物が未作成になる。
- `test0628_002` のように build 可能な浅い game で止まりやすい。

### P4. 診断粒度が不足

fallback plan が保存されても、それが degraded fallback か通常 plan かが plan file 単体から分かりにくい。

影響:

- UAT 後に「なぜ薄い plan になったのか」を即座に追えない。
- eval 上も「計画品質が低い」のか「実行が悪い」のかを誤分類しやすい。

### P5. planner tool call を invalid plan として扱う契約が明示されていない

移植元は UltraPlan 生成で tool call が返った場合、invalid output として扱う。  
MVP では UltraPlan 生成時に `allowed_tools` は空だが、provider 実装や将来の planner 経路によって tool call が返る可能性を契約上閉じていない。

影響:

- planner 専用 turn で tool call が混ざった場合、parse failure / schema failure として扱うべきかが曖昧になる。
- eval event 上、provider/tool-call 由来の planner failure を区別しづらい。

### P6. generated metadata をどう扱うかが曖昧

移植元は planner output を parse した後、`goal/profile/style/intent` を呼び出し元の値で上書きしてから validate / lint する。  
MVP 側の計画では、metadata mismatch を hard lint として扱うようにも読める箇所があり、source parity とずれる。

影響:

- phase prompts は良いのに、profile や intent の echo が揺れただけで失敗する可能性がある。
- planner の形式揺れに対して、source より過剰に脆くなる。
- 本来 runtime に渡すべき canonical profile/style/intent が、LLM の出力に左右される。

## 4. 根本原因

根本原因は、UltraPlan 生成を「YAML を1本作らせる処理」として MVP 化し、移植元の **planner contract** を移植対象として明示しなかったことである。

具体的には以下。

- 移植元の `ultra_plan_generation_system_prompt` を MVP 側に対応付けなかった。
- `profile_generation_rules(profile, intent)` を UltraPlan 生成に接続しなかった。
- invalid output retry を「安定性のための補助」ではなく省略可能な実装詳細と見なした。
- deterministic fallback を runtime recovery scaffold と planner fallback plan で同一視した。
- tool-free planner turn という移植元の契約を明文化しなかった。
- generated metadata を request context で正規化する移植元の契約を明文化しなかった。
- 受入条件が「UltraPlan file が保存される」「phase が走る」に寄り、「source 同等の planner contract である」ことを見ていなかった。

結果として、指標上は phase 数や YAML shape が成立しても、実際の成果物品質を支える計画の責任分界が弱くなった。

## 5. 対策案

### C1. UltraPlan generator prompt を source parity に戻す

MVP に以下の helper を追加する。

- `ultra_plan_generation_system_prompt(profile, style, intent)`
- `ultra_plan_generation_user_prompt(goal, profile, style, intent)`
- `profile_generation_rules(profile, intent)`

移植方針:

- 移植元の文面をベースにする。
- MVP の YAML UltraPlan 形式に合わせ、shape だけ MVP 形式に調整する。
- ただし semantics は source と揃える。
- JSON 形式への移行は 021-1 の対象外とする。

受入条件:

- system prompt に planner role、output shape、phase rules、profile rules が含まれる。
- Next.js profile の generation rules が UltraPlan 生成時に含まれる。
- unit test で prompt 内容を確認する。

### C2. invalid output retry を入れる

`ULTRA_PLAN_GENERATION_ATTEMPTS = 3` 相当の bounded retry を入れる。

retry prompt には以下を含める。

- 前回の parse/lint error
- corrected YAML only
- phase prompt は shell command ではなく natural language task
- phase 数上限
- Required final artifacts preservation
- tool calls are not allowed

受入条件:

- 1回目 invalid、2回目 valid の fake planner test が通る。
- 3回 invalid の fake planner test で fail-fast する。
- tool call を返す fake planner が retry 対象になる。
- retry event が eval events に残る。

### C2.1 generated metadata を正規化する

parse に成功した UltraPlan に対して、移植元同様に以下を request context から上書きする。

- `goal`
- `profile`
- `style`
- `intent`

方針:

- phase id / phase prompt は planner output を使う。
- metadata は planner output を信用せず canonical request value にする。
- metadata mismatch は hard failure にしない。
- 正規化したことは必要に応じて debug / eval event に残すが、通常 failure にはしない。

受入条件:

- planner が `profile: generic` を返しても、呼び出しが `--profile nextjs` なら保存 plan は `profile: nextjs` になる。
- planner が `intent` を省略または誤る場合でも、`detect_intent(goal)` 由来の canonical intent になる。
- metadata の echo 揺れだけでは retry / failure にしない。

### C3. deterministic fallback を通常成功扱いしない

parse/lint failure 後に `UltraPlan::deterministic` を返す挙動を廃止する。

第一段階では以下にする。

- retry 後も invalid なら `Err(invalid generated ultra plan: ...)` を返す。
- `.anvil/plans` に degraded diagnostic plan を保存する場合は optional とし、実行しない。
- degraded diagnostic を保存する場合は、既存 UltraPlan YAML と別 artifact または eval event として扱う。
- eval classification は `planner_schema_error` または `planner_lint_error` として明示する。

受入条件:

- invalid planner output で `run_ultra_plan` が実行されない。
- degraded fallback が保存される場合でも、通常の `ultra-plan-*.yaml` と混同されない。
- 成功率集計で degraded fallback を success に混ぜない。

### C4. UltraPlan lint を source parity 観点で強化する

まず hard lint は移植元の validate 相当の汎用条件に限定する。

- phase id が non-empty kebab-like string。
- phase prompt が non-empty。
- phase prompt が REPL command / shell command だけでない。
- phase count が 2〜8。
- duplicate phase id がない。
- canonical metadata 正規化後の `goal/profile/style/intent` が空でない。

Next.js profile では、以下を **hard lint ではなく retryable quality guidance** として扱う。

- create intent なのに setup/build/implementation/final verification の責任分界がない。
- build verification phase がない。
- dependency setup と build verify が同一 phase に混ざりすぎている。

ただし、Space Invaders 専用の capability は lint に入れない。  
capability acceptance は別レイヤで扱う。

この切り分けにより、021-1 では planner の一般契約を戻し、profile-specific execution 品質は prompt/retry で誘導する。hard lint を増やしすぎて正当な plan を落とすリスクを避ける。

### C5. 診断 event を追加する

最低限、eval events に以下を出す。

- `ultra_plan_generation_attempt`
- `ultra_plan_generation_retry`
- `ultra_plan_generation_failed`
- `ultra_plan_generation_succeeded`
- `ultra_plan_generation_degraded_fallback_created` がある場合は `ok=false`
- `ultra_plan_generation_tool_call_rejected`
- `ultra_plan_generation_metadata_normalized`

通常 TUI の常時 run log は今回の範囲外だが、eval event はこの段階で入れる。

event の必須 field:

- `attempt`
- `provider`
- `model`
- `profile`
- `style`
- `intent`
- `degraded`

event の optional field:

- `failure_kind`
- `failure_message`
- `normalized_fields`
- `raw_profile`
- `raw_style`
- `raw_intent`

### C6. rollback-safe に実装する

広範囲変更を避けるため、以下の順序で実装する。

1. prompt helper と tests を追加する。
2. retry logic と fake planner tests を追加する。
3. generated metadata normalization を追加する。
4. deterministic fallback の success return を fail-fast に変える。
5. eval classification を最小追加する。
6. mvp smoke / ultra-plan-run targeted eval で比較する。

Rollback-safe のため、021-1 では以下を行わない。

- `UltraPlan` struct への field 追加
- `.anvil/runs` 常時ログの新設
- phase execution prompt の変更
- profile verification の変更
- runtime repair handoff の変更

## 6. 対策後の期待する挙動

### 6.1 正常系

`/ultra-plan-run --profile nextjs ...` で planner が valid UltraPlan を返した場合:

- 保存される UltraPlan は `scaffold / implement / verify` の薄い fallback ではない。
- phase prompt に Next.js profile の要求が反映される。
- setup, implementation, verification の責任分界が明確になる。
- `src/app/page.tsx` など required artifacts の扱いが phase 全体で維持される。
- 後続の plan-run / minimal-loop が、より具体的な phase goal を受け取る。

### 6.2 planner 一時失敗

1回目の output が invalid の場合:

- エラー理由付きで retry する。
- retry で valid になればその plan を実行する。
- event に retry 回数が残る。
- valid plan の metadata は request context で正規化される。

### 6.3 planner 継続失敗

全 attempt が invalid の場合:

- deterministic fallback を通常成功 plan として実行しない。
- TUI / CLI には UltraPlan generation failure として出す。
- eval では planner failure として分類される。
- phase execution に入らないため、成果物が中途半端に作られない。

### 6.4 品質面の期待

この対策だけで、すべての `ultra-plan-run` が高品質成果物を作るわけではない。  
ただし、以下は改善する。

- fallback plan による false success が減る。
- plan-run / runtime の問題と planner の問題を分離できる。
- 良い phase plan から実行を開始する割合が上がる。
- `test0628_001` のような「薄い fallback plan で scaffold だけ残る」事象は減る。

## 7. 検証方法

### 7.1 Unit tests

追加するテスト:

1. `ultra_plan_prompt_includes_source_parity_rules`
   - system prompt に role, output shape, phase rules が含まれる。

2. `ultra_plan_prompt_includes_nextjs_profile_rules`
   - nextjs/create で dependency setup / build separation / Tailwind consistency / `next build` が含まれる。

3. `ultra_plan_generation_retries_invalid_output`
   - fake planner が invalid -> valid を返す。
   - retry 後に valid plan が採用される。

4. `ultra_plan_generation_fails_after_retry_exhaustion`
   - fake planner が invalid を返し続ける。
   - deterministic fallback が実行対象として返らない。

5. `invalid_ultra_plan_does_not_create_successful_run`
   - `run_ultra_plan_with_ui` が呼ばれないことを fake client / event で確認する。

6. `degraded_fallback_is_not_counted_as_success`
   - degraded fallback diagnostic を保存する場合でも success event を出さない。

7. `ultra_plan_generation_rejects_tool_calls`
   - fake planner が tool call を返す。
   - retry 対象になり、retry exhaustion 後は fail-fast する。

8. `ultra_plan_generation_uses_yaml_shape`
   - system prompt が MVP の YAML shape を要求している。
   - JSON-only prompt へ戻っていない。

9. `ultra_plan_generation_normalizes_metadata`
   - planner が mismatched `profile/style/intent` を返しても、canonical request values に正規化される。
   - metadata mismatch だけでは retry / failure にならない。

### 7.2 Integration tests

追加するテスト:

1. `ultra_plan_run_valid_plan_executes_phases`
   - fake planner が source-parity 形の valid UltraPlan を返す。
   - phase が順に実行される。

2. `ultra_plan_run_invalid_planner_output_fails_before_workspace_mutation`
   - invalid output のみを返す planner。
   - workspace に `package.json` や `src/app` が作られない。

3. `ultra_plan_run_retry_then_valid_preserves_profile`
   - retry で valid plan になった時、profile/style/intent が config / request context と一致する。

4. `ultra_plan_run_invalid_planner_output_does_not_save_executable_plan`
   - retry exhaustion 後、通常の executable `ultra-plan-*.yaml` が保存されない。
   - diagnostic を保存する場合は executable plan と区別できる。

### 7.3 Eval

対象を狭めて再計測する。

1. MVP only, cloud only, local LLM unused。
2. 対象 mode:
   - `ultra-plan-run`
   - 必要に応じて `step-plan` を参考値として実行
3. 比較対象:
   - rollback 後 baseline
   - 021-1 実装後

見る指標:

- `ultra_plan_generation_success_rate`
- `ultra_plan_generation_retry_count`
- `planner_schema_error`
- `planner_lint_error`
- `planner_tool_call_error`
- `ultra_plan_generation_metadata_normalized_count`
- `ultra_plan_degraded_fallback_count`
- `ultra_plan_fallback_success_count` は 0 であること
- `phase_completion_score`
- `acceptance_success`
- `plan_output_adherence_score`

期待:

- planner failure は一時的に増えてよい。
- 成功率だけを見ると下がる可能性がある。
- ただし degraded fallback による false success は減る。
- valid UltraPlan から開始した run の accepted artifact quality が改善する。

### 7.4 Manual UAT

同等コマンドで確認する。

```text
anvilminimal --yes --context-budget 65536 --model qwen3.6:27b-coding-nvfp4 --planner-model gemini-3.5-flash --planner-provider gemini --provider ollama
/ultra-plan-run --profile nextjs あなたが考える最高に面白くかっこいいスペースインベーダーゲームを3011ポートで起動可能なnext.jsアプリとして開発してください。
```

確認項目:

- 保存された UltraPlan が deterministic fallback 形だけではない。
- fallback になった場合は通常実行されず、明示的に failure として表示される。
- `src/app/page.tsx` がない状態で成功扱いされない。
- 成功扱いの場合、`src/app/page.tsx` と root route が存在する。
- invalid planner output の場合、workspace が空のまま維持される。

## 8. リスクと対策

### R1. 成功率が一時的に下がる

fail-fast により、今まで fallback で進んでいたケースが planner failure として止まる。

対策:

- これは正しい失敗検知として扱う。
- 成功率だけでなく false success reduction を見る。
- valid UltraPlan subset の成果物品質を別集計する。

### R2. Gemini planner の YAML/JSON 揺れで失敗が増える

Gemini は形式揺れが起きやすい。

対策:

- retry prompt を具体化する。
- parser は既存の YAML parser を基本とし、形式修復は bounded に留める。
- JSON 形式への移行はこの phase に混ぜない。
- Gemini tool calling 対応は別計画で扱い、今回の scope に混ぜない。

### R3. prompt が大きくなりすぎる

profile rules を入れることで prompt が長くなる。

対策:

- UltraPlan 生成専用の compact rules にする。
- phase 実行 prompt とは重複しすぎないようにする。
- ただし Next.js dependency/setup/build contract は削らない。

### R4. deterministic fallback 廃止で UX が悪化する

今までは何かしら動いていたが、今後は planner failure で止まる。

対策:

- CLI/TUI message に「planner output invalid, no workspace changes were made」を明示する。
- recovery handoff は次フェーズで扱う。
- この段階では「中途半端な成果物を作らない」ことを優先する。

### R5. hard lint 強化による過剰拒否

UltraPlan lint を profile-specific に強くしすぎると、実行可能な plan まで拒否する可能性がある。

対策:

- 021-1 の hard lint は source validate 相当の汎用条件に限定する。
- Next.js topology は prompt rules と retryable quality guidance に留める。
- hard failure と quality retry を eval event で分ける。

## 9. 完了条件

Phase 021-1 の完了条件:

- MVP の UltraPlan 生成が system prompt + user prompt + profile rules を使う。
- UltraPlan 生成 prompt が MVP 既存 YAML shape を要求する。
- invalid output retry が実装されている。
- planner tool call が invalid output として retry/fail-fast される。
- generated metadata が request context で正規化される。
- retry exhaustion で deterministic fallback を通常成功 plan として返さない。
- fallback / degraded plan が success metric に混ざらない。
- invalid planner output では phase execution に入らず、workspace mutation が発生しない。
- unit tests / integration tests が追加され、通る。
- MVP only eval で `ultra_plan_generation_retry_count`, `ultra_plan_degraded_fallback_count`, planner failure 分類を確認できる。
- `test0628_001` 型の「fallback plan で scaffold だけ作成して成功扱い」は再現しない。

## 10. 次フェーズへの引き継ぎ

この計画で直すのは UltraPlan 生成だけである。

残る課題:

- phase-aware verification
- phase 間 context continuity
- runtime recovery / repair handoff
- TUI run log
- capability acceptance oracle

021-1 実装後の評価で、planner failure と runtime failure を分離できるようにし、次にどこを戻すべきかを判断する。
