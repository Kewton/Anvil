# Plan Root Cause Review With vibe-local

## 目的

`Plan` モードの停滞や shallow な plan 品質について、これまでの見立てが十分かを `vibe-local` と比較しながら再検討する。

このメモの結論は、従来の

- モデルが探索を好む
- repo 固有の観察が浅い
- stage 制御が弱い

という整理は部分的には正しいが、主因を単純化しすぎていた、というものである。

## 結論

`Plan` が止まりやすい根本原因は、単にモデルが探索へ流れることではない。

より正確には、次の組み合わせが問題である。

1. `anvil` の `Plan` 契約が中規模ローカル LLM にはやや重い
2. その割に `vibe-local` ほど中央集権的な loop 制御と salvage が強くない
3. classifier / transport の不安定さが入口で混ざる

つまり、根本原因は

`探索優先`

単体ではなく、

`重い Plan 契約に対して中央の回復力が足りないこと`

である。

## 以前の見立てのどこが正しく、どこが足りなかったか

以前の見立てで正しかった点:

- `Read README` のような探索へ流れやすい
- repo 固有の観察が浅いまま plan を埋めがち
- stage 情報はあるが runtime で十分に強制できていない

足りなかった点:

- `Plan` 自体の契約の重さを十分に考慮していなかった
- tool loop や malformed args への中央制御不足を主因として扱っていなかった
- classifier / transport failure が `Plan` 入口の不安定さを大きくしている点を、`Plan` 問題と十分に接続できていなかった

## vibe-local から見えること

`vibe-local` は、計画の構造的な厳密さより先に、壊れず前進することを優先している。

観察できた特徴:

- malformed / empty response に retry と backoff がある
- 同じ tool call の反復を正規化比較して止める
- 壊れた tool args を積極的に salvage する
- plan mode 自体は比較的シンプルで、主に read-only tools と plans 配下への write 制限に寄っている

対して `anvil` は、`PlanStage`、`Quality Bar`、`Execution Plan`、`Verification Plan`、approval readiness などを持ち、より高品質な plan を要求している。

これは方向としては正しいが、中規模ローカル LLM にとっては `vibe-local` より要求が重い。

## anvil の現状で起きていること

`anvil` は現在、次のような特徴を持つ。

- stage-aware prompt
- stage state
- stage ごとの exploration budget
- approval readiness
- classifier retry
- classifier failure 時の heuristic fallback
- tool argument normalization

ここまでで入口や parser は改善している。

ただし、依然として足りないものがある。

- normalized repeated tool detection が弱い
- tool-level block や salvage が `vibe-local` ほど中央化されていない
- `Plan` の契約が重く、Stage 2 以降で認知負荷が上がりやすい

## 2つの代表的な問題の再整理

### 1. 多くのモデルで空テンプレか Stage 1 止まりになる

従来の説明:

- 探索優先
- `Read README` を繰り返す

修正版:

- 探索優先だけではない
- `Plan` 契約が重く、Stage 2 以降で要求水準が急に上がる
- それに対して loop-breaking と write-forward 制御が弱い

つまり、

`重い Plan 契約 + 弱い中央制御`

が主因である。

### 2. 完走しても repo 固有の深い洞察が弱い

従来の説明:

- shallow exploration
- repo 固有の観察不足

修正版:

- shallow exploration は一因にすぎない
- repo 固有洞察を要求する評価条件が弱いまま、構造だけ整った plan が通ってしまう
- `Quality Bar` はあるが、repo-specific な観察を必須にしていない

つまり、

`一般的でも合格できる品質条件`

が残っている。

## 根本原因の修正版

`Plan` 停滞の根本原因は:

1. `anvil` が `vibe-local` より重い plan 契約を中規模モデルに要求している
2. その契約を支える loop 制御、tool-level block、salvage がまだ十分強くない
3. classifier / transport の不安定さが入口で追加ノイズになっている

このため、`anvil` は

- 良い plan が出る時は良い
- しかし中規模モデルでは安定してそこまで進めない

という性質になっている。

## 今後の対策の示唆

この見立てに立つと、次に効く対策は prompt の長文化ではない。

優先度が高いのは次である。

1. `vibe-local` 型の normalized repeated tool detection を `Plan` に入れる
2. repeated exploration に対する tool-level block を強める
3. malformed args や shallow plan 出力への salvage を増やす
4. `Quality Bar` や approval 条件に repo-specific observation を入れる
5. classifier / transport 不安定を `Plan` の単一点障害にしない

## 要約

以前の

`探索優先が根本原因`

という整理は、半分は正しいが十分ではない。

より正確には、

`anvil` は `vibe-local` より高い品質の Plan を要求している一方で、その要求を支える中央の回復力がまだ足りない`

ことが本質である。
