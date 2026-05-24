# 3. RepairJob を中心構造にする

## 現在の構造問題

`RepairJob` は既に存在している。

しかし現状では、repair lifecycle の唯一の制御主体ではない。

同じ verifier failure に対して、次の情報が分散している。

- legacy `VerifierRepairAssessment`
- `SemanticRepairPlan`
- `RepairBrief`
- `RepairAction`
- target hint
- patch proposal
- repair intent
- validator reject
- verifier rerun result
- exhausted targets

これらが 1 つの state transition として閉じていないため、診断が成功しても patch がその診断に従わなかったり、reject が次の plan に必ず反映されなかったりする。

## あるべき lifecycle

中心構造は次の 1 本にする。

```text
FailurePacket
  -> RepairPlan
  -> RepairStep
  -> PatchProposal
  -> PatchAdmission
  -> VerifierDelta
  -> next action
```

重要なのは、LLM が「次どうするか」を決めないこと。

Anvil が state と evidence から次 action を決める。

## Core model

### FailurePacket

verifier failure の観測事実。

含めるもの:

- command
- failure kind candidate
- affected cases
- observed / expected pairs
- stack trace / file hints
- candidate artifacts
- prior attempts

含めないもの:

- 仕様としての正しさ
- patch 方針
- LLM の推測

### RepairPlan

diagnostic LLM が提案する意味診断。

ただし authority ではなく proposal。

最低限の fields:

- `failure_clusters`
- `probable_cause_role`
- `source_of_truth`
- `authority_scope`
- `ambiguity`
- `repair_hypothesis`
- `target_candidates`
- `allowed_change_kind`
- `expected_verifier_delta`

### RepairStep

Anvil が承認した 1 step。

条件:

- 1 target role
- 1 target path
- 1 intent
- 1 allowed change kind
- 1 expected verifier delta

### PatchProposal

LLM または deterministic fallback が出す具体 diff。

条件:

- 1 file
- small diff
- `RepairStep` に従属
- target / authority / allowed change kind を選ばない

### PatchAdmission

Anvil の検証結果。

見るもの:

- path confinement
- role mismatch
- scope ownership
- test weakening
- assertion deletion
- broad deletion
- unrelated file edit
- dependency/config safety

### VerifierDelta

patch 後の verifier 結果。

分類:

- passed
- improved
- unchanged
- worsened
- different failure
- verifier unavailable

## RepairJob state

`RepairJob` は次を持つ。

- current `FailurePacket`
- accepted `RepairPlan`
- current `RepairStep`
- rejected attempts
- applied attempts
- verifier deltas
- exhausted target / intent pairs
- budgets
- terminal reason

## next_action

`RepairJob::next_action()` は pure に近い関数にする。

入力:

- current state
- latest event

出力:

- request diagnostic
- request patch proposal
- apply deterministic fallback
- rerun verifier
- switch target
- re-plan
- safe stop
- fail hard

## 状態遷移の原則

- diagnostic が malformed なら budget 内で再診断。
- diagnostic が authority 不足なら patch に進まない。
- patch reject は `RejectedAttempt` として保存する。
- same target / same intent / same reject が続いたら re-plan または target switch。
- verifier improved なら同じ hypothesis を継続可能。
- verifier unchanged / worsened なら同じ patch family を繰り返さない。
- ambiguity がある場合は test weakening ではなく safe stop。

## v0.4.16 での最小実装単位

最初から全実装を置き換えない。

MVP:

1. `RepairPlan` を accepted plan と proposal に分ける。
2. `RejectedAttempt` を構造化して `RepairJob` に保存する。
3. `RepairJob::next_action()` の小さな transition table を作る。
4. 既存 deterministic repair は `PatchProposalProvider` の一種として接続する。
5. 旧分岐は telemetry を出しながら fallback に降格する。

