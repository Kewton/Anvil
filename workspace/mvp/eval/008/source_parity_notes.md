# Phase 0 source parity notes

## 対象

`step_plan_executable_verify_strength_work_breakdown.md` Phase 0 の確認メモ。

## 確認観点

- plan 生成後の quality warning / retry 相当機構
- verify command allowlist / policy
- profile guidance / profile verify / postcheck の責務分担
- plan-run に渡す step instruction 補強
- eval failure classification が移植元 failure を過度に不利に扱っていないか

## 確認結果

### 移植元側

`src/agent/minimal_step_runner/*` には、StepPlan lint と verify command policy が存在する。

主な特徴:

- verify command allowlist が MVP より狭い
- shell control syntax や allowlist 外 command を拒否する
- Next.js build ordering の lint がある
- planner quality warning はあるが、今回の MVP で追加するような `retryable_quality` と last valid plan fallback の閉ループは確認範囲では主要機構ではない

今回の anvildev eval 失敗は、`node smoke-check.js` / `python -m unittest ...` / `npm run build` が source 側 policy と suite expectation の間で噛み合わず、YAML 保存前に拒否されたものを含む。

### MVP 側

`mvp/anvilminimal/src/planner/*` は、JSON StepPlan 生成、schema repair、fatal lint、profile guidance、profile verify を持つ。

主な特徴:

- StepPlan 生成成功率と artifact ownership は高い
- verify command policy は source より緩い
- `step_plan_quality_warnings` は event 記録中心で、valid だが弱い plan を再生成へ戻す力が弱い
- profile guidance は prompt / final step instruction 補強として効くが、生成後 self-check の根拠としては弱い

## 移植漏れかどうか

今回の主問題は、単純な source safeguard 取りこぼしとは断定しない。

理由:

- source 側は verify allowlist が強く、今回の suite では policy mismatch による失敗がある
- MVP 側は YAML 生成成功率を改善済みで、失敗ではなく valid plan の品質不足が問題
- `retryable_quality` と last valid plan fallback は、source の厳格 policy をそのまま移すより、MVP の設計思想に合わせた閉ループとして追加するのが自然

ただし、source 側の以下は MVP 実装でも参考にする。

- verify command policy と dependency ordering は deterministic に保つ
- quality diagnostic は failure classification に混ぜない
- profile / postcheck / verify の責務を混ぜすぎない

## 後続 Phase への反映

- Phase 1-3: quality self-check は source allowlist の完全移植ではなく、MVP の deterministic quality report として実装する
- Phase 4: retry は hard fail ではなく last valid plan fallback を持つ bounded retry とする
- Phase 5: prompt 補強は source policy の厳格化ではなく、self-check を満たしやすくする短い contract に留める
- Phase 7-8: anvildev comparison では source policy mismatch と MVP quality issue を分離して評価する
