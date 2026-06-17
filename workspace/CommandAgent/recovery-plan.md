# CommandAgent Contract Recovery Plan

作成日: 2026-06-17

## 目的

CommandAgent 移植時に削った、または未移植だった `/ultra-plan-run` 周辺の決定論的契約を復元する。

ここで復元する対象は legacy engine や巨大な repair 機構ではない。目的は、モデルに曖昧な判断を背負わせていた境界を Rust 側の型・lint・verifier・eval contract に戻し、CommandAgent の minimal-first 方針を保ったまま大規模タスクの失敗を観測・修正しやすくすることである。

## 現状診断

CommandAgent の大規模 6 ケース fresh eval は 0/6 だったが、失敗は「全く動いていない」ものではなく、主に契約不足に集中している。

- modify / investigate / docs などの作業意図が runtime に渡らず、modify タスクでも `intent: "new"` で計画される。
- eval case の `expected_artifacts` が `/ultra-plan-run` の required artifact contract に伝播していない。
- setup / verify / report / repair の責務境界が plan DSL ではなく prompt とモデル判断に寄っている。
- plan lint が workspace の既存ファイルを知らないため、false positive と false negative の両方を起こす。
- profile contract が小さい一方、intent 別の違いまで削られており、Next.js などで create と modify の契約が混ざる。
- verifier / repair は改善済みだが、invalid plan 保存、stop reason、failure evidence の標準化がまだ弱い。

## 原則

1. legacy engine は戻さない。
2. case memory、sidecar 推論、PAM、旧 repair job は戻さない。
3. 追加するのは「能力」ではなく「契約・型・観測性」に限定する。
4. profile は太らせない。まず common DSL で責務境界を定義し、profile は小さな事実契約だけを持つ。
5. すべての変更は eval script と docs の更新を同じ PR に含める。
6. 大規模タスクの成功率改善は副次効果として扱う。主目的は曖昧さの削減と triage 可能性の回復である。

## レビュー反映済みの制約

`workspace/CommandAgent/recovery-plan-review.md` の指摘を受け、以下を本計画の制約として明文化する。

- `intent` / `kind` / `expected_result` は executor 分岐を増やすためではなく、lint / prompt / reporting の責務境界を明確にするために使う。
- `kind` ごとの専用 executor は作らない。
- intent detector は補助に留める。明示 `--intent` を優先し、判定できない場合は `unknown` で進める。
- 既存 `.commandagent/plans/*.yaml` は読める状態を維持する。新 schema 欠落時は default 補完し、保存時に正規化する。
- `expected_artifacts` は eval 専用最適化ではなく、ユーザーが要求した成果物契約として扱う。通常利用で artifacts 未指定の場合の挙動は変えない。
- workspace-aware lint は shallow existence check に限定し、ファイル内容の読み取りや glob 展開を行わない。
- profile は framework 固有の事実契約だけを持つ。provider/model 固有の失敗対策は profile に入れない。
- repair 回数は増やさない。追加するのは証拠の保存・圧縮・再投入可能な packet 化であり、自動修復能力の強化ではない。

## リカバリ範囲

### 復元する

- common DSL の `intent`
- common DSL の `step kind`
- common DSL の `expected result`
- eval `expected_artifacts` から runtime required artifacts への伝播
- workspace-aware plan lint
- intent-aware profile contract
- verifier / repair evidence の標準化
- stop reason と invalid plan 保存
- 既存 plan YAML の後方互換
- CommandAgent と Anvil の差分台帳

### 復元しない

- legacy engine
- Anvil の巨大 loop control
- sidecar-based semantic summarization
- case memory / anti-pattern corpus
- 自動で深追いする multi-stage repair
- provider ごとの場当たり profile rule
- `kind` ごとの専用 executor
- provider/model 固有の prompt 分岐
- eval 成功率だけを目的にした hidden rule

## 作業単位

### CA-R0: Evidence Freeze

目的:

- 現在の失敗 root と診断を固定し、以降の変更が何を直したのか追跡できるようにする。
- 今回復元する契約と、復元しない legacy 機構の境界を ADR として固定する。

作業:

- `docs/eval/triage/large-root-20260617T003924.md` を recovery の基準 root として明記する。
- `docs/architecture.md` に「contract recovery は mechanism admission ではなく deterministic contract restoration」と追記する。
- `docs/eval/mvp-eval-report.md` に recovery 前 baseline として 0/6 root を参照する。
- `docs/adr/0002-contract-recovery.md` を作成し、戻す契約 / 戻さない機構 / Anvil との差分を明記する。

完了条件:

- recovery 前 baseline root が docs から追跡できる。
- ADR で「契約復元」と「legacy 回帰」の境界が説明されている。
- この段階ではコード変更しない。

### CA-R1: Common DSL Types

目的:

- plan の責務境界を prompt から YAML schema / Rust type へ戻す。

作業:

- step plan schema に以下を追加する。
  - `intent`: `new | modify | investigate | document | data | unknown`
  - `kind`: `inspect | create | edit | setup | verify | repair | report`
  - `expected_result`: `pass | fail | unavailable`
- 既存 plan との後方互換のため、欠落時は safe default を使う。
- 既存 plan を読み込んだ場合は、内部表現で default 補完し、再保存時に新 schema へ正規化する。
- plan generation prompt に「setup と verify を混ぜない」「verify は状態変更しない」「report は block を明示する」を schema として明記する。
- `kind` は lint / prompt / reporting に限定して使う。`kind` ごとの専用 executor は作らない。

完了条件:

- YAML parse/render unit test。
- old plan fixture が読める。
- old plan fixture を保存し直すと新 schema に正規化される。
- invalid enum が lint で拒否される。
- `kind` が loop 分岐を増やしていないことを code review checklist に含める。
- docs/usage.md に schema の意味が説明されている。

### CA-R2: Intent Propagation

目的:

- eval / CLI / slash command の intent が ultra plan と step plan へ伝わるようにする。

作業:

- `/ultra-plan-run` に `--intent <intent>` を追加する。
- intent 未指定時は軽量な deterministic detector を使う。ただし detector は補助であり、明示指定を優先する。
- detector は少数の明白な語彙だけを見る。分類不能な場合は `unknown` とし、実行を止めない。
- eval case YAML の `intent` を `scripts/eval_agent_slice.sh` から `/ultra-plan-run --intent` に渡す。
- generated ultra plan の missing intent を一律 `"new"` にしない。

完了条件:

- modify fixture で generated ultra plan が `intent: modify` になる。
- eval dry-run summary に intent が記録される。
- unknown intent fixture が generic contract で継続する。
- docs/usage.md と docs/evaluation.md に指定方法を記載する。

### CA-R3: Required Artifact Contract Propagation

目的:

- eval の success_check と runtime の artifact contract を分離させず、同じ成果物要求を phase / step に渡す。

作業:

- eval case の `expected_artifacts` を `/ultra-plan-run` prompt 末尾ではなく structured contract として渡す。
- CLI でも同じ概念を扱えるように、`--artifact <path>` または plan file contract として設計する。
- ultra phase prompt に required final artifacts を不変 contract として含める。
- step plan 生成時に required artifact を phase 内の expected paths へ落とす。ただし検証専用 step では既存 artifact の確認として扱う。
- 実行後の final artifact check と eval success_check の差分を meta に記録する。

完了条件:

- required artifact が phase prompt / saved plan / meta.json に残る。
- Next.js modify fixture で要求ファイルの path drift が検出できる。
- expected_artifacts なしの通常利用は挙動が変わらない。
- docs では eval 専用機能ではなく、ユーザー要求成果物の contract として説明されている。

### CA-R4: Workspace-Aware Plan Lint

目的:

- lint が実際の workspace 状態を知らないために起きる false positive を減らす。

作業:

- `lint_plan_with_workspace(plan, cwd)` を追加する。
- shallow existence check で existing files を見て、既に存在する `package.json` / `app/page.tsx` / `Cargo.toml` などを introduced path と同等に扱う。
- lint のためにファイル内容は読まない。glob 展開もしない。
- expected_paths の glob / 代替パス / JSON property path を file path grammar で拒否する。
- verification-only step は expected paths を「作成要求」ではなく「確認対象」として扱う。
- invalid generated plan は `.commandagent/plans/invalid-*.yaml` に保存する。

完了条件:

- workspace existing file fixture。
- expected_paths に `app/layout.tsx or app/layout.ts`、`app/routes/*.py`、`scripts.build` が混ざる fixture。
- invalid plan 保存の regression test。
- path confinement regression test。

### CA-R5: Thin Intent-Aware Profiles

目的:

- profile を太らせず、common DSL で表現できない framework 固有の事実だけを intent ごとに提示する。

作業:

- profile API を `profile_contract(profile, intent)` にする。
- `generic` は最小契約を維持する。
- `nextjs` は以下だけを intent-aware にする。
  - new: dependencies / build script / app entry / setup before verify
  - modify: existing structure preservation / no fake build success
  - investigate: inspect first / report evidence / avoid destructive changes
- `python` / `rust` / `docs` / `data-*` は、まず共通 DSL に寄せ、必要な薄い contract だけ追加する。
- provider/model 固有の失敗対策は profile に入れない。

完了条件:

- profile snapshot test。
- Next.js new / modify / investigate の contract 差分が docs に説明されている。
- profile rule が provider/model 固有の失敗に依存していない。
- snapshot diff で profile contract の増分が確認できる。

### CA-R6: Verifier And Repair Observability

目的:

- verifier / repair の証拠を標準化し、repair が失敗したときにユーザーと次の plan が同じ証拠を見られるようにする。

作業:

- stop reason を標準 enum として保存する。
  - `completed`
  - `max_iterations`
  - `max_iterations_verified`
  - `dependency_missing`
  - `verification_failed`
  - `verification_failed_after_repair`
  - `turn_error`
- verifier failure packet を保存する。
  - command
  - exit code
  - key diagnostic lines
  - source excerpt
  - missing artifacts
  - changed files
- saved repair prompt は short packet を標準にする。
- suggested command は file reference 形式にする。
- repair 回数・自動再計画回数は増やさない。

完了条件:

- verifier failure fixture で error payload が落ちない。
- saved repair prompt が `/ultra-plan-run --profile ... "$(cat ...)"` で goal length 制限にかからない。
- repair は bounded のまま。回数は増やさない。
- sidecar semantic summary は導入しない。

### CA-R7: Eval Regression Matrix

目的:

- recovery が契約復元として効いたか、large 6 だけでなく smoke と provider split でも確認する。

作業:

- offline smoke。
- unit tests。
- `scripts/eval_agent_slice.sh --dry-run`。
- large 6 cases runs=1。
- 可能なら release-quality として large 6 cases runs=3。

評価観点:

- 0/6 からの改善値だけを見ない。
- false positive plan lint が減ったか。
- expected artifact path drift が減ったか。
- dependency_missing と real build failure が分離されたか。
- max_iterations の stop reason が明確になったか。
- invalid generated plan が保存され、後から再現できるか。

必須メトリクス:

- `success_check`
- `rc`
- `stop_reason` distribution
- `plan_lint_failure` count
- `dependency_missing` count
- `missing_artifacts` count
- `invalid_plan_saved` count
- `repair_exhausted` count

完了条件:

- `docs/eval/contract-recovery-report.md` を作成する。
- before/after root を併記する。
- 失敗が残る場合は、mechanism 追加ではなく contract 不足 / model limitation / provider instability に分類する。

## 推奨実行順

1. CA-R0: Evidence Freeze
2. CA-R1: Common DSL Types
3. CA-R2: Intent Propagation
4. CA-R3: Required Artifact Contract Propagation
5. CA-R4: Workspace-Aware Plan Lint
6. CA-R5: Thin Intent-Aware Profiles
7. CA-R6: Verifier And Repair Observability
8. CA-R7: Eval Regression Matrix

R1-R4 が中核である。R5-R6 は R1-R4 の上に薄く乗せる。R7 までは MVP 公開前ゲートとして扱う。

## 成功条件

最低条件:

- modify タスクが `intent: new` にならない。
- eval expected_artifacts が runtime contract として保存される。
- invalid plan の中身が後から読める。
- plan lint が既存 workspace を考慮する。
- verifier failure と dependency_missing が区別される。
- 既存 plan YAML が読める。
- `kind` / `intent` による executor 分岐が増えていない。

MVP 公開判断に使う条件:

- large 6 cases runs=1 で、失敗理由がすべて docs から再現可能。
- 0/6 のままでも、失敗が contract 不足ではなく model/provider/task 難度に分類できる。
- runs=3 は release-quality 判定として別枠にする。

