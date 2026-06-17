# CommandAgent Contract Recovery Plan Review

作成日: 2026-06-17

対象文書:

- `workspace/CommandAgent/recovery-plan.md`

## レビュー観点

このレビューでは、リカバリ計画が以下に反していないかを確認する。

- minimal-first 方針
- legacy engine を戻さない方針
- 「能力を足す」より「曖昧さを削る」方針
- profile を太らせない方針
- eval / docs / tests を同じ単位で扱う方針

## 総合判定

判定: 条件付きで妥当。

理由:

- 復元対象を legacy 機構ではなく deterministic contract に限定している。
- R1-R4 を中核に置いており、まず DSL / intent / artifact / lint の責務境界を戻す構成になっている。
- repair 強化や provider-specific prompt 分岐を非目標にしており、肥大化リスクを明示的に抑えている。
- eval の成功率そのものではなく、失敗理由の可観測性と分類可能性を成功条件に入れている。

ただし、R5 と R6 は実装の仕方を誤ると profile / repair が太るリスクがある。R1-R4 の後に改めて差分を見て、必要最小限に絞るべきである。

## 良い点

### 1. legacy 回帰を避けている

計画は legacy engine、case memory、sidecar、PAM、旧 repair job を明示的に非目標としている。

これは CommandAgent の移植方針と整合する。今回戻すべきなのは「賢い外側制御」ではなく、plan / verifier / eval contract である。

### 2. 真因に対応している

大規模 eval 0/6 の直接原因は、モデル能力不足だけではなく、以下の contract 欠落だった。

- modify intent が new に落ちる
- expected_artifacts が runtime に渡らない
- setup と verify が混ざる
- workspace を知らない lint が誤判定する
- failure evidence が plan / repair / report に安定して残らない

計画の R1-R4 はこの真因に直接対応している。

### 3. profile より common DSL を優先している

Next.js で目立った問題を Next.js rule だけで直すと、profile が太る。計画はまず common DSL に `intent` / `kind` / `expected_result` を入れるため、Python / Rust / docs / data 系へ横展開しやすい。

### 4. eval を製品の一部として扱っている

R7 で before/after root、failure class、dependency_missing、stop reason を見ることにしている。これは、移植計画の「評価スクリプトと評価 docs も MVP の一部」という方針に合う。

## 懸念と修正提案

### 懸念 1: StepKind が制御機構に化ける可能性

`kind` を導入すると、将来的に `kind=verify` なら別 loop、`kind=repair` なら別 retry、という方向に膨らむリスクがある。

対策:

- 初期実装では `kind` を lint / prompt / reporting に限定する。
- loop の分岐は最小限にする。
- `kind` ごとの executor を作らない。

### 懸念 2: Intent detector がヒューリスティック過多になる可能性

未指定時の deterministic detector は便利だが、日本語・英語・複合タスクを細かく分類しようとすると rule が増える。

対策:

- CLI/eval の明示 `--intent` を優先する。
- detector は `new | modify | investigate | document | unknown` 程度に留める。
- 判定できなければ `unknown` にする。
- unknown は失敗ではなく generic contract で進める。

### 懸念 3: expected_artifacts が eval 過適合になる可能性

eval case の `expected_artifacts` を runtime に渡すと、bench 専用最適化に見える可能性がある。

対策:

- eval だけでなく CLI でも `--artifact <path>` または plan file contract として使える設計にする。
- docs では「ユーザーが要求した成果物契約」と説明する。
- artifacts 未指定の通常利用では挙動を変えない。

### 懸念 4: Workspace-aware lint が filesystem read を増やす

lint が workspace を読むことで、plan generation のたびに I/O が増える。

対策:

- 読む範囲は cwd 配下の shallow existence check に限定する。
- ファイル内容までは読まない。
- glob 展開はしない。
- path confinement を必ず通す。

### 懸念 5: Profile が intent-aware になると太りやすい

`profile_contract(profile, intent)` は便利だが、profile に修正履歴が積み上がる危険がある。

対策:

- profile は framework の事実契約に限定する。
- 失敗 run 個別の対策は profile に入れない。
- common DSL で解ける問題は profile から排除する。
- profile snapshot test で増分を見える化する。

