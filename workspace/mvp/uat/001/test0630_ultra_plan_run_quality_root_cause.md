# test0630_001 Ultra Plan Run Quality Root Cause Analysis

作成日: 2026-06-30

## 対象

UAT workspace:

`/Users/maenokota/share/work/localwork/commandagent_mvp/01/test0630_001`

実行条件:

```bash
anvilminimal --yes --context-budget 65536 \
  --model gemini-3.1-flash-lite \
  --planner-model gemini-3.5-flash \
  --planner-provider gemini \
  --provider gemini
```

TUI command:

```text
/ultra-plan-run --profile nextjs あなたが考える最高に面白くかっこいいスペースインベーダーゲームを3011ポートで起動可能なnext.jsアプリとして開発してください。
```

主な観測証跡:

- Plan: `/Users/maenokota/share/work/localwork/commandagent_mvp/01/test0630_001/.anvil/plans/ultra-plan-019f1754-2d9c-71d1-8475-afd83b610f28.yaml`
- Events: `/Users/maenokota/share/work/localwork/commandagent_mvp/01/test0630_001/.anvil/runs/019f1753-ff27-76c3-bcf2-c1ecbf62851f/events.jsonl`
- Summary: `/Users/maenokota/share/work/localwork/commandagent_mvp/01/test0630_001/.anvil/runs/019f1753-ff27-76c3-bcf2-c1ecbf62851f/summary.md`
- Main implementation: `/Users/maenokota/share/work/localwork/commandagent_mvp/01/test0630_001/src/app/page.tsx`

## 結論

今回の成果物は、scaffold-only からは前進している。`Canvas`、プレイヤー移動、弾、敵、衝突、score、combo、particles、screen shake までは生成されている。

ただし、これは完成品ではない。`ultra-plan-run` は 5 phase 中 3 phase で停止し、phase 4 `web-audio-synth-and-ui` と phase 5 `build-and-verify` が未実行である。さらに completed phase の `npm run build` verify と手動 build は通るが、実ブラウザの dev server では `/` が `HTTP 500` になり、canvas が表示されない。

根本的には、MVP runtime がまだ「実装された capability を最後まで運び、ブラウザで動く成果物として acceptance する」よりも、「expected path が作られた」「npm run build が通った」に寄っている。

## 妥当性レビュー結果

本分析の大筋は、UAT workspace の `.anvil` events、保存 plan、生成物、現行 MVP コードと整合している。

ただし、以下は誤読を避けるために補正する。

- 今回の停止は「valid StepPlan 実行後の verify failure」ではなく、phase 4 の **StepPlan 生成時に verify command policy violation が corrective retry 後も解消できなかった planning/scaffold failure** である。
- `.anvil/repairs/repair-phase-web-audio-synth-and-ui-*.md` は保存されているが、手動リカバリ用の `.anvil/plans/recovery-ultra-plan-*.yaml` は保存されていない。
- `npm run build` が通ったことは、completed phase の build verify および手動確認としては事実だが、final phase は未実行であり、browser readiness は別の acceptance evidence として扱う必要がある。
- eval/release gate 側の問題は「evidence path の有無」と「evidence JSON の `ok=false` / HTTP 500 の内容検証」を分けて扱うべきである。

## 追加レビュー結果

観点別レビューの結果、以下を本分析の前提に追加する。

- **設計思想**: 対策は MVP の小さい API 境界に留めるべきであり、source の TaskContract / RepairJob / browser runner 全体を丸ごと移植する方向にはしない。
- **不安定化リスク**: browser readiness を通常 unit test や全 TUI 実行の必須処理にすると環境依存で不安定になる。実装は availability-aware にし、browser unavailable は `partial`、browser HTTP 500 は `fail` と分ける必要がある。
- **影響範囲**: 影響は `ultra-plan-run` だけでなく、`step-plan` の verify policy、`plan-run` の step completion、`minimal-loop` の completion contract、eval/release gate に横展開する。
- **ソフトウェア複雑性**: 新しい大規模 controller を追加するのではなく、既存の `VerificationReport`、`RuntimeAcceptanceReport`、`RepairContext`、`UltraPlan` 保存処理の拡張で済ませる。
- **過適応回避**: `Space Invaders` 固有語ではなく、interactive app/game の一般 capability として扱う。本文中の enemy movement / lives / high score などは今回成果物の不足例であり、固定 rule ではない。
- **不確実性**: 今回の失敗は events と生成物で再現可能な deterministic failure であり、現段階では LLM API 追加実行による仮説検証は不要である。provider prompt 変更を行う段階では、別途 provider probe で確認する。

