# Anvil vs vibe-local Audit

日付: 2026-03-12

対象:

- Anvil
- vibe-local (`/Users/maenokota/share/work/github_kewton/vibe-local`)

前提:

- 同一ローカルモデル (`qwen3.5:35b`) を利用
- create task として「ブラウザから直接実行可能なスペースインベーダーゲーム」の生成を比較
- 観点は安定性、性能、生成品質、保守性、拡張性、UX

## Findings

1. 高: read-only result cache に invalidation がなく、write/edit/mkdir 後でも古い `read_file` / `list_dir` / `path_exists` を再利用し得る。verify/review が stale state を読む危険がある。

2. 高: create task の phase truth source が二重化している。`RequirementState` を持ちながら `create_phase_for_task()` は別の heuristic で進むため、`remaining requirements` と `current phase` がズレる。

3. 高: provider-native tool calling の streaming 利用が弱い。Anvil は tool use で同期呼び出し中心、vibe-local は tool streaming 検出と streaming path を活かしている。

4. 高: prompt が巨大な単一文字列に寄りすぎている。Anvil は `Project instructions + Memory + tool history + contract + quality targets + hint + task` を 1 つの prompt に束ねている一方、vibe-local は会話 message 列に近い構造を維持する。

5. 中: creative guidance は追加されたが、品質向上目標の達成を評価する仕組みがない。結果として prompt は重くなるが、出力品質向上は運任せ。

6. 中: tool loop 側の compaction が弱い。Anvil の loop は `compact_turns()` で直近重視に丸めるだけで、失敗の要約・戦略転換が十分ではない。

7. 中: create task の探索 budget はまだ小さい。`vibe-local` の safety limit は 50 iteration で、Anvil より探索余地が大きい。

8. 低: `ANVIL.md` / `ANVIL-MEMORY.md` の読み込みが毎回重複している。性能面では小さいが、tool loop が長いと効いてくる。

## Why vibe-local looks stronger

- system prompt が「作る」方向へ強く寄っている
- tool-first と silent recovery の方針が明確
- tool streaming と duplicate detection が成熟している
- session compaction と長時間 loop の運用がこなれている

## Recommended Fix Order

1. stale cache invalidation
2. phase truth source の一元化
3. provider-native tool streaming の導入
4. prompt/message 構成の見直し
5. quality target evaluator の追加

## Applied Responses (this turn)

1. 上位の論点を `Phase 3.8` の設計課題として計画書へ反映する
2. `cache invalidation matrix`, `phase truth source migration`, `provider streaming capability model`, `message-structured prompt layout` を別設計書に具体化する
3. `provider runtime policy`, `working transcript -> carryover summary` 昇格規則, `RequirementState` の粒度, `repeat recovery` fail-safe を Phase 3.8 設計へ追加する
4. `RequirementState` を `EntryPointVerified / CoreBehaviorVerified` 粒度へ見直し、`transcript retention policy`, `fallback reason codes`, `assistant transition note`, `evidence delta` を Phase 3.8 設計へ追加する