### 懸念 6: R6 が repair 強化に見える

verifier / repair evidence を厚くすると、repair を賢くする方向に読める。

対策:

- repair 回数は増やさない。
- repair prompt のサイズ制限を維持する。
- 追加するのは証拠の保存と圧縮であり、判断ロジックではない。
- sidecar semantic summary は今回の範囲外にする。

## 順序レビュー

推奨順は妥当。

R1 Common DSL Types がないと、R2 intent propagation も R4 lint も prompt 依存になる。

R2 Intent Propagation がないと、modify / investigate / docs の評価が壊れたままになる。

R3 Required Artifact Contract Propagation は大規模 eval の path drift に直結するため、R4 より前に入れてよい。

R4 Workspace-Aware Plan Lint は false positive 対策として中核。

R5 Profile は R1-R4 の後でよい。先に profile を直すと Next.js 専用修正に寄る。

R6 Observability は R1-R4 の途中で並行可能だが、実装負荷を考えると後続でよい。

R7 Eval は最後でよい。ただし R1-R4 の各 PR でも小さな fixture eval は入れるべきである。

## 不足している観点

### 1. Backward compatibility の明記がやや弱い

既存 `.commandagent/plans/*.yaml` を読めなくするとユーザー体験が悪くなる。

追加推奨:

- old plan read test を各 schema 変更に含める。
- missing `intent` / `kind` / `expected_result` は default 補完し、保存時に新 schema へ正規化する。

### 2. Metrics の定義が必要

R7 で見る指標は書かれているが、最小限の数値列が未定義。

追加推奨:

- success_check
- rc
- stop_reason distribution
- plan_lint_failure count
- dependency_missing count
- missing_artifacts count
- invalid_plan_saved count
- repair_exhausted count

### 3. CommandAgent と Anvil の差分台帳が必要

同じ問題を再発させないため、今回の復元対象が「Anvil から何を戻したのか」を残すとよい。

追加推奨:

- `docs/adr/0002-contract-recovery.md` を作り、戻す契約 / 戻さない機構を明記する。

## 修正すべき文言

`R5: Thin Intent-Aware Profiles` の説明は、読む人によっては profile 強化に見える。

推奨表現:

- "profile を intent ごとに賢くする" ではなく、"common DSL で表現できない framework 固有の事実だけを intent ごとに提示する" と書く。

`R6: Verifier And Repair Observability` は repair 改善に見える。

推奨表現:

- "repair を強くする" ではなく、"repair が失敗したときにユーザーと次の plan が同じ証拠を見られるようにする" と書く。

## 最終レビュー結論

このリカバリ計画は、現在の CommandAgent の設計思想に概ね従っている。

ただし、実装時は以下を守る必要がある。

1. R1-R4 を先に完了する。
2. profile / repair の拡張は R1-R4 後の失敗差分を見てから最小限にする。
3. `kind` / `intent` は executor 分岐を増やすためではなく、責務境界を明確にするためだけに使う。
4. eval 成功率だけで判断せず、failure class が contract 不足から外れたかを見る。

この条件を満たすなら、計画は「足し算の機構追加」ではなく「削りすぎた契約の復元」として妥当である。

## レビュー反映確認

以下の指摘は `workspace/CommandAgent/recovery-plan.md` に反映済み。

| 指摘 | 反映内容 |
| --- | --- |
| 既存 plan YAML との後方互換が弱い | R1 と成功条件に old plan fixture / default 補完 / 保存時正規化を追加。 |
| R7 metrics が未定義 | 必須メトリクスとして `success_check`、`rc`、`stop_reason`、`plan_lint_failure`、`dependency_missing`、`missing_artifacts`、`invalid_plan_saved`、`repair_exhausted` を追加。 |
| 差分台帳が必要 | R0 に `docs/adr/0002-contract-recovery.md` 作成を追加。 |
| R5 が profile 強化に見える | R5 の目的を「common DSL で表現できない framework 固有の事実だけを intent ごとに提示する」に修正。 |
| R6 が repair 強化に見える | R6 の目的を「同じ証拠をユーザーと次の plan が見られるようにする」に修正。 |
| StepKind が制御機構化するリスク | 原則、R1、成功条件、主要リスクに `kind` ごとの専用 executor を作らない制約を追加。 |
| intent detector がヒューリスティック過多になるリスク | R2 と主要制約に明示 `--intent` 優先、判定不能時 `unknown` を追加。 |
| expected_artifacts が eval 過適合に見える | R3 に CLI artifact contract と docs 化を追加。 |
| workspace-aware lint の I/O 増加 | R4 に shallow existence check 限定、ファイル内容読取なし、glob 展開なしを追加。 |