## 1. ultra-plan-run が完走していない

### 現象

UltraPlan は 5 phase:

1. `init-project-and-tailwind`
2. `core-game-loop`
3. `cool-effects-and-juice`
4. `web-audio-synth-and-ui`
5. `build-and-verify`

実際には phase 3 まで完了し、phase 4 の StepPlan scaffold で停止した。

停止理由:

```text
phase scaffold failed: invalid StepPlan after corrective retries:
verify command may not use shell control syntax
```

これは phase の verify step 実行後に落ちたのではなく、phase 4 用 StepPlan を生成・lint する段階で verify command policy に落ちたものである。

### 問題箇所

生成 plan:

- `.anvil/plans/ultra-plan-019f1754-...yaml`
  - phase 4 で Web Audio / HUD / high score / overlays を要求
  - phase 5 で final build verify を要求

MVP runtime:

- `mvp/anvilminimal/src/planner/verify.rs:266`
  - `validate_verify_command`
- `mvp/anvilminimal/src/planner/verify.rs:292`
  - `diagnose_verify_command`
- `mvp/anvilminimal/src/planner/verify.rs:300`
  - shell control syntax を verify command violation として拒否
- `mvp/anvilminimal/src/planner/runner.rs:202`
  - corrective retry 後も StepPlan invalid なら fail
- `mvp/anvilminimal/src/planner/runner.rs:1464` 付近
  - `ultra_phase_failed` を出して phase を停止

### 直接原因

Gemini planner が phase 4 用 StepPlan で `&&` などの shell control syntax を含む verify command を生成し、補正 retry 後も policy-compliant な単一 verify command に直せなかった。

### 根本原因

1. **planner repair が安全な verify command へ決定論的に正規化できていない**
   - shell control syntax を拒否する方針自体は妥当。
   - しかし、`npm run build && ...` のような荒い verify を「複数の safe verify に分割する」「許可済み代表 verify に縮退する」などの deterministic repair がない。
   - そのため provider が同じ誤りを繰り返すと phase 全体が停止する。

2. **phase scaffold failure がそのまま ultra-run 全体の中断になる**
   - recovery prompt は保存されるが、自動的に次の安全な recovery phase へは進まない。
   - source parity の handoff としては前進しているが、UAT の期待である「最後まで成果物を作る」には足りない。
   - さらに、現状は `.md` の recovery prompt だけで、確認・編集して再実行できる recovery UltraPlan YAML は保存されない。

3. **TUI が途中成果物を明確に「未完了」と扱いきれていない**
   - summary には failure が出るが、workspace にはそれっぽい成果物が残る。
   - ユーザー視点では「完成したが品質が低い」のか「途中で止まった」のか判別しづらい。

## 2. 実ブラウザでは動いていない

### 現象

`npm run dev -p 3011` 後、Playwright で `http://127.0.0.1:3011/` を確認したところ:

- HTTP status: `500`
- canvas: 未表示
- error: `./src/app/globals.css Module parse failed: Unexpected character '@' (1:0)`

該当 CSS:

- `/Users/maenokota/share/work/localwork/commandagent_mvp/01/test0630_001/src/app/globals.css:1`

```css
@tailwind base;
@tailwind components;
@tailwind utilities;
```

### 問題箇所

生成物:

- `src/app/globals.css`
- `postcss.config.js`
- `tailwind.config.js`
- `package.json`
- `src/app/layout.tsx`

MVP runtime / eval:

- `mvp/anvilminimal/src/planner/runner.rs:2373` 付近
  - final acceptance 時の Next.js browser requirement の扱い
- `mvp/anvilminimal/scripts/eval_lib/browser_oracle.py:9`
  - browser oracle は adapter hook だが通常は `not_enabled`
- `mvp/anvilminimal/scripts/eval_lib/parity_gate.py:262`
  - release evidence は required evidence path の有無を主に見ており、JSON 内の `ok=false` や HTTP 500 を release gate fail に落とし切れていない

### 直接原因

Next.js dev server の CSS pipeline が `@tailwind` directive を処理できず、`globals.css` を parse error にしている。

