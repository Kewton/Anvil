# Plan Improvement Priorities

## 目的

`Plan` モードの改善候補を、`vibe-local` 比較と最近の実験結果を踏まえて優先順位付きで整理する。

このメモでは、単に思いついた改善を列挙するのではなく、

- 何に効くか
- なぜ優先度が高いか
- どの副作用があるか

まで含めて整理する。

## 前提

現在の `anvil` は次の状態にある。

- stage-aware prompt は入っている
- `PlanStage` state も入っている
- exploration budget もある
- classifier retry と heuristic fallback もある
- tool argument normalization も一部入っている

それでもなお、主ボトルネックは `Plan` の停滞である。

特に残っている問題は次の2つ。

1. `Read README` などの探索反復から抜けられない
2. 完走しても repo 固有の深い観察が弱い

## 優先順位

### 優先度1: normalized repeated tool detection

#### 内容

`Plan` 中の `Read / Glob / Grep` を、文字列ではなく正規化した意味単位で比較し、同じ探索反復を検出する。

例:

- `Read("README.md")`
- `Glob("*")`
- 同じ stage のまま同じ対象を読む

#### なぜ最優先か

- `vibe-local` で効いている中核の1つがここ
- 今の `anvil` は note ベースの停滞検出が主で、行動レベルの反復検出が弱い
- `Plan` が前進しない一番わかりやすい症状が `Read README` 反復

#### 期待効果

- 同じ探索ループを明示的に止められる
- stage-aware prompt や exploration budget が活きやすくなる

#### 副作用

- 本当に必要な再読まで止める危険

#### 緩和策

- 完全一致ではなく stage と対象を見て判定する
- threshold は 2〜3 回程度に抑える

### 優先度2: repeated exploration への tool-level block

#### 内容

反復探索を recovery note だけでなく、tool result レベルで block する。

例:

- `Error: repeated exploration blocked for current plan stage`
- `Update the plan file next`

#### なぜ高いか

- `vibe-local` は malformed args や bad action を、tool result として LLM に返す方向が強い
- `anvil` はまだ「説明する」比重が高く、「止める」比重が弱い

#### 期待効果

- `Read` 反復のあと、次の一手を `Write/Edit(plan)` に寄せやすい
- prompt 違反を runtime 側で明示できる

#### 副作用

- モデルが block を繰り返し食らって停止感が強くなる可能性

#### 緩和策

- block メッセージを短く具体的にする
- `current stage` と `next expected action` を併記する

### 優先度3: malformed args / shallow outputs の salvage 強化

#### 内容

すでに入っている `file -> path` のような alias 補正に加えて、

- `text/body -> content`
- nested wrapper の吸収
- shallow plan output への補助再試行

などを増やす。

#### なぜ高いか

- `vibe-local` は broken JSON や軽い引数崩れをかなり吸収している
- `anvil` はここが弱いと、`Plan` の質以前に `Write/Edit(plan)` が失敗する

#### 期待効果

- 中規模モデルの揺れに強くなる
- `Plan` の進行率が上がる

#### 副作用

- 補正しすぎると誤解釈で危険な action を通す可能性

#### 緩和策

- `Plan` 中は plan file への write/edit に限定して salvage を強める
- destructive action には適用しない

### 優先度4: repo-specific observation を要求する quality / approval 条件

#### 内容

`Quality Bar` と approval 条件に、repo 固有観察を明示的に入れる。

例:

- 既存内容の弱点を1つ以上述べる
- この repo の目的や読者に即した改善点を1つ以上述べる
- generic な一般論だけでは不可

#### なぜ必要か

- 今は構造が揃えば、一般的な plan でも通りやすい
- 完走しても浅い理由は、repo-specific な観察が必須になっていないから

#### 期待効果

- 完走時の質を底上げできる
- `Plan` が単なるテンプレ埋めで終わりにくくなる

#### 副作用

- さらに plan 契約が重くなる

#### 緩和策

- approval 条件にいきなり全部入れず、まず `Quality Bar` から導入する
- stage 2/3 のレビュー条件として徐々に適用する

### 優先度5: classifier / transport 不安定の継続対策

#### 内容

- classifier retry の改善
- fallback の structured logging 継続
- transport failure を `Plan` と分離して観測

#### なぜこの順位か

- 入口の安定化は重要だが、今は heuristic fallback で一応 `Plan` に寄せられるようになった
- したがって最優先は `Plan` 本体の停滞制御

#### 期待効果

- モデル差の比較がしやすくなる
- `Plan` 停滞と transport failure を混同しにくくなる

#### 副作用

- 根本改善というより観測改善寄り

## 実施順の提案

実施順は次が妥当。

1. normalized repeated tool detection
2. tool-level block
3. salvage 強化
4. repo-specific quality / approval 条件
5. classifier / transport の継続対策

## なぜこの順か

最初の3つは、`vibe-local` の強みである

- 反復を止める
- 壊れた出力を救う

に対応している。

これは `Plan` を前進させる土台であり、今の `anvil` に最も不足している部分でもある。

その後で repo 固有性を強める。  
先に品質条件を重くすると、ただでさえ止まりやすい `Plan` がさらに止まりやすくなるためである。

## まとめ

次に効く改善は、prompt の微修正ではない。

優先すべきは、

1. 反復探索を検出する
2. それを runtime で止める
3. 出力の揺れを救う

ことである。

その土台ができてから、

4. repo 固有の深い plan を要求する

へ進むのがよい。
