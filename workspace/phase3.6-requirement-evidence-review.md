# Phase 3.6 Review: Requirement Set + Evidence + Progress Budget

## Findings

1. 高: `Requirement Set` の粒度を早い段階で増やしすぎると、保守性より複雑性が勝ちます。`output_root_exists`, `deliverable_written`, `browser_entry_verified`, `review_completed`, `final_summary_ready` くらいまでは妥当ですが、要件を細かく分割しすぎると tool result evaluator と phase transition の整合性維持が難しくなります。Phase 3.6 の初期実装では create task に限定し、5-7 個程度の requirement に抑えるべきです。

2. 高: `Evidence` を tool 名だけで判定すると精度が不足します。たとえば `read_file(index.html)` は verification evidence になりえますが、単に読んだだけで `browser_entry_verified` を満たすと扱うと誤判定になります。最低でも `path`, `result shape`, `output_root との対応`, `empty/non-empty`, `main deliverable かどうか` を含めて evidence 化しないと、柔軟性は出ても誤収束しやすいです。

3. 高: `Progress Score` は便利ですが、単純な `+2 / +1 / 0 / -1` だけでは provider や task に対して脆いです。ローカルLLMは同じ requirement に対して複数の確認手順を踏みやすく、そこを全部 `0` 扱いすると必要な verification まで stalled 判定されます。score は単一値より、`new_requirement_satisfied`, `requirement_strengthened`, `no_requirement_change` のカテゴリを残した方が安全です。

4. 中: `adaptive budget` を入れる場合、`max_steps` の代わりに見える budget を UI に出さないと挙動が分かりにくくなります。今の UX 改善方針と整合させるなら、`remaining budget`, `stall count`, `last progress` を表示した方がよいです。さもないと「なぜ今回は長く続き、別の時は早く止まるのか」が説明不能になります。

5. 中: `finalize` を requirement 化するのは正しいですが、`final_summary_ready` を内部だけで満たしたことにするとモデルが最終回答を返せないまま budget を使い切る恐れがあります。`final_summary_ready` は「モデルが final を返せるだけの evidence が揃った」という内部状態として扱い、actual final text の生成自体は requirement から切り離した方がよいです。

6. 中: `Requirement Set` は create task には強い一方、inspect task や branch explanation への一般化は別問題です。Phase 3.6 で create task 専用の仕組みとして導入するのは妥当ですが、後で inspect 系へ拡張するなら requirement taxonomy を分離しないと保守性が落ちます。`CreateRequirement`, `InspectRequirement` のように将来分割できる形で入れるべきです。

7. 中: `Evidence Evaluator` は監査ログとも接続した方がよいです。そうしないと「なぜ finalize に入ったのか」「なぜ stalled 扱いしたのか」を後から追えません。少なくとも `requirements_before`, `evidence_gained`, `requirements_after`, `progress_class` を監査イベントか debug trace に残せる設計にした方が運用しやすいです。

8. 中: provider 差異を過小評価しない方がよいです。Ollama と LM Studio で tool call の出方や text/final の癖が違うので、`Evidence Evaluator` は tool result 後の内部状態だけを見て、モデル出力の文面に依存しないようにするべきです。これは拡張性の観点で重要です。

9. 低: `vibe-local` の `MAX_SAME_TOOL_REPEAT` は単純ですが効果があります。一方、Phase 3.6 の方式はそれより複雑なので、最後の safety net として単純 repeat detector は残した方がよいです。`Requirement Set` がうまく働かない場合の fail-safe として価値があります。

10. 低: `OpenCode` や `Codex` から学ぶべき本質は、単に compaction があることではなく「内部状態を圧縮しても agent policy が崩れないこと」です。Phase 3.6 の requirement/evidence 状態も summary と同様に compact/persist 可能にしておくと、Phase 3.5 と自然に繋がります。

## Recommendations

1. Phase 3.6 の初期 requirement は create task に限定して 5 個前後に抑える。
2. `Progress Score` は数値 1 本ではなく、`progress class + optional score` にする。
3. `Evidence Evaluator` は `tool`, `path`, `output_root match`, `main deliverable`, `non-empty result` を使って判定する。
4. `finalize` は「actual final text 生成」ではなく「final text を返してよい内部状態」として扱う。
5. UI と audit に `remaining requirements`, `progress class`, `stall count` を出す。
6. repeat detector は消さず、最後の fail-safe として併用する。

## Verdict

方向性は良いです。特に、phase 固定ルールから requirement/evidence 駆動へ重心を移すのは、柔軟性と精度の両立に最も効きます。

ただし、Phase 3.6 は設計を欲張ると一気に複雑になります。create task に限定し、requirement 数を絞り、`progress class` を中心に据える実装なら、性能・拡張性・保守性のバランスは十分に取れます。

## Applied Responses

以下の方針で計画へ反映した。

1. Phase 3.6 の対象を create task に限定する。
2. 初期 requirement 数を 5 個前後に抑える。
3. `Progress Score` 単独ではなく `progress class + optional score` を採用する。
4. `Evidence Evaluator` は `tool`, `path`, `output_root match`, `main deliverable`, `non-empty result` を使って判定する。
5. `finalize` は actual final text と分離し、`final を返してよい内部状態` として扱う。
6. `remaining requirements`, `progress class`, `stall count`, `remaining budget` を UI に出す。
7. `requirements_before`, `evidence_gained`, `requirements_after`, `progress_class` を audit/debug trace に残す。
8. repeat detector は fail-safe として残す。
9. requirement/evidence state は compaction / persistence 対象に含め、Phase 3.5 の carryover と接続する。