## 非目標

- legacy と同等の全機能復元。
- 成功するまで自動で何度も再計画する仕組み。
- provider ごとの専用 prompt 分岐。
- Next.js だけを特別扱いして他 profile と分離した別 agent にすること。
- large task 成功率だけを目的にした rule 追加。

## 主要リスク

### DSL が太りすぎる

対策:

- enum は少数に限定する。
- unknown を許容し、分類不能なタスクを止めない。
- StepKind は制御機構ではなく lint / prompt / reporting の責務境界に限定する。
- `kind` ごとの executor を作らない。

### 後方互換を壊す

対策:

- old plan fixture を残す。
- missing fields は default 補完する。
- 保存時のみ新 schema へ正規化する。

### profile が太る

対策:

- profile は framework の事実契約だけを持つ。
- provider/model 固有の失敗は profile に入れない。
- common DSL で解けるものは profile に入れない。
- profile snapshot test で増分を見える化する。

### eval に過適合する

対策:

- expected_artifacts は eval 専用ではなく user contract として扱う。
- normal `/ultra-plan-run` では artifacts 未指定でも動く。
- eval report では改善値だけでなく failure class の変化を見る。

### repair が legacy 化する

対策:

- repair 回数は増やさない。
- repair 失敗後は short packet を保存し、明示的な `/ultra-plan-run` に逃がす。
- sidecar summarization は後回しにする。

## リカバリ範囲の妥当性チェック

### 妥当と判断する範囲

以下は復元対象として妥当である。

| 対象 | 妥当性 | 理由 |
| --- | --- | --- |
| common DSL `intent` | 妥当 | modify が new に落ちる失敗を防ぐ契約であり、能力追加ではない。 |
| common DSL `kind` | 条件付きで妥当 | setup / verify / report の責務境界を明確にするために必要。ただし executor 分岐には使わない。 |
| `expected_result` | 妥当 | TDD red や dependency_missing を成功偽装と区別するための観測契約。 |
| expected_artifacts propagation | 妥当 | eval 専用ではなく、ユーザーが要求した成果物 path の契約伝播である。 |
| workspace-aware lint | 妥当 | false positive を減らす決定論的検査。shallow existence check に限定すれば複雑化は小さい。 |
| invalid plan 保存 | 妥当 | triage 不能を防ぐ観測性。制御機構ではない。 |
| stop_reason 標準化 | 妥当 | max_iterations / dependency_missing / verifier failure を分ける観測性。 |
| verifier evidence 標準化 | 妥当 | repair を強くするのではなく、同じ証拠を人間と次の plan が見られるようにする。 |

### 条件付きで妥当な範囲

以下は必要だが、実装を誤ると膨張する。

| 対象 | 条件 |
| --- | --- |
| intent detector | 明示 `--intent` を優先し、判定不能なら `unknown`。細かいヒューリスティックを増やさない。 |
| intent-aware profile | common DSL で表現できない framework 固有の事実だけに限定する。 |
| required artifacts CLI | eval 用隠し機能にせず、ユーザー向けの artifact contract として docs 化する。 |
| repair short packet | repair 回数は増やさず、再投入可能な情報保存だけに留める。 |

### 範囲外のままにすべきもの

以下は今回の recovery に含めない判断が妥当である。

| 対象 | 理由 |
| --- | --- |
| sidecar semantic summary | 観測情報の決定論的改善を先に行うべき。導入すると provider / cost / failure mode が増える。 |
| case memory / anti-pattern | legacy 的な外側知識ベースであり、MVP の単純性に反する。 |
| multi-stage repair | 成功率は上がる可能性があるが、制御肥大化の入口になる。 |
| provider-specific prompt | GPT/Gemini/Ollama 別の場当たり分岐は保守性を落とす。 |
| Next.js 専用の隠し rule | common DSL で解ける問題を profile に押し込むと横展開できない。 |

### 最終判定

リカバリ範囲は概ね妥当である。

ただし、実装時の優先順位は R1-R4 を厳守する。R5 profile と R6 repair observability は、R1-R4 後の残存失敗を見て必要最小限に留める。これにより、今回の作業は「機能追加」ではなく「削りすぎた契約の復元」として成立する。
