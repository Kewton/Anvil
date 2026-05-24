# 6. 評価方法を変える

## 現在の問題

大量 E2E 評価を先に回すと、次の問題が起きる。

- 同じ既知 failure を何度も再サンプリングする。
- LLM の揺らぎと controller の構造問題が混ざる。
- 失敗ログは増えるが、修正対象が明確にならない。
- PAM あり / なしの差分が noise に埋もれる。

そのため、評価を段階化する。

## 評価フェーズ

### Phase 1: State transition unit tests

対象:

- `RepairJob::next_action()`
- `apply_event()`
- `RepairPlan` validation
- `PatchAdmission`

確認項目:

- malformed diagnostic は budget 内で retry。
- authority 不足なら patch に進まない。
- same target / same intent / same reject の反復を止める。
- verifier improved は positive delta として保存される。
- verifier unchanged / worsened なら re-plan / target switch。
- ambiguity は test weakening ではなく safe stop。

### Phase 2: Synthetic verifier failure tests

LLM を使わず、固定の `FailurePacket` と `RepairPlanProposal` で確認する。

ケース:

- missing dependency
- local import mismatch
- syntax error
- runtime error
- generated test bug
- assertion ambiguity
- setup/config failure

狙い:

- controller が正しく next action を出せるか確認する。
- LLM 品質に依存しない構造品質を測る。

### Phase 3: Diagnostic LLM contract tests

実際の local LLM に bounded prompt を渡す。

確認:

- JSON schema を守るか。
- ambiguity を明示できるか。
- authority を勝手に作らないか。
- prior reject を反映するか。

対象モデル:

- `qwen3.6:27b-coding-nvfp4`
- fallback として `qwen3.5:9b`

### Phase 4: Patch proposal tests

accepted `RepairStep` を渡し、1 file / 1 intent の patch を出せるか確認する。

確認:

- unrelated file を触らない。
- assert deletion をしない。
- allowed change kind を逸脱しない。
- patch admission に通る。

### Phase 5: Small smoke

PAM なし 5 回。

目的:

- 成果物生成から verifier 到達まで進むか。
- repair phase で同じ無効 repair を繰り返さないか。
- safe stop が actionable か。

### Phase 6: Expanded evaluation

small smoke で改善が見えたら実施する。

- PAM なし 20 回
- PAM あり 20 回

記録:

- verified success
- actionable safe stop
- unsafe failure
- average iterations
- repair attempts
- repeated invalid repair count
- final terminal reason

## 評価ログの配置

評価ログは task workdir 内に置かない。

理由:

- artifact detection を汚染する。
- README / tests / app 以外のファイルが current task 成果物と誤認される。

配置:

- `/private/tmp/anvil-v0416-eval/...`
- `workspace/v0.4.16/eval-summary.md`

## 判定基準

### 次の実装へ進んでよい条件

- state transition unit tests が pass。
- synthetic failure tests が pass。
- diagnostic LLM が 主要ケースで valid JSON を返す。
- PAM なし small smoke で unsafe failure が減る。

### 40 回評価へ進まない条件

- smoke で同じ structural failure が再現する。
- repair が同じ target / same reject を繰り返す。
- safe stop が actionable でない。
- test weakening が validator を通る。

## v0.4.16 の評価ゴール

短期ゴール:

- verifier repair が非収束でも、理由付き safe stop になる。
- `assistant stopped before repairing` のような終了を減らす。
- deterministic repair の反復ではなく、RepairJob state で次 action が決まる。

中期ゴール:

- PAM なしで `verified_success + actionable_safe_stop` が 80% 以上。
- PAM ありで verified success が上がる。
- FastAPI CRUD 以外でも同じ terminal reason 分類が使える。