completed phase の `npm run build` verify と手動の build 確認は通っているため、build-only verify ではこの問題を検出できなかった。

### 根本原因

1. **deterministic verify が build 偏重**
   - phase verify は `npm run build` を主に見ている。
   - dev server readiness、route `/` の HTTP status、browser rendering、canvas existence が通常実行の acceptance に十分接続されていない。

2. **Tailwind toolchain の整合性検証が runtime path まで届いていない**
   - package/config/CSS が存在することは確認できても、dev server で CSS loader が正しく動くことは確認できていない。
   - `build passes but dev route fails` という Next.js 特有の失敗形を profile verifier が捕まえられていない。
   - ただし、原因を単一の Tailwind version へ固定しない。package/config/CSS/dev server error を evidence として集め、version-aware な profile contract で扱う。

3. **release evidence の content validation が未完成**
   - `parity_gate.py` は evidence path の有無を見て `pass/partial` を判定する。
   - browser evidence JSON の `ok=false` や HTTP 500 を release gate の機械的 fail にするロジックが不足している。
   - これは通常実行の failure そのものではなく、release-grade 評価で false positive を残し得る評価側の問題である。

## 3. ゲームロジックが浅い

### 現象

`src/app/page.tsx` はゲームらしいコードを含むが、実ゲームとしては浅い。

存在する要素:

- canvas
- keyboard event
- score
- combo
- particles
- screen shake
- player bullet
- collision

不足する要素:

- 敵の左右移動 / 下降
- 敵の攻撃
- lives
- win condition
- lose condition
- game over 遷移
- start / restart / pause flow
- 実体のある power-up
- high score
- Web Audio

### 問題箇所

生成物:

- `src/app/page.tsx:19`
  - `player` が `useEffect` 内ローカル変数
- `src/app/page.tsx:20`
  - `bullets` が `useEffect` 内ローカル変数
- `src/app/page.tsx:21`
  - `invaders` が `useEffect` 内ローカル変数
- `src/app/page.tsx:53`
  - `gameLoop`
- `src/app/page.tsx:80`
  - invader は描画のみ
- `src/app/page.tsx:94`
  - collision は player bullet vs invader のみ
- `src/app/page.tsx:127`
  - `useEffect` dependency に `combo` が入っている

MVP runtime:

- `mvp/anvilminimal/src/minimal_loop/loop_run.rs:796`
  - required path が満たされたら completion 判定に進む
- `mvp/anvilminimal/src/minimal_loop/loop_run.rs:873`
  - completion contract が有効でない場合、`required_artifacts_satisfied_after_tool` で step を終了

該当 events:

```json
{"event":"loop_stop","reason":"required_artifacts_satisfied_after_tool","required_paths":["src/app/page.tsx","src/app/layout.tsx","src/app/global.d.ts"]}
```

```json
{"event":"step_obligation_scope","completion_contract_verification_enabled":false}
```

### 直接原因

実装 step が `src/app/page.tsx` を 1 回 Write した時点で、expected path が満たされたとして終了している。ゲームとして必要な capability を満たしたかは、この step の停止条件にはなっていない。

### 根本原因

1. **expected artifact と capability evidence の結合が弱い**
   - `page.tsx` が存在することと、playable game loop が存在することは別。
   - 現状の step completion は「ファイルができた」ことを強く見ており、「敵が動く」「敵が撃つ」「ライフが減る」「勝敗が決まる」を controller が検証していない。

2. **phase-specific capability が concrete evidence に落ちていない**
   - phase 2 は core game loop、phase 3 は effects/power-ups を要求している。
   - しかし events の `required_final_evidence` は空。
   - その結果、LLM がそれっぽいコードを書いた時点で phase が進む。

3. **React/canvas の状態管理品質を acceptance できていない**
   - `useEffect` 内ローカル変数に game state を置く設計は壊れやすい。
   - `combo` 変更で effect が作り直されるため、ゲーム状態が再初期化され得る。
   - この種の runtime design issue は build では検出されない。

## 4. 計画で要求した要素が実装されていない

### 現象

UltraPlan phase 4 は以下を要求している。

- Web Audio synthesizer sounds
- cyberpunk themed neon dashboard UI
- score / lives
- high scores with LocalStorage
- start / pause / game-over overlays

実装には以下がない。

