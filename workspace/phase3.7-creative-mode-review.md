# Phase 3.7 Review: Quality Targets + Stretch Goals + Creative Mode

## Findings

1. 高: `quality targets` を requirement と混ぜると、Phase 3.6 の収束制御を壊します。`start screen` や `enemy fire` のような品質目標を blocking requirement にすると、stalled loop が増えます。Phase 3.7 では creative guidance は `non-blocking` に固定すべきです。

2. 高: creative mode を task 文言の単純マッチで増やしすぎると、ルールベース化が進みます。`cool`, `polished`, `いけてる`, `カッコ良い` などを見ても、せいぜい `standard/enhanced` の切り替え程度に留めるべきです。具体的な tool sequence や fixed workflow に繋げるべきではありません。

3. 中: quality targets の粒度を増やしすぎると保守性が落ちます。初期は `browser-runnable`, `playable core loop`, `clear restart path`, `visible HUD`, `basic visual polish` 程度に抑えるのが妥当です。deliverable ごとに 4-6 個程度が上限です。

4. 中: `stretch goals` は価値がありますが、実装・検証・レビューの各 phase で露出しすぎると prompt が冗長になります。prompt には full list を入れつつ、UI には代表例 2-3 個だけ出す方が性能と可読性のバランスがよいです。

5. 中: `vibe-local` が richer output を出す理由は creative bias の強さだけでなく、HTML/JS 優先や「interactive app は HTML/JS」が強く system prompt に入っている点です。Anvil 側も `browser-runnable html_app` に対しては creative targets を 조금強めるのが有効ですが、Rust code など他 deliverable へ同じ bias を掛けるべきではありません。

6. 中: quality targets は audit / compaction / persistence から見える必要があります。後で「なぜこの game が HUD を持っているのか」を説明するには、targets が session state に残っている方がよいです。最低限 prompt と UI に載せ、将来的には session summary にも織り込める形にしておくべきです。

7. 低: `creative_mode` という名前は分かりやすいですが、内部的には `guidance intensity` です。将来、design-heavy な web app と code-heavy な scaffolding で分けたくなるので、enum は増やしやすい形にしておくとよいです。

## Applied Responses

1. `quality targets` と `stretch goals` は requirement/evidence から明確に分離する。
2. `creative_mode` は `disabled / standard / enhanced` の 3 段階に抑える。
3. quality targets は create task 専用、かつ deliverable ごとに 4-6 個前後に制限する。
4. UI には代表的な targets / stretch goals だけを出し、prompt には完全版を入れる。
5. `browser-runnable html_app` と game 系 create task に重点を置き、他 deliverable へは控えめに適用する。
6. guidance は tool sequence を固定せず、model-driven のまま使う。

## Summary

Phase 3.7 の方向性は良いです。Anvil が `vibe-local` より conservative に寄る問題に対して、creative guidance を requirement と分離して導入するのは筋が良いです。

ただし、Phase 3.7 は「 richer output を出したい 」欲求から設計を盛りすぎると、再びルールベース化と prompt 肥大を招きます。初期実装は create task 専用、3 段階 creative mode、少数の quality targets / stretch goals に絞るのが妥当です。