## リカバリ範囲の妥当性監査

### 監査基準

各復元対象を以下の 4 分類で確認した。

1. Contract: 責務境界、型、成果物契約、観測性を明確にするもの。
2. Mechanism: 成功率を上げるための新しい制御・修復機構。
3. Legacy: Anvil の旧アーキテクチャに由来する重い仕組み。
4. Profile Rule: framework 固有の事実契約、または場当たりルール。

今回の recovery に含めてよいのは原則 Contract のみ。Profile Rule は common DSL で解けない framework 事実に限定する。Mechanism と Legacy は含めない。

### 復元対象の分類

| 対象 | 分類 | 判定 | 理由 |
| --- | --- | --- | --- |
| `intent` | Contract | 妥当 | 作業意図を runtime に渡す schema。model に暗黙推定させないための境界。 |
| `step kind` | Contract | 条件付き妥当 | setup / verify / report の混在を防ぐ。ただし executor 分岐に使うと Mechanism 化する。 |
| `expected_result` | Contract | 妥当 | red / pass / unavailable を区別する観測契約。 |
| expected_artifacts propagation | Contract | 妥当 | eval と runtime の成果物契約を一致させる。bench 専用ではなく user contract として扱う必要あり。 |
| workspace-aware plan lint | Contract | 妥当 | 既存ファイルを知らない lint の false positive を防ぐ。shallow existence check に限定すれば低リスク。 |
| intent-aware profile | Profile Rule | 条件付き妥当 | framework 事実だけなら妥当。provider/model 固有対策を入れると不適切。 |
| verifier evidence packet | Contract | 妥当 | repair 能力追加ではなく、失敗証拠の保持。 |
| stop_reason enum | Contract | 妥当 | max_iterations / dependency_missing / turn_error を分離する観測性。 |
| invalid plan 保存 | Contract | 妥当 | plan lint 失敗を再現可能にする。 |
| old plan compatibility | Contract | 妥当 | ユーザー資産を壊さない migration contract。 |

### 除外対象の分類

| 対象 | 分類 | 除外判定 |
| --- | --- | --- |
| legacy engine | Legacy | 除外妥当。CommandAgent の minimal-only 方針に反する。 |
| case memory / anti-pattern | Legacy | 除外妥当。外側知識ベースであり MVP を太らせる。 |
| sidecar semantic summary | Mechanism | 除外妥当。決定論的 evidence 改善を先に行うべき。 |
| multi-stage repair | Mechanism | 除外妥当。bounded repair の思想に反する。 |
| provider-specific prompt | Profile Rule / Mechanism | 除外妥当。保守性と横展開性を落とす。 |
| Next.js 専用 hidden rule | Profile Rule | 除外妥当。common DSL で解けるものを profile に入れるべきではない。 |

### 境界上の注意点

- `intent-aware profile` は必要だが最も膨張しやすい。R1-R4 完了後の残存失敗を見てから最小限にする。
- `expected_artifacts` は eval から渡すだけだと bench 最適化に見える。CLI / plan file でも扱える user contract として設計する必要がある。
- `step kind` は強力だが、実行戦略を変えるために使うと legacy 化する。初期用途は lint / prompt / report に限定する。
- verifier evidence は厚くしてよいが、repair 回数や自動再計画回数は増やさない。

### 監査結論

リカバリ範囲は妥当。

今回の対象は、CommandAgent の能力を足すものではなく、移植時に削りすぎた契約を戻すものとして整理できている。特に R1-R4 は CommandAgent の大規模失敗の真因に直接対応しており、MVP 公開前に優先してよい。

一方、R5 と R6 は境界上にある。これらは R1-R4 の後、残存失敗を見て最小限に実装するべきで、先に大きく入れるべきではない。
