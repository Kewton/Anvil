# 022-5 Repair Targeting / Follow-through / Recovery Handoff Implementation Result

作成日: 2026-06-30

## 対象

`workspace/mvp/eval/022/README.md` の 022-5 に従い、MVP `anvilminimal` の repair targeting / follow-through / recovery handoff を source semantics に寄せた。

## 参照した source

- `/Users/maenokota/share/work/github_kewton/Anvil-develop/src/agent/loop_run/verifier_repair_targeting.rs`
- `/Users/maenokota/share/work/github_kewton/Anvil-develop/src/agent/loop_run/repair_lifecycle.rs`
- `/Users/maenokota/share/work/github_kewton/Anvil-develop/src/agent/loop_run/verifier_driver.rs`
- `/Users/maenokota/share/work/github_kewton/Anvil-develop/src/agent/minimal_step_runner/repair.rs`

## 実装内容

- `RepairFollowThrough` を追加し、repair turn を `no_change` / `target_matched` / `target_misdirected` に分類するようにした。
- 変更ゼロの repair を target followed と見なす挙動を廃止した。
- `DependencySetup` target が manifest / lockfile / setup artifact の変更を受け入れるようにした。
- `interactive_ui_source_evidence` / `non_static_screen_evidence` 欠落を、単なる evidence ではなく capability repair target として扱うようにした。
- plan-run step repair で follow-through を event に出し、連続 no-change / misdirected は bounded failure として `loop_stop` に分類するようにした。
- bounded repair exhausted 後の recovery prompt 保存は failure handoff として維持し、`recovery_prompt_saved.failure_kind` を出すようにした。
- eval failure classifier が `step_verify_repair.failure_kind` から no-change / misdirected を分類できるようにした。

## 追加した fixture / test

- missing entrypoint repair が `src/app/page.tsx` を作成すると `target_matched` になる。
- no-change repair が `verify_repair_no_change` として分類され、`.anvil/repairs/repair-*.md` が保存される。
- target not followed repair が `repair_target_misdirected` として分類され、handoff が保存される。
- eval classifier が `step_verify_repair.failure_kind=repair_target_misdirected` を分類できる。

## Gate 更新

- `G-S10 repair targeting` を `fail` から `partial` に変更した。
- 理由: fixture と event taxonomy は追加したが、同条件 source/MVP trace diff と targeted eval は未再計測のため pass にはしない。
- `G-S12 final acceptance` は引き続き fail。

## 残タスク

- MVP / anvildev の同条件 trace で repair lifecycle stage 差分を確認する。
- targeted eval で repair follow-through 改善が plan-run / ultra-plan-run の runtime bridge に悪影響を出していないか確認する。
- TUI/manual UAT で recovery handoff の表示と `.anvil/repairs` 保存を確認する。

