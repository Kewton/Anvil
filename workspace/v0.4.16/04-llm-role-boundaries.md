# 4. LLM の役割を分ける

## 基本方針

LLM を使う。

ただし、LLM に「次に何をすべきか」を自由に決めさせない。

LLM には狭い仕事だけ渡す。

## main LLM

役割:

- 初期実装
- required artifact の作成
- accepted `RepairStep` に基づく狭い edit

やらせないこと:

- verifier failure 全体の最終判断
- authority の決定
- retry budget の決定
- target switching
- safe stop 判定

main LLM への入力:

- current task
- current target file
- accepted repair step
- allowed change kind
- must preserve
- previous rejection summary

main LLM の出力:

- 1 file edit
- または tool call なしなら retry event

## diagnostic LLM

役割:

- verifier failure の意味診断
- structured `RepairPlanProposal` の作成
- ambiguity の明示

やらせないこと:

- patch の直接適用
- shell command の選択
- authority の最終決定
- test weakening の許可

diagnostic LLM への入力:

- `FailurePacket`
- bounded excerpts
- controller-computed authority evidence
- prior rejected attempts
- verifier delta history
- runtime adapter hints
- PAM retrieval hints

diagnostic LLM の出力:

- JSON only
- `RepairPlanProposal`
- confidence / ambiguity / expected delta

## Anvil controller

役割:

- `RepairPlanProposal` validation
- accepted `RepairPlan` の作成
- `RepairJob::next_action()` による状態遷移
- authority 判定
- target selection
- retry budget
- patch admission
- verifier rerun
- terminal condition 判定

Anvil が絶対に守ること:

- LLM / PAM / README / verifier output は untrusted。
- workspace 外 path は触らない。
- generated test を根拠なく弱めない。
- verifier observation を仕様 authority にしない。
- dependency install は policy に従う。

## PAM

役割:

- 類似 failure の検索
- 過去の reject / successful repair の retrieval
- diagnostic LLM への補助 context

やらせないこと:

- authority の決定
- patch の直接適用
- target path の無検証採用

PAM retrieval key:

- failure category
- affected case
- observed / expected
- target role
- reject reason
- runtime
- verifier command family

PAM の出力は plan candidate の補助情報であり、controller validation を必ず通す。

## 役割分離の狙い

ローカル LLM は、大域的な制御や長い retry 管理が苦手。

一方で、局所的な意味診断や小さい edit は得意な場合がある。

そのため、Anvil は「制御」と「検証」を持ち、LLM は「提案」と「局所修正」を担当する。

## 期待する挙動

例: `201 vs 200`

1. verifier が assertion mismatch を返す。
2. diagnostic LLM が `status_code expectation mismatch` と診断する。
3. Anvil が user request / README / behavior contract を確認する。
4. status code の authority がなければ、test edit を許可しない。
5. implementation が public interface として妥当でも、Anvil は勝手に test を green にしない。
6. `actionable safe stop` として「POST の期待 status code が未指定」と報告する。

例: `NameError: pytest is not defined`

1. diagnostic LLM または runtime hint が test setup bug と診断する。
2. Anvil が target test file と allowed change kind を承認する。
3. patch provider が `import pytest` を提案する。
4. patch admission が import 追加だけであることを確認する。
5. verifier rerun へ進む。

