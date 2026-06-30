# 022-6 TaskContract-lite / Obligation Tracking Implementation Result

作成日: 2026-06-30

## 対象

`workspace/mvp/eval/022/README.md` の 022-6 に従い、MVP `anvilminimal` に TaskContract-lite 相当の obligation tracking と scaffold/style/docs-only false positive 抑制を追加した。

## 参照した source

- `/Users/maenokota/share/work/github_kewton/Anvil-develop/src/agent/loop_run/task_contract.rs`
- `/Users/maenokota/share/work/github_kewton/Anvil-develop/src/agent/loop_run/task_contract_core.rs`
- `/Users/maenokota/share/work/github_kewton/Anvil-develop/src/agent/loop_run/task_contract_completion_policy.rs`
- `/Users/maenokota/share/work/github_kewton/Anvil-develop/src/agent/loop_run/task_contract_artifact_contract.rs`
- `/Users/maenokota/share/work/github_kewton/Anvil-develop/src/agent/loop_run/task_contract_deliverable_lifecycle.rs`
- `/Users/maenokota/share/work/github_kewton/Anvil-develop/src/agent/loop_run/evidence_binding.rs`
- `/Users/maenokota/share/work/github_kewton/Anvil-develop/src/agent/loop_run/scaffold_pipeline.rs`

## 実装内容

- `CompletionContract` に `required_obligations` を追加し、`setup` / `scaffold` / `implementation` / `verification` / `acceptance_evidence` に限定した。
- `RuntimeAcceptanceReport` に `missing_obligations` を追加し、capability/evidence と独立して role-level obligation を判定できるようにした。
- `verify_runtime_acceptance` が expected artifact と capability evidence だけでなく、artifact role が required obligation を満たしているか確認するようにした。
- plan-run / ultra-plan-run の final contract で、Next.js や app/game 系 goal から `implementation` obligation を推定し、外部 completion contract がない場合でも setup-only / docs-only / scaffold-only を成功扱いしないようにした。
- `completion_verify` / `plan_final_contract` / `ultra_final_acceptance` event に required/missing obligations を出すようにした。

## 追加した fixture / test

- setup-only output は `implementation` obligation を満たさない。
- scaffold-only output は implementation artifact / capability evidence を満たさない。
- style-only output は implementation obligation を満たさない。
- docs-only output は app/game task の acceptance を満たさない。
- required capability と artifact evidence の対応が `RuntimeAcceptanceReport.artifact_obligations` に残る。
- plan-run の Next.js game fixture で setup-only / scaffold-only / docs-only が final contract failure になる。

## Gate 更新

- `G-S01 request understanding` は `partial` のまま、MVP 側に inferred obligation gate が追加された。
- `G-S02 task contract` は `partial` のまま、TaskContract-lite role separation の fixture が追加された。
- `G-S11 scaffold fallback` は `partial` のまま、scaffold-only success 抑制を plan-run final contract に接続した。
- ただし source/MVP same-condition trace diff と manual UAT evidence は未取得のため pass にはしない。

## 検証

- `python3 -m pytest mvp/anvilminimal/tests/eval`
  - 175 passed, 1 skipped
- `cargo test --manifest-path mvp/anvilminimal/Cargo.toml`
  - 366 lib tests passed
  - integration tests passed
  - ignored live/provider/TUI pty tests は従来どおり未実行

## 残タスク

- MVP / anvildev の同条件 trace で obligation tracking と source TaskContract semantics の差分を確認する。
- manual UAT で scaffold-only / setup-only が成功表示にならず、repair/continuation target へ進むことを確認する。
- 022-7 の final acceptance / UAT-equivalent gate と合わせて、browser/interaction evidence まで release gate に接続する。
