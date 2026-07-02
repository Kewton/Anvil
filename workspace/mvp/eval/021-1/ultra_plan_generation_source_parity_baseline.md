# UltraPlan Generation Source Parity Baseline

作成日: 2026-06-29

## 目的

Phase 021-1 の実装前 baseline として、rollback 後 MVP と移植元 anvil の UltraPlan 生成契約の差分を固定する。

この baseline は「UltraPlan 生成だけ」を対象にする。phase-aware verification、runtime recovery、repair handoff、capability oracle、TUI 常時 run log は対象外である。

## MVP rollback 後の挙動

対象:

- `mvp/anvilminimal/src/planner/runner.rs::generate_ultra_plan_with_ui`
- `mvp/anvilminimal/src/planner/ultra_plan.rs::UltraPlan::deterministic`

確認した挙動:

- planner へ渡す message は user prompt 1本のみ。
- system prompt がない。
- profile generation rules が UltraPlan 生成に入らない。
- planner tool call を invalid output として明示拒否していない。
- parse / lint failure 時に `UltraPlan::deterministic(...)` を返す。
- deterministic fallback が通常 plan と同じように保存・実行される。

このため、planner failure が runtime failure や shallow artifact success に見えやすい。

## 移植元 anvil の挙動

対象:

- `src/agent/minimal_step_runner.rs::generate_ultra_plan`
- `src/agent/minimal_step_runner.rs::ultra_plan_generation_system_prompt`
- `src/agent/minimal_step_runner/profile.rs`
- `src/agent/minimal_step_runner/profiles/nextjs.rs::generation_rules`

確認した契約:

- system prompt と user prompt を分離する。
- planner は tool call を出さない。
- profile generation rules、style rules、work intent を system prompt に入れる。
- invalid output は bounded retry する。
- planner が tool call を返した場合も invalid output として扱う。
- parse 後に `goal/profile/style/intent` は request context で canonicalize する。
- retry exhaustion 後は `invalid generated ultra plan` として fail-fast する。
- deterministic fallback を planner success として扱わない。

## 021-1 での移植方針

MVP では既存 UltraPlan 保存形式が YAML であるため、021-1 では JSON parser へ移行しない。戻す対象は format ではなく、以下の生成意味論である。

- system prompt / user prompt
- profile generation rules
- invalid output retry
- tool-call rejection
- generated metadata normalization
- retry exhaustion fail-fast
- planner failure diagnostics

UltraPlan YAML schema に `degraded` のような新 field は追加しない。degraded / retry / fail-fast は eval event と stderr で表現する。

## test0628 系 UAT との関係

`test0628_001` / `test0628_002` では、保存された UltraPlan が `scaffold / implement / verify` の薄い3 phase になっていた。これは `UltraPlan::deterministic` の形と一致する。

021-1 の期待は、planner が invalid な場合にこの fallback を通常成功 plan として保存・実行せず、workspace mutation 前に planner failure として止めることである。

## 受入基準

- UltraPlan generation が system + user messages を使う。
- Next.js profile rules が system prompt に入る。
- invalid output / tool call は retry される。
- retry exhaustion 後に `.anvil/plans/ultra-plan-*.yaml` が作成されない。
- generated metadata の echo 揺れは request context で正規化され、good phase plan を過剰拒否しない。
- deterministic fallback は通常成功 path で使われない。