- `AudioContext`
- `localStorage`
- lives state
- pause state
- start overlay
- restart flow
- high score persistence

### 問題箇所

生成 plan:

- `.anvil/plans/ultra-plan-019f1754-...yaml:12`
  - phase 4 `web-audio-synth-and-ui`

実行 events:

- `ultra_phase_failed`
  - phase 4 scaffold failure
- `recovery_prompt_saved`
  - phase 4 の recovery prompt は保存済み
  - ただし recovery UltraPlan YAML は未作成

生成物:

- `src/app/page.tsx`
  - phase 4 要素が未実装

### 直接原因

phase 4 の StepPlan 生成が verify command policy violation で失敗し、phase 4 自体が実行されていない。

### 根本原因

1. **phase が未実行でも workspace には途中成果物が残る**
   - phase 3 までの成果物が見えるため、ユーザーは完成品として評価しやすい。
   - 実際には plan 上の要求のうち後半が未実施。

2. **phase completion と final acceptance の間に UX gap がある**
   - TUI は失敗を出しているが、成果物側に `INCOMPLETE` marker や run status summary がない。
   - workspace を開くと失敗状態が見えにくい。

3. **recovery handoff はあるが、ユーザーの通常導線では継続実行されていない**
   - `.anvil/repairs/repair-phase-web-audio-synth-and-ui-...md` は作成済み。
   - suggested command も出ている。
   - ただし、これは完走ではなく「次に回復するための入口」である。
   - YAML plan が残らないため、ユーザーが recovery 内容を plan として確認・編集・再実行する導線は弱い。

## 5. UI/UX が最低限

### 現象

画面上の要素は以下に留まる。

- title: `NEON INVADERS`
- score
- combo
- canvas

不足:

- 操作説明
- start/restart button
- pause
- game over overlay
- win/lose feedback
- lives
- high score
- difficulty/wave progression
- sound feedback

### 問題箇所

生成物:

- `src/app/page.tsx:129`
  - JSX return
- `src/app/page.tsx:130`
  - main layout
- `src/app/page.tsx:132`
  - score/combo 表示のみ
- `src/app/page.tsx:136`
  - canvas のみ

### 直接原因

UI/HUD を実装する phase が未実行であり、phase 3 までの最小 UI のまま停止している。

### 根本原因

1. **UI/UX capability が final acceptance にしか十分現れない**
   - phase 3 までの build verify では UI/UX の不足を検出できない。
   - phase 4 に UI/HUD が寄せられており、そこが失敗すると品質が大きく落ちる。

2. **品質要求が acceptance evidence に分解されていない**
   - 「最高に面白くかっこいい」は定性的だが、最低限の evidence には分解できる。
   - 例: visible controls/help, start/restart, lives, enemy movement, player damage, win/lose, score persistence, sound toggle。
   - 現状はこれらが explicit evidence として controller に渡っていない。
   - ただし、上記は app/game の一般 evidence 例であり、特定ゲーム専用の hard-coded checklist にはしない。

## 6. 評価上の問題

### 現象

今回の workspace では completed phase の `npm run build` verify と手動 build は通っているが、browser readiness は失敗する。

また、phase 4 で停止しているにもかかわらず、生成物だけを見ると「それっぽいゲーム」が存在する。

### 問題箇所

MVP runtime / eval:

- `mvp/anvilminimal/src/minimal_loop/loop_run.rs:873`
  - completion contract 無効時に required paths satisfied で終了
- `mvp/anvilminimal/scripts/eval_lib/browser_oracle.py:9`
  - browser oracle は明示 adapter だが通常 smoke では未実行
- `mvp/anvilminimal/scripts/eval_lib/parity_gate.py:287`
  - release evidence path が揃うと `status=pass` になり得る
- `mvp/anvilminimal/scripts/eval_lib/parity_gate.py:297`
  - gap 判定は evidence path の有無のみ

### 直接原因

評価がまだ以下を十分に区別できていない。

- build success
- dev server route success
- browser render success
- interaction success
- playable core loop success
- full plan completion

### 根本原因

1. **build-only false positive がまだ残る**
   - `npm run build` が通っても dev server で 500 になるケースがある。
   - Next.js App Router / Tailwind / CSS loader 周辺では特に顕在化しやすい。

2. **release evidence の内容検証が弱い**
   - evidence file が存在することと、evidence 内の `ok=true` は別。
   - `parity_gate.py` は path presence gate であり、browser JSON の HTTP 500 を gate fail にしない。
   - そのため release evidence が添付されても、内容が失敗なら full pass にしない追加判定が必要である。

3. **partial artifact visibility 問題**
   - ultra-run が失敗しても、workspace には途中成果物が残る。
   - TUI の failure message と workspace の状態が分離しており、ユーザーが成果物だけ見ると品質問題として見える。
   - 実際には「未完了成果物 + low capability completion」の複合問題である。

## 横断的な根本原因

### RC-01: Step completion が path existence に寄りすぎている

`required_artifacts_satisfied_after_tool` は狭いタスクでは有効だが、interactive app/game では弱い。

今回のように `src/app/page.tsx` が存在するだけでは、playable core loop を満たしたとは言えない。

### RC-02: Capability obligation が runtime controller に十分接続されていない

events では `required_final_capabilities` は出ているが、`required_final_evidence` が空である。

つまり、controller は capability を知っていても、それを「何を確認すれば満たしたと言えるか」に変換できていない。

### RC-03: Phase failure recovery は保存されるが、完走 UX にはなっていない

recovery prompt と suggested command は出ている。これは前進。

ただし UAT 期待は「失敗時の次手が出る」だけでなく、「途中成果物を完成品と誤認しない」「必要なら継続実行できる」ことである。

### RC-04: Next.js release gate が build から browser readiness へ十分移っていない

Next.js interactive app では `npm run build` だけでは足りない。

最低限:

- `npm run dev -p 3011`
- `/` HTTP 200
- canvas or interactive DOM visible
- keyboard/pointer interaction changes state
- browser console/page error absent

が必要。

### RC-05: planner retry が provider 任せ

verify command policy violation に対して corrective retry はあるが、safe command への deterministic transformation がない。

LLM が同じ構造の違反を繰り返すと phase が停止する。

## 今回の問題の分類

| 問題 | 主分類 | 直接原因 | 根本原因 |
| --- | --- | --- | --- |
| phase 4 で停止 | planning / bridge | verify command policy violation | shell-control verify の deterministic repair 不足 |
| browser 500 | runtime / acceptance | Tailwind CSS parse error | browser readiness が通常 acceptance に不足 |
| game logic が浅い | runtime output quality | path 作成で step 完了 | capability evidence と completion の結合不足 |
| plan 要素未実装 | phase execution | phase 4 未実行 | phase failure 後の continuation UX 不足 |
| UI/UX 最低限 | output quality | UI phase 未実行 / evidence 不足 | quality contract の具体化不足 |
| eval 問題 | evaluation | build-only / path-only 判定 | release evidence content validation 不足 |

## 次に見るべき優先順

1. **browser readiness を通常実行の final acceptance に入れる**
   - Next.js interactive app では build-only を合格にしない。
   - dev server `/` 200 と browser render を見る。

2. **interactive game の capability evidence を controller に持たせる**
   - enemy movement
   - player control
   - shooting
   - collision
   - score/progression
   - damage/lives/failure state
   - start/restart/pause

3. **implement step の path-only completion を抑制する**
   - app/game profile では `required_artifacts_satisfied_after_tool` だけで止めない。
   - required capability evidence が空なら、phase task から最低限の evidence を導出する。

4. **verify command policy violation の deterministic repair を追加する**
   - shell control syntax を許可するのではなく、safe command へ分解または縮退する。
   - 例: `npm run build && test -f src/app/page.tsx` は verify list `["npm run build", "test -f src/app/page.tsx"]` に変換する。

5. **TUI に partial artifact 状態を明示する**
   - ultra-run failure 時に `workspace contains partial artifacts` を表示。
   - `.anvil/runs/<run-id>/summary.md` に completed/failed phase と next recovery command を強調。

6. **release gate report が evidence content を読む**
   - `browser-readiness.json.ok=false` なら release gate fail/partial。
   - `interaction-evidence.json.ok=false` なら release gate fail/partial。
   - path presence だけで pass にしない。

7. **verify/planner failure 時の手動リカバリ導線を強化する**
   - 次に直すべき点は、verify/planner failure 時に `.md` だけでなく `.anvil/plans/recovery-ultra-plan-*.yaml` を保存し、TUI に「未完了」「保存した recovery YAML」「推奨コマンド」を明示することである。
