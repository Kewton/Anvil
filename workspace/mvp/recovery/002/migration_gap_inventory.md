# Recovery 002 Migration Gap Inventory

作成日: 2026-07-01

## スコープ

対象は、RECOVERY-001-A〜H 実施後に `test0701_004` UAT で残った移植漏れ・意味論ズレ・acceptance bridge 不足である。

この inventory は、Space Invaders 固有の改善リストではない。対象は以下の一般化した runtime semantics である。

- Next.js interactive app/game の final acceptance。
- build / dev route / browser readiness の関係。
- dev server start / readiness wait / route probe / cleanup の関係。
- static capability evidence と実挙動 evidence の関係。
- CompletionContract / external contract と通常 TUI/ultra-run の接続。
- release gate partial/fail から repair / recovery handoff への接続。
- verifier policy と runtime tool policy の一貫性。
- planner verify normalization / retry / quality warning の診断。
- TUI/manual run での completion 表示と partial/failure diagnostics。

## 現象サマリ

`test0701_004` では以下を確認した。

| 項目 | 結果 |
| --- | --- |
| ultra phase | 5/5 completed |
| `npm run build` | pass |
| `npm run dev -p 3011` + `/` | `HTTP 500` |
| summary | `Final acceptance: partial`, `Release gate: partial`, 末尾 `Status: complete` |
| browser evidence | missing |
| interaction evidence | missing |
| completion contract | `completion_contract_verification_enabled=false`, `external_contract_checked=false` |
| planner diagnostics | verify command normalization / retry / quality warning observed |
| recovery `.md` / recovery UltraPlan YAML | not created |
| generated game | Canvas/source hints はあるが、勝敗・被弾・敵移動・start/restart flow が不十分 |

## Status Definition

| status | 意味 |
| --- | --- |
| `open` | gap として確定。未実装または未検証。 |
| `partial` | 一部実装済みだが、UAT/release/source parity のいずれかが不足。 |
| `pass` | 受入条件、fixture/eval、必要な UAT evidence が揃っている。 |
| `intentionally_different` | source と異なるが、代替 gate と残リスクが明確。 |

## Confirmation Method And Timing

各 gap は、以下のタイミングで確認する。確認結果は `current_status` と該当 gap の本文に反映する。

| timing | 対象 | 方法 | status 更新基準 |
| --- | --- | --- | --- |
| planning | G01〜G14 全体 | source refs / MVP refs / 問題箇所 / 根本原因 / 受入条件が揃っているかを確認する | 欠けている場合は `open` のままにし、実装へ進まない |
| after implementation | 該当 Gxx | unit test / fixture / targeted pytest / cargo test を実行する | fixture が追加され、期待どおり fail/pass を区別できれば `partial` 以上 |
| after runtime eval | P0/P1 gap | MVP eval、必要に応じて anvildev 比較、events/summary inspection を実行する | failure layer / release gate / completion authority の改善が確認できれば `partial` または `pass` |
| after manual UAT | G01/G02/G03/G04/G05/G07/G08/G10/G11/G13/G14 | 新規 workspace で Next.js interactive app/game を生成し、build / dev route / browser evidence / TUI summary / recovery artifacts を確認する | release-quality の再発がなければ `pass`。browser unavailable 等で未確認なら `partial` |

`pass` にするには、少なくとも以下が必要である。

- 該当 gap の受入条件を満たす automated test または fixture。
- P0/P1 gap では、eval summary または targeted fixture。user-visible runtime / release gate に影響する gap は manual UAT または UAT 相当の release evidence。
- source と異なる場合、`intentionally_different` として代替 gate / 残リスク / 確認タイミングを明記する。
- success rate だけでなく、completion authority、release gate、recovery handoff、diagnostics のいずれが改善したかを明記する。

G10 は横断 gap として扱う。runtime summary の比較可能性は RECOVERY-002-A、comparative gate / release evidence operation は RECOVERY-002-F で確認する。

## UAT Issue Coverage Matrix

`test0701_004` の主要事象が G01〜G14 で漏れなく扱われているかを、以下で確認する。

| UAT issue | covered by | required evidence |
| --- | --- | --- |
| `Status: complete` だが `Release gate: partial` | G01, G07, G10 | summary / events で command completion と release completion が分離される |
| browser readiness evidence missing | G02, G11 | dev server lifecycle event と browser readiness status |
| 手動 dev route が HTTP 500 | G02, G03, G04, G11 | HTTP 500 が failure kind と recovery target に入る |
| recovery `.md` / recovery UltraPlan YAML がない | G03, G08 | release gate partial/fail 時の recovery artifacts |
| static capability evidence は pass だがゲーム挙動が浅い | G05, G13, G14 | static evidence と runtime/browser/contract evidence の分離 |
| runtime Bash が verifier policy を迂回し得る | G06, G12 | runtime tool policy event と deterministic verifier evidence の分離 |
| planner verify normalization / quality warning が見えにくい | G12 | normalization/retry/warning counts in summary/eval |
| CompletionContract が通常 TUI/ultra-run に bind されない | G13 | `completion_contract_verification_enabled=true`, `external_contract_checked=true` |
| RECOVERY-001 は通ったが UAT で再発 | G09, G10 | recovery item status と UAT/release evidence の連動 |

この matrix に未対応の UAT issue が追加で見つかった場合、既存 Gxx に追記するか、新しい Gxx を追加してから実装へ進む。

## Gap Summary

| id | priority | category | title | current_status |
| --- | --- | --- | --- | --- |
| G01 | P0 | `semantic_drift` | release gate partial が command completion / success semantics に接続されない | partial |
| G02 | P0 | `new_release_gate_gap` | browser readiness を通常 final acceptance で実行・取得していない | partial |
| G03 | P0 | `acceptance_bridge_gap` | browser/dev route failure が repair target / recovery UltraPlan に接続されない | partial |
| G04 | P0 | `semantic_drift` | Next.js/Tailwind dev pipeline が build verifier lifecycle に含まれていない | partial |
| G05 | P1 | `acceptance_bridge_gap` | static capability evidence が interactive behavior を過大評価する | partial |
| G06 | P1 | `semantic_drift` | verifier command policy と runtime Bash policy が一致しない | partial |
| G07 | P1 | `diagnostic_gap` | TUI/summary が partial artifact を完成品に見せる | partial |
| G08 | P1 | `migration_missing` | source の repair handoff semantics が release gate partial/fail に横展開されていない | partial |
| G09 | P2 | `diagnostic_gap` | RECOVERY-001 の pass 判定が UAT/release evidence に十分連動していない | partial |
| G10 | P2 | `acceptance_bridge_gap` | source/MVP 比較が aggregate success に寄り、completion authority 差分を見落とす | partial |
| G11 | P0 | `new_release_gate_gap` | browser readiness の dev server lifecycle が controller にない | partial |
| G12 | P1 | `diagnostic_gap` | planner verify normalization / quality warning が不安定性として gate に残らない | partial |
| G13 | P0 | `acceptance_bridge_gap` | CompletionContract / external contract が通常 TUI ultra-run に bind されていない | partial |
| G14 | P2 | `semantic_drift` | step kind の role authority が弱く、setup/verify/report と implementation の責務境界が曖昧 | partial |

---

## G01: release gate partial が command completion / success semantics に接続されない

| 項目 | 内容 |
| --- | --- |
| priority | P0 |
| category | `semantic_drift` |
| current_status | partial |

### 現象

`test0701_004` の summary は `Final acceptance: partial` / `Release gate: partial` を出しているが、末尾は `Status: complete` である。

### source refs

- `/Users/maenokota/share/work/github_kewton/Anvil-develop/src/agent/loop_run/task_contract_completion_policy.rs`
- `/Users/maenokota/share/work/github_kewton/Anvil-develop/src/agent/loop_run/task_contract.rs`
- `/Users/maenokota/share/work/github_kewton/Anvil-develop/src/agent/loop_run/actor_loop_flow.rs`
- `/Users/maenokota/share/work/github_kewton/Anvil-develop/src/agent/loop_run/summary.rs`

### MVP refs

- `/Users/maenokota/share/work/github_kewton/Anvil-develop/mvp/anvilminimal/src/planner/runner.rs`
- `/Users/maenokota/share/work/github_kewton/Anvil-develop/mvp/anvilminimal/src/minimal_loop/evidence.rs`
- `/Users/maenokota/share/work/github_kewton/Anvil-develop/mvp/anvilminimal/src/eval_events.rs`

### 問題箇所

MVP の `ultra_final_acceptance_report` は、`release_gate.status == "failed"` のときだけ `VerificationReport` に failure を追加する。`partial` は event/summary に残るが、command stop は `completed` のままになり得る。

### 移植元との比較

source は TaskContract / CompletionPolicy の completion authority を通し、deterministic evidence が completion に足りない場合は completion へ進めない。MVP は release gate を追加したが、その `partial` を completion authority に接続していない。

### 根本原因

MVP では `phase completed`、`runtime_acceptance_passed`、`release_gate_status` が別々の信号になっている。source 側の「completion authority を満たさない限り完了としない」意味論が、MVP の release gate に写像されていない。

### 受入条件

- interactive Next.js task で `release_gate_status=partial` の場合、TUI/summary は `complete` 単独表示にしない。
- `partial` は `completed_with_release_gate_partial` などの明示 state として event/summary に出る。
- `partial` が recovery handoff 対象か、ユーザー確認対象か、明示的に分類される。
- eval summary でも process success と release success を分ける。

### RECOVERY-002-A 実装メモ / evidence

- `mvp/anvilminimal/src/eval_events.rs` に completion projection を追加し、command completion と runtime/final acceptance/release gate/next action を別フィールドへ投影するようにした。
- `mvp/anvilminimal/src/lib.rs` と `mvp/anvilminimal/src/tui/slash.rs` の `run_stop` / `tui_command_stop` event に `completion_status`, `command_completion_state`, `runtime_acceptance_status`, `final_acceptance_status`, `release_gate_status`, `release_quality_completion`, `next_action` を追加した。
- `release_gate_status=partial` かつ command/process が成功した場合、summary/TUI は `Status: complete_with_partial_release_gate` を出す。`complete` 単独表示にはしない。
- `mvp/anvilminimal/scripts/eval_lib/run_summary.py` と `mvp/anvilminimal/scripts/eval-run.py` に completion/release gate 列を追加し、eval summary でも process success と release gate status が分かれるようにした。
- 検証: `cargo test --manifest-path mvp/anvilminimal/Cargo.toml` pass。
- 検証: `pytest mvp/anvilminimal/tests/eval` pass。
- 残: release gate partial/fail から recovery handoff artifact を作る部分は G03/G08 の対象。manual UAT での再確認も未実施のため `partial`。

---

## G02: browser readiness を通常 final acceptance で実行・取得していない

| 項目 | 内容 |
| --- | --- |
| priority | P0 |
| category | `new_release_gate_gap` |
| current_status | partial |

### 現象

summary では browser readiness evidence missing。手動で `npm run dev -p 3011` と `curl /` を実行すると `HTTP 500` だった。

### source refs

- `/Users/maenokota/share/work/github_kewton/Anvil-develop/src/agent/loop_run/project_probe.rs`
- `/Users/maenokota/share/work/github_kewton/Anvil-develop/src/agent/loop_run/project_verifier.rs`
- `/Users/maenokota/share/work/github_kewton/Anvil-develop/src/agent/loop_run/verifier_driver.rs`
- `/Users/maenokota/share/work/github_kewton/Anvil-develop/src/agent/loop_run/auto_test.rs`

### MVP refs

- `/Users/maenokota/share/work/github_kewton/Anvil-develop/mvp/anvilminimal/src/planner/runner.rs`
- `/Users/maenokota/share/work/github_kewton/Anvil-develop/mvp/anvilminimal/scripts/eval_lib/browser_oracle.py`
- `/Users/maenokota/share/work/github_kewton/Anvil-develop/mvp/anvilminimal/scripts/eval_lib/parity_gate.py`

### 問題箇所

MVP の `browser_release_gate` は既存 evidence JSON を読むだけで、通常 TUI 実行時に dev server / route / browser probe を実行しない。したがって internal run は `unavailable:browser_readiness_evidence_missing` で partial になり、実際の HTTP 500 は自動検知されない。

### 移植元との比較

source に常時 browser readiness gate があるわけではない。そのためこれは純粋な source 関数の移植漏れではない。ただし source は verifier authority / project probe によって、completion evidence を controller flow に組み込む。MVP は browser readiness を release-grade evidence として導入した以上、それを通常 final acceptance に取り込まないと source の completion authority 代替にならない。

### 根本原因

browser readiness が eval/release artifact として後付けされ、runtime controller の final acceptance action になっていない。

### 受入条件

- interactive Next.js task では、少なくとも release/UAT mode で dev route readiness probe が通常 final acceptance に接続される。
- browser unavailable / permission denied / port bind failure / HTTP 4xx/5xx を分けて分類する。
- browser unavailable は partial、HTTP 500 は fail とする。
- probe が実行できない環境でも full success にはしない。

### RECOVERY-002-B evidence

- `mvp/anvilminimal/src/planner/runner.rs` の `browser_release_gate` が browser readiness evidence missing 時に Next.js dev route probe を生成し、`browser-readiness.json` と `dev_server_lifecycle` events を final acceptance/release gate に接続するようになった。
- `status=unavailable` は `ok=false` より先に partial/unavailable として分類し、HTTP 500 は fail として分類する。`browser_unavailable:*`、`port_in_use`、`bind_denied`、`startup_timeout`、`http_500`、`tailwind_dev_pipeline_failure` を混同しない。
- Automated evidence: `cargo test --manifest-path mvp/anvilminimal/Cargo.toml`、`pytest mvp/anvilminimal/tests/eval` pass。
- Targeted tests/fixtures: `planner::runner::tests::nextjs_dev_route_probe_disabled_records_lifecycle_stages`、`planner::runner::tests::plan_run_nextjs_tailwind_dev_route_failure_keeps_failure_kind`、`mvp/anvilminimal/tests/eval/fixtures/uat_002/test0701_004_nextjs_dev_route_failure.json`。
- Manual UAT は未実施のため `pass` ではなく `partial`。

---

## G03: browser/dev route failure が repair target / recovery UltraPlan に接続されない

| 項目 | 内容 |
| --- | --- |
| priority | P0 |
| category | `acceptance_bridge_gap` |
| current_status | partial |

### 現象

手動 browser readiness では HTTP 500 だが、run 中には recovery `.md` / recovery UltraPlan YAML が保存されていない。

### source refs

- `/Users/maenokota/share/work/github_kewton/Anvil-develop/src/agent/minimal_step_runner/repair.rs`
- `/Users/maenokota/share/work/github_kewton/Anvil-develop/src/agent/loop_run/verifier_repair_targeting.rs`
- `/Users/maenokota/share/work/github_kewton/Anvil-develop/src/agent/loop_run/repair_lifecycle.rs`
- `/Users/maenokota/share/work/github_kewton/Anvil-develop/src/agent/loop_run/verifier_driver.rs`

### MVP refs

- `/Users/maenokota/share/work/github_kewton/Anvil-develop/mvp/anvilminimal/src/planner/repair.rs`
- `/Users/maenokota/share/work/github_kewton/Anvil-develop/mvp/anvilminimal/src/minimal_loop/repair_target.rs`
- `/Users/maenokota/share/work/github_kewton/Anvil-develop/mvp/anvilminimal/src/planner/runner.rs`

### 問題箇所

MVP の recovery handoff は bounded repair exhaustion や phase failure を中心に発火する。release gate partial、browser readiness missing、外部 UAT で発覚した browser failure は repair target / recovery YAML 生成へ接続されていない。

### 移植元との比較

source の `build_repair_exhausted_report` は、repair exhaustion 時に次の `/ultra-plan-run` 用 prompt を保存し、明示的な command を提示する。MVP はこの機構を step/phase failure には移し始めたが、final acceptance / release gate partial/fail には横展開できていない。

### 根本原因

`verify/phase failure` と `release acceptance failure` が別レイヤとして扱われ、後者が recovery lifecycle に入っていない。

### 受入条件

- browser readiness failed / missing required release evidence は `RepairTarget` または `RecoveryHandoff` に分類される。
- recovery `.md` と recovery UltraPlan YAML が保存される。
- suggested command が TUI/summary に出る。
- recovery YAML は original goal、failure evidence、failed acceptance layer、repair target、preferred verify/browser check を含む。
- recovery artifact の保存だけで success にはしない。

### RECOVERY-002-C evidence

- `mvp/anvilminimal/src/planner/runner.rs` に release acceptance 用の `save_release_recovery_handoff` を追加し、release gate partial/fail、browser readiness missing/fail、plan final contract failure を `.anvil/repairs/repair-*.md` と `.anvil/plans/recovery-ultra-plan-*.yaml` 保存へ接続した。
- `recovery_prompt_saved` event は `release_acceptance_handoff=true`、`handoff_saved_not_success=true`、`acceptance_layer`、`recovery_prompt_path`、`recovery_ultra_plan_path`、`suggested_recovery_command`、`suggested_recovery_yaml_command` を出す。保存は handoff であり、final/release success へ丸めない。
- `mvp/anvilminimal/src/planner/repair.rs` の recovery UltraPlan 生成は `render_ultra_plan` / `parse_ultra_plan` roundtrip を維持し、original goal、failure evidence、`Failed acceptance layer or phase`、repair target、`Preferred verify/browser check` を prompt に含める。
- `mvp/anvilminimal/src/minimal_loop/repair_target.rs` は `browser_readiness_evidence_missing` / `interaction_evidence_missing` を required evidence、HTTP 500/browser failure を test/evidence、Tailwind dev pipeline failure を framework config repair target に分類する。
- Targeted tests: `planner::runner::tests::plan_run_nextjs_interactive_app_records_partial_release_gate`、`planner::runner::tests::plan_run_nextjs_browser_http_500_fails_final_contract`、`planner::runner::tests::ultra_final_acceptance_repair_failure_saves_recovery_handoff`、`minimal_loop::repair_target::tests::release_gate_missing_browser_evidence_targets_required_evidence`、`minimal_loop::repair_target::tests::tailwind_dev_route_failure_targets_framework_config`。
- Automated evidence: `cargo test --manifest-path mvp/anvilminimal/Cargo.toml` pass、`pytest mvp/anvilminimal/tests/eval` pass。
- Manual UAT は未実施のため `pass` ではなく `partial`。

---

## G04: Next.js/Tailwind dev pipeline が build verifier lifecycle に含まれていない

| 項目 | 内容 |
| --- | --- |
| priority | P0 |
| category | `semantic_drift` |
| current_status | partial |

### 現象

`npm run build` は成功するが、dev route は `@tailwind` parse error で HTTP 500 になる。

### source refs

- `/Users/maenokota/share/work/github_kewton/Anvil-develop/src/agent/minimal_step_runner/profiles/nextjs.rs`
- `/Users/maenokota/share/work/github_kewton/Anvil-develop/src/agent/loop_run/project_probe.rs`
- `/Users/maenokota/share/work/github_kewton/Anvil-develop/src/agent/loop_run/project_verifier.rs`
- `/Users/maenokota/share/work/github_kewton/Anvil-develop/src/agent/loop_run/verifier_driver.rs`

### MVP refs

- `/Users/maenokota/share/work/github_kewton/Anvil-develop/mvp/anvilminimal/src/planner/profiles/nextjs.rs`
- `/Users/maenokota/share/work/github_kewton/Anvil-develop/mvp/anvilminimal/src/minimal_loop/build_verifier.rs`
- `/Users/maenokota/share/work/github_kewton/Anvil-develop/mvp/anvilminimal/src/minimal_loop/dependency_setup.rs`

### 問題箇所

MVP の Next.js profile verifier は Tailwind dependency/config/import の静的整合性を確認するが、dev route で CSS pipeline が実際に動くかは確認しない。

### 移植元との比較

source profile も Tailwind dependency/config を static に検証する。よって dev route probe は source の単純移植漏れとは言い切れない。しかし source の verifier lifecycle は failed verifier output を repair target に戻す設計であり、MVP では `build pass / dev fail` が verifier lifecycle にまだ入っていない。

### 根本原因

Next.js の production build と dev server route の差を acceptance に組み込む gate が不足している。Tailwind dev pipeline は static contract だけで足りない。

### 受入条件

- `@tailwind` directive がある Next.js app で、build pass / dev route fail を再現する fixture がある。
- failure kind は `browser_readiness_failed:http_500` または `tailwind_dev_pipeline_failure` 等に具体化される。
- Tailwind を使わない plain CSS app は不要な Tailwind toolchain を要求されない。
- failure は recovery target に戻る。

### RECOVERY-002-B evidence

- `mvp/anvilminimal/src/planner/runner.rs` の dev route probe は `@tailwind` + `Module parse failed` / `Unexpected character` / PostCSS/Tailwind signature を `tailwind_dev_pipeline_failure` として HTTP 500 から分離する。
- `mvp/anvilminimal/scripts/eval_lib/browser_oracle.py` と `parity_gate.py` は HTTP 500 の既存 `browser_http_500` を維持しつつ、明示的な `tailwind_dev_pipeline_failure` を優先する。
- `mvp/anvilminimal/src/planner/profiles/nextjs.rs` の既存 test `nextjs_allows_plain_css_without_tailwind_toolchain` により plain CSS app へ Tailwind toolchain を強制しない挙動は維持されている。
- Added fixture: `mvp/anvilminimal/tests/eval/fixtures/uat_002/test0701_004_nextjs_dev_route_failure.json` が build pass / dev route HTTP 500 / release gate failed を固定する。
- Automated evidence: `cargo test --manifest-path mvp/anvilminimal/Cargo.toml`、`pytest mvp/anvilminimal/tests/eval` pass。
- release failure を recovery target に戻す handoff は RECOVERY-002-C で G03/G08 に接続済み。manual UAT は未実施のため G04 は引き続き `partial`。

---

## G05: static capability evidence が interactive behavior を過大評価する

| 項目 | 内容 |
| --- | --- |
| priority | P1 |
| category | `acceptance_bridge_gap` |
| current_status | partial |

### 現象

events では `missing_capabilities=[]` / `missing_evidence=[]` だが、実装には敵移動、被弾、ライフ、勝敗遷移、start/restart flow、LocalStorage high score などが不足している。

### source refs

- `/Users/maenokota/share/work/github_kewton/Anvil-develop/src/agent/loop_run/task_contract.rs`
- `/Users/maenokota/share/work/github_kewton/Anvil-develop/src/agent/loop_run/task_contract_completion_policy.rs`
- `/Users/maenokota/share/work/github_kewton/Anvil-develop/src/agent/loop_run/task_contract_artifact_contract.rs`
- `/Users/maenokota/share/work/github_kewton/Anvil-develop/src/agent/loop_run/evidence_binding.rs`
- `/Users/maenokota/share/work/github_kewton/Anvil-develop/src/agent/loop_run/task_contract_deliverable_lifecycle.rs`

### MVP refs

- `/Users/maenokota/share/work/github_kewton/Anvil-develop/mvp/anvilminimal/src/minimal_loop/evidence.rs`
- `/Users/maenokota/share/work/github_kewton/Anvil-develop/mvp/anvilminimal/src/minimal_loop/completion.rs`
- `/Users/maenokota/share/work/github_kewton/Anvil-develop/mvp/anvilminimal/src/planner/runner.rs`

### 問題箇所

MVP の source evidence は `useState`、`requestAnimationFrame`、`keydown`、`enemy`、`score`、`gameover` などの lexical/structural hint で capability を満たしやすい。到達可能な状態遷移やブラウザ上の操作結果までは確認していない。

### 移植元との比較

source は full TaskContract graph により、artifact role、required behavior、completion evidence、verifier authority を分離する。MVP は TaskContract-lite として capability evidence を作ったが、behavior coverage / artifact lifecycle / runtime interaction の代替としてはまだ弱い。

### 根本原因

keyword/static evidence を completion authority として扱っており、interactive behavior の「到達可能性」「ユーザー操作による状態変化」「失敗/勝利条件」を証拠にできていない。

### 受入条件

- title-only / style-only / keyword-only / unreachable-state game は final acceptance failure になる。
- interactive game task では、state transition evidence、input-to-state-change evidence、challenge progression evidence、failure/win condition evidence を要求する。
- browser/interaction evidence がない場合は full pass にしない。
- static source evidence は `partial` までで、release-grade pass は browser/interaction evidence を必要とする。

### RECOVERY-002-D status / evidence

- status: `partial`
- implemented:
  - `mvp/anvilminimal/src/minimal_loop/evidence.rs` の `failure_or_collision_evidence` を lexical token だけでは満たさず、`setGameState("gameover")` 等の failure transition または `setLives` / `setHealth` 等の damage mutation を要求するように変更。
  - `restart_or_recoverable_state_evidence` も start/restart/reset token に加えて、実際の state transition / reset function / dispatch と user input handler を要求するように変更。
  - `capability_evidence_bindings` から scaffold path fallback を外し、static/scaffold evidence を implementation capability binding として扱わないように変更。
- automated evidence:
  - `cargo test --manifest-path mvp/anvilminimal/Cargo.toml minimal_loop::evidence::tests::unreachable_game_state_literals_do_not_satisfy_release_grade_game_evidence`
  - `cargo test --manifest-path mvp/anvilminimal/Cargo.toml minimal_loop::evidence::tests::interactive_game_source_satisfies_generic_capability_evidence`
  - `cargo test --manifest-path mvp/anvilminimal/Cargo.toml`
  - `pytest mvp/anvilminimal/tests/eval`
- residual risk:
  - browser/interaction evidence による release-grade full pass の最終確認は G02/G11 と RECOVERY-002-F の comparative release evidence で継続確認する。

---

## G06: verifier command policy と runtime Bash policy が一致しない

| 項目 | 内容 |
| --- | --- |
| priority | P1 |
| category | `semantic_drift` |
| current_status | partial |

### 現象

planner verify では shell control syntax が拒否される一方、runtime Bash tool call では `grep -q 3011 package.json && echo "Port 3011 configured"` が実行され `ok` になっている。

### source refs

- `/Users/maenokota/share/work/github_kewton/Anvil-develop/src/agent/loop_run/verifier_command_policy.rs`
- `/Users/maenokota/share/work/github_kewton/Anvil-develop/src/tools/bash.rs`
- `/Users/maenokota/share/work/github_kewton/Anvil-develop/src/agent/loop_run/tool_policy.rs`

### MVP refs

- `/Users/maenokota/share/work/github_kewton/Anvil-develop/mvp/anvilminimal/src/planner/verify.rs`
- `/Users/maenokota/share/work/github_kewton/Anvil-develop/mvp/anvilminimal/src/tools/bash.rs`
- `/Users/maenokota/share/work/github_kewton/Anvil-develop/mvp/anvilminimal/src/tools/registry.rs`

### 問題箇所

MVP の `tools/bash.rs` は危険コマンドや offline setup を一部 block するが、一般の shell control syntax は `sh -c` で実行される。planner verify policy と runtime Bash policy が別物になっている。

### 移植元との比較

source の verifier command admission は shell control を deterministic evidence command として拒否する。source の Bash tool 全体が全 shell control を禁止しているとは限らないが、completion/verifier evidence として認める境界は明確である。MVP は verify step / report step の Bash 実行が deterministic verification の代替として使われ得るため、policy boundary が曖昧になっている。

### 根本原因

`verify command` と `runtime Bash tool call` の意味を分離しきれていない。planner が verify policy に従っても、実行モデルが Bash で同じ検証を shell control 付きで実行できてしまう。

### 受入条件

- verify/report/final acceptance 目的の Bash tool call は deterministic verify policy と同等の shape check を受ける。
- implementation/setup 目的の Bash は別 policy として分類され、success が verifier evidence に流入しない。
- `&&`, `||`, `|`, `;`, redirection, command substitution は deterministic verification evidence として拒否される。
- unsafe runtime Bash は `tool_policy_error` または `verify_command_policy_error` として event に残る。

### RECOVERY-002-E status / evidence

- `mvp/anvilminimal/src/minimal_loop/loop_run.rs` で plan step kind ごとの Bash purpose を分類し、Verify/Report Bash は `planner::verify::diagnose_verify_command` と同じ deterministic verifier policy を通す。
- Verify/Report Bash が `&&`, `||`, `|`, `;`, redirection, command substitution などを含む場合は実行前に `runtime_bash_policy`、`tool_policy_error`、`tool_validation_error(error_kind=verify_command_policy_error)` を emit し、deterministic verifier evidence にはしない。
- Setup/implementation/inspect 目的の Bash は `runtime_setup` / `runtime_implementation` / `runtime_inspection` として event に残し、`deterministic_verifier_evidence=false` のまま通常作業を許可する。
- fixture: `verify_step_bash_shell_control_is_policy_error_not_evidence`、`setup_step_bash_shell_control_is_runtime_setup_not_verifier_evidence`。

---

## G07: TUI/summary が partial artifact を完成品に見せる

| 項目 | 内容 |
| --- | --- |
| priority | P1 |
| category | `diagnostic_gap` |
| current_status | partial |

### 現象

summary は途中で `Final acceptance: partial` を2回出しつつ、最後に `Status: complete` を出す。ユーザー視点では「完了したが成果物が微妙」と見える。

### source refs

- `/Users/maenokota/share/work/github_kewton/Anvil-develop/src/agent/loop_run/summary.rs`
- `/Users/maenokota/share/work/github_kewton/Anvil-develop/src/agent/loop_run/safe_stop_emit.rs`
- `/Users/maenokota/share/work/github_kewton/Anvil-develop/src/agent/loop_run/actor_loop_flow.rs`

### MVP refs

- `/Users/maenokota/share/work/github_kewton/Anvil-develop/mvp/anvilminimal/src/eval_events.rs`
- `/Users/maenokota/share/work/github_kewton/Anvil-develop/mvp/anvilminimal/src/planner/runner.rs`
- `/Users/maenokota/share/work/github_kewton/Anvil-develop/mvp/anvilminimal/src/tui`

### 問題箇所

TUI/summary が lifecycle layer を区別しない。phase completion、runtime acceptance、release gate、manual UAT readiness が別々の状態であることがユーザーに十分伝わらない。

### 移植元との比較

source は failure / safe stop / artifact completion exhausted を summary と event に出す設計がある。MVP は event は増えたが、TUI表示と summary status の最終状態が release gate partial を反映できていない。

### 根本原因

diagnostics は event にはあるが、user-facing command state へ集約するルールが弱い。

### 受入条件

- summary の最終 status は `complete`, `complete_with_partial_release_gate`, `incomplete`, `failed` などを区別する。
- partial/fail の場合、completed phases、failed gate、missing evidence、next action が表示される。
- TUI の最後にも同じ情報が見える。
- duplicate `Final acceptance` block を整理し、最新/最終判定が明確になる。

### RECOVERY-002-A 実装メモ / evidence

- 古い `Final acceptance` summary 追記を止め、`tui_command_stop` / `run_stop` の completion summary に集約した。summary は `Command completion`, `Runtime acceptance`, `Final acceptance`, `Release gate`, `Next action` を分けて表示する。
- `mvp/anvilminimal/tests/tui_integration.rs` に `tui_slash_success_with_partial_release_gate_is_not_complete_only` を追加し、TUI command 成功時でも release gate partial なら `complete_with_partial_release_gate` になることを固定した。
- `mvp/anvilminimal/src/lib.rs` に `run_lifecycle_does_not_mask_partial_release_gate_as_complete` と `run_lifecycle_does_not_mask_browser_http_500_release_failure_as_complete` を追加し、`build pass / browser HTTP 500 / Status complete` 相当の再発を防ぐ fixture を追加した。
- 検証: `cargo test --manifest-path mvp/anvilminimal/Cargo.toml` pass。
- 検証: `pytest mvp/anvilminimal/tests/eval` pass。
- 残: completed phases と failed gate を同じ summary block に出すことは phase failure handoff 側と統合余地がある。manual UAT で端末表示をまだ確認していないため `partial`。

---

## G08: source の repair handoff semantics が release gate partial/fail に横展開されていない

| 項目 | 内容 |
| --- | --- |
| priority | P1 |
| category | `migration_missing` |
| current_status | partial |

### 現象

phase failure では recovery artifacts が保存されるようになったが、release gate partial / browser readiness missing/fail では保存されない。

### source refs

- `/Users/maenokota/share/work/github_kewton/Anvil-develop/src/agent/minimal_step_runner/repair.rs`
- `/Users/maenokota/share/work/github_kewton/Anvil-develop/src/agent/loop_run/repair_lifecycle.rs`
- `/Users/maenokota/share/work/github_kewton/Anvil-develop/src/agent/loop_run/verifier_repair_targeting.rs`
- `/Users/maenokota/share/work/github_kewton/Anvil-develop/src/agent/loop_run/verifier_driver.rs`

### MVP refs

- `/Users/maenokota/share/work/github_kewton/Anvil-develop/mvp/anvilminimal/src/planner/repair.rs`
- `/Users/maenokota/share/work/github_kewton/Anvil-develop/mvp/anvilminimal/src/planner/runner.rs`

### 問題箇所

RecoveryHandoff の trigger が step/phase/verify failure に偏っている。final acceptance partial/fail を local repair exhausted と同等の handoff packet に変換していない。

### 移植元との比較

source の repair exhausted report は、失敗を次の explicit `/ultra-plan-run` に渡すための情報パケットである。MVP はこの移植を一部実施したが、final acceptance layer に適用していない。

### 根本原因

「実行中に失敗した場合」と「最終受入で失敗/partial の場合」を別物として扱っている。source semantics では、どちらも次の修復行動へつなぐ diagnostics/recovery の対象である。

### 受入条件

- final acceptance partial/fail でも recovery `.md` / recovery UltraPlan YAML が保存される。
- release gate reason、browser error、missing evidence、missing behavior が recovery prompt に含まれる。
- suggested command が summary/TUI に出る。
- handoff 保存だけで success にはしない。

### RECOVERY-002-C evidence

- source の repair exhausted handoff semantics に合わせ、MVP の final/release acceptance failure でも次アクション用 artifact を保存する。release gate partial/fail は `failed_phase=release_gate` 相当の `RecoveryHandoff` として recovery prompt/YAML に変換される。
- `mvp/anvilminimal/src/eval_events.rs`、`mvp/anvilminimal/src/lib.rs`、`mvp/anvilminimal/src/tui/slash.rs` に recovery handoff fields を追加し、TUI と `.anvil/runs/<run-id>/summary.md` に recovery prompt、Recovery UltraPlan YAML、suggested command を出す。
- `mvp/anvilminimal/tests/tui_integration.rs` の partial release gate fixture は、`complete` 単独表示を避けつつ `Recovery handoff` / `Suggested YAML command` が summary に残ることを固定する。
- bounded final acceptance repair は自動再帰実行にせず、修復成功後でも release gate partial なら handoff を保存する。artifact 保存自体は `handoff_saved_not_success=true` として event に残り、release-quality success にはしない。
- Automated evidence: `cargo test --manifest-path mvp/anvilminimal/Cargo.toml` pass、`pytest mvp/anvilminimal/tests/eval` pass。
- Manual UAT は未実施のため `pass` ではなく `partial`。

---

## G09: RECOVERY-001 の pass 判定が UAT/release evidence に十分連動していない

| 項目 | 内容 |
| --- | --- |
| priority | P2 |
| category | `diagnostic_gap` |
| current_status | partial |

### 現象

RECOVERY-001-A〜H 実施後、unit/eval は通っているが manual UAT は browser HTTP 500 と partial release gate になった。

### source refs

- `/Users/maenokota/share/work/github_kewton/Anvil-develop/src/agent/loop_run/task_contract_completion_policy.rs`
- `/Users/maenokota/share/work/github_kewton/Anvil-develop/src/agent/loop_run/summary.rs`

### MVP refs

- `/Users/maenokota/share/work/github_kewton/Anvil-develop/workspace/mvp/recovery/001/README.md`
- `/Users/maenokota/share/work/github_kewton/Anvil-develop/workspace/mvp/recovery/001/migration_gap_inventory.md`
- `/Users/maenokota/share/work/github_kewton/Anvil-develop/mvp/anvilminimal/scripts/eval_lib/parity_gate.py`

### 問題箇所

RECOVERY-001 の一部項目は unit/eval fixture では pass になったが、release-level evidence の不足を阻止条件として十分に扱えていない。

### 移植元との比較

source は completion authority を runtime flow に埋め込むため、unit-level feature presence だけで完了とはしない。MVP recovery process は、実装完了と UAT/release completion をまだ分けきれていない。

### 根本原因

recovery の pass 判定が implementation-level に寄り、release-level UAT evidence を blocking condition として再評価する運用が弱い。

### 受入条件

- recovery item の status は `implementation_pass` と `release_pass` を分けて表現する。
- UAT で HTTP 500 / release gate partial が出た場合、該当 REC は自動的に partial/open へ戻る。
- pass の証跡に browser/interaction evidence content が必要な REC を明示する。

### RECOVERY-002-F status / evidence

- `mvp/anvilminimal/scripts/eval_lib/parity_gate.py` が `recovery_item_status` を出力し、`implementation_pass` と `release_pass` を分離する。release gate を明示 opt-in で実行していない場合、または UAT/browser/interaction/TUI evidence が不足・失敗している場合は `release_pass` にしない。
- `workspace/mvp/recovery/002/README.md` に REC/Gxx reopen 運用を追記した。HTTP 500、`release_gate_status=partial|failed`、`final_acceptance_status=partial|failed|incomplete`、missing recovery handoff は該当 item を `partial` または `open` に戻す根拠として扱う。
- summary TSV に `recovery_prompt_path`、`recovery_ultra_plan_path`、`recovery_artifact_presence`、`completion_authority_reason` を追加し、implementation evidence と release evidence を別々に記録できるようにした。
- fixtures: `test_comparative_report_includes_completion_authority_release_and_recovery_fields`、`test_release_report_requires_release_quality_for_recovery_release_pass`。
- Manual UAT / actual anvildev same-condition comparison は未実施のため `pass` ではなく `partial`。

---

## G10: source/MVP 比較が aggregate success に寄り、completion authority 差分を見落とす

| 項目 | 内容 |
| --- | --- |
| priority | P2 |
| category | `acceptance_bridge_gap` |
| current_status | partial |

### 現象

これまでの比較 eval では MVP が anvildev を上回ることもあったが、manual UAT では成果物品質と browser readiness が不足した。

### source refs

- `/Users/maenokota/share/work/github_kewton/Anvil-develop/src/agent/loop_run/task_contract.rs`
- `/Users/maenokota/share/work/github_kewton/Anvil-develop/src/agent/loop_run/verifier_driver.rs`
- `/Users/maenokota/share/work/github_kewton/Anvil-develop/src/agent/loop_run/repair_lifecycle.rs`

### MVP refs

- `/Users/maenokota/share/work/github_kewton/Anvil-develop/mvp/anvilminimal/scripts/eval-run.py`
- `/Users/maenokota/share/work/github_kewton/Anvil-develop/mvp/anvilminimal/scripts/eval_lib/report.py`
- `/Users/maenokota/share/work/github_kewton/Anvil-develop/mvp/anvilminimal/scripts/eval_lib/runtime_scoring.py`
- `/Users/maenokota/share/work/github_kewton/Anvil-develop/mvp/anvilminimal/scripts/eval_lib/parity_gate.py`

### 問題箇所

aggregate success、phase completion、static acceptance が高くても、completion authority / browser readiness / user-visible final state の差分が埋もれる。

### 移植元との比較

source の強さは単一 score ではなく、task contract -> evidence -> verifier -> repair -> safe stop の連鎖にある。MVP eval はこれを一部スコア化しているが、manual UAT の `complete but unusable` を十分に予測できていない。

### 根本原因

source/MVP comparison が「成功率」と「plan/phase score」に寄り、completion authority の失敗を gate-level diff として強く扱っていない。

### 受入条件

- MVP vs anvildev 比較に、少なくとも以下を入れる。
  - command completion status
  - final acceptance status
  - release gate status
  - browser readiness status
  - recovery artifact presence
  - failure layer

### RECOVERY-002-A 実装メモ / evidence

- `mvp/anvilminimal/scripts/eval_lib/run_summary.py` の TSV schema を `eval-summary-v4-completion-authority` に更新し、`command_completion_state`, `final_acceptance_status`, `runtime_acceptance_status`, `release_gate_status`, `release_gate_reasons`, `browser_readiness_status`, `interaction_evidence_status`, `next_action` を追加した。
- `mvp/anvilminimal/scripts/eval-run.py` は runtime event 由来の completion state を row に反映する。`release_gate_status` は header に入ったため、比較 report/parity gate で aggregate success とは別に数えられる。
- `mvp/anvilminimal/scripts/eval_lib/runtime_scoring.py` は phase completion を維持しつつ、release gate partial/fail を finalization score と reason に反映する。
- `mvp/anvilminimal/tests/eval/test_runtime_scoring.py` に release gate partial が phase completion 100 と finalization partial を分ける fixture を追加した。
- 検証: `cargo test --manifest-path mvp/anvilminimal/Cargo.toml` pass。
- 検証: `pytest mvp/anvilminimal/tests/eval` pass。
- 残: recovery artifact presence と comparative gate 運用への組み込みは RECOVERY-002-F の対象。manual/source comparison eval は未実施のため `partial`。
  - completion authority reason
- aggregate success が高くても release gate failed/partial なら release-quality comparison は pass にしない。
- manual UAT scenario を eval の gate fixture として登録する。

### RECOVERY-002-F status / evidence

- `mvp/anvilminimal/scripts/eval_lib/parity_gate.py` の `summary_snapshot` が command completion、final acceptance、release gate、browser readiness、recovery artifact presence、failure layer、completion authority reason を集計する。
- `compare_mvp_to_anvildev` が `completion_authority_comparison` と `release_quality_status` を出力し、aggregate success が高くても release gate partial/fail や final acceptance partial/fail があれば release-quality pass に丸めない。
- `success_delta_classification` で `correct_failure_detection` / `release_quality_blocker_detected` / `simple_regression` を分け、正しい failure detection による成功率低下と単純 regression を report 上で区別する。
- comparative/release gate は `build_parity_gate_report(... gate_level="comparative"|"release")` の明示 opt-in のまま維持し、normal unit test に browser/anvildev/network/API key を要求しない。
- Actual MVP vs `anvildev --engine minimal` rerun は未実施のため、existing fixture/report operation evidence に基づく `partial`。

---

## G11: browser readiness の dev server lifecycle が controller にない

| 項目 | 内容 |
| --- | --- |
| priority | P0 |
| category | `new_release_gate_gap` |
| current_status | partial |

### 現象

`test0701_004` は browser readiness evidence が missing のまま完了した。手動で dev server を起動すると `/` は HTTP 500 だった。つまり browser readiness の「判定」以前に、dev server start / wait / route probe / shutdown / port conflict handling の lifecycle が通常 controller にない。

### source refs

- `/Users/maenokota/share/work/github_kewton/Anvil-develop/src/agent/loop_run/project_probe.rs`
- `/Users/maenokota/share/work/github_kewton/Anvil-develop/src/agent/loop_run/project_verifier.rs`
- `/Users/maenokota/share/work/github_kewton/Anvil-develop/src/agent/loop_run/verifier_driver.rs`
- `/Users/maenokota/share/work/github_kewton/Anvil-develop/src/agent/loop_run/node_request_helpers.rs`
- `/Users/maenokota/share/work/github_kewton/Anvil-develop/src/agent/loop_run/node_runner_manifest.rs`

### MVP refs

- `/Users/maenokota/share/work/github_kewton/Anvil-develop/mvp/anvilminimal/src/planner/runner.rs`
- `/Users/maenokota/share/work/github_kewton/Anvil-develop/mvp/anvilminimal/src/planner/profiles/nextjs.rs`
- `/Users/maenokota/share/work/github_kewton/Anvil-develop/mvp/anvilminimal/scripts/eval_lib/browser_oracle.py`
- `/Users/maenokota/share/work/github_kewton/Anvil-develop/mvp/anvilminimal/scripts/eval_lib/parity_gate.py`

### 問題箇所

MVP は evidence JSON が既に存在するかを読む path はあるが、通常 TUI/ultra-run が Next.js dev server を起動し、ready になるまで待ち、route を probe し、終了処理する controller action を持たない。

### 移植元との比較

source は browser readiness そのものを常時必須にはしていないが、project probe / verifier driver が workspace state と実行結果を lifecycle event として扱う。MVP は browser readiness を release gate として採用したため、その取得 lifecycle も controller semantics として持つ必要がある。

### 根本原因

browser readiness を gate scoring の入力として追加したが、入力 evidence を生成する実行 lifecycle を同じ acceptance controller に組み込んでいない。

### 受入条件

- Next.js interactive task で release/UAT mode の場合、dev server start / readiness wait / route probe / cleanup が一つの lifecycle event series として残る。
- port in use、bind denied、startup timeout、HTTP 500、browser unavailable を別 failure kind に分類する。
- dev server lifecycle が未実行なら full pass にならない。
- dev server が起動できない環境では `browser_unavailable` として partial にし、HTTP 500 と混同しない。

### RECOVERY-002-B evidence

- `mvp/anvilminimal/src/planner/runner.rs` に Next.js dev server lifecycle を追加した。`start`、`wait`、`probe`、`cleanup` を `dev_server_lifecycle` event として出し、probe evidence を `browser-readiness.json` に保存する。
- Runtime unit tests では dev server 起動を無効化し、Playwright/browser/dev-server を通常 test の必須依存にしない。一方で同じ lifecycle event series と `browser_unavailable:dev_server_probe_disabled_in_tests` evidence を検証する。
- 実 probe は `npm/pnpm/yarn run dev` のみを使い、network install や package manifest の自動変更は行わない。`node_runner_manifest.rs` の deterministic manifest completion 方針とは分離し、dev server readiness で package.json を勝手に補完しない。
- `runtime_scoring.py` と `runtime_trace.py` は新 `dev_server_lifecycle` event を postcheck/runtime trace の readiness signal として扱う。
- Automated evidence: `cargo test --manifest-path mvp/anvilminimal/Cargo.toml`、`pytest mvp/anvilminimal/tests/eval` pass。
- Manual UAT は未実施のため `pass` ではなく `partial`。

---

## G12: planner verify normalization / quality warning が不安定性として gate に残らない

| 項目 | 内容 |
| --- | --- |
| priority | P1 |
| category | `diagnostic_gap` |
| current_status | partial |

### 現象

`test0701_004` events には `planner_verify_command_normalized`、`planner_error: verify_command_policy_error`、`planner_quality_warning: multiple expected paths owned by one step` が出ている。しかし最終的には phase が完走し、summary では planner instability が十分目立たない。

### source refs

- `/Users/maenokota/share/work/github_kewton/Anvil-develop/src/agent/loop_run/verifier_command_policy.rs`
- `/Users/maenokota/share/work/github_kewton/Anvil-develop/src/agent/minimal_step_runner.rs`
- `/Users/maenokota/share/work/github_kewton/Anvil-develop/src/agent/minimal_step_runner/repair.rs`

### MVP refs

- `/Users/maenokota/share/work/github_kewton/Anvil-develop/mvp/anvilminimal/src/planner/verify.rs`
- `/Users/maenokota/share/work/github_kewton/Anvil-develop/mvp/anvilminimal/src/planner/lint.rs`
- `/Users/maenokota/share/work/github_kewton/Anvil-develop/mvp/anvilminimal/src/planner/runner.rs`
- `/Users/maenokota/share/work/github_kewton/Anvil-develop/mvp/anvilminimal/scripts/eval_lib/runtime_scoring.py`

### 問題箇所

deterministic normalization は有効だが、normalization が多い、retry 後も warning が残る、expected path ownership が粗い、といった planner instability が acceptance / release risk として十分に集約されない。

### 移植元との比較

source は verifier command policy 違反を fail-fast / retry / repair lifecycle の入力として扱う。MVP は normalization により進めることはできるが、「荒い plan を controller が救済した」という事実が gate 上の不安定性として残りにくい。

### 根本原因

planner output quality と runtime success を分ける設計は入ったが、planner normalization / lint retry の回数・種類・残存 warning を release risk として扱う gate が不足している。

### 受入条件

- planner normalization / retry / quality warning counts が summary と eval report に出る。
- safe normalization で成功した場合も `planner_repaired` として記録される。
- warning が残った plan は release-quality full pass にならない、または intentional evidence を要求する。
- verify command policy violation が runtime Bash で迂回されていないことを確認する fixture がある。

### RECOVERY-002-E status / evidence

- `mvp/anvilminimal/src/eval_events.rs` の completion projection が run 全体の `planner_verify_command_normalized`、planner retry/error、`planner_quality_warning`、`planner_quality_issue` を集計し、TUI と `.anvil/runs/<run-id>/summary.md` に `Planner diagnostics`、`Planner repaired`、`Planner release risk` として表示する。
- `tui_command_stop` / `run_stop` events に planner diagnostics fields を追加し、machine-readable event でも normalization/retry/warning/issue counts を確認できる。
- `mvp/anvilminimal/scripts/eval_lib/runtime_scoring.py` と summary TSV に `runtime_bash_policy_error_count` / `runtime_bash_verifier_bypass_count` を追加し、runtime Bash による verifier policy bypass を tool policy score に反映する。
- fixtures: `completion_projection_renders_planner_diagnostics_as_release_risk`、`test_runtime_bash_verify_policy_bypass_reduces_tool_policy_score`。

---

## G13: CompletionContract / external contract が通常 TUI ultra-run に bind されていない

| 項目 | 内容 |
| --- | --- |
| priority | P0 |
| category | `acceptance_bridge_gap` |
| current_status | partial |

### 現象

`test0701_004` events では各 step の `step_obligation_scope` に `completion_contract_verification_enabled=false`、`completion_contract_paths=[]` が出ている。final contract でも `external_contract_checked=false` である。つまり required capabilities は prompt/event には出ているが、外部 contract として通常 TUI ultra-run に bind されていない。

### source refs

- `/Users/maenokota/share/work/github_kewton/Anvil-develop/src/agent/loop_run/task_contract.rs`
- `/Users/maenokota/share/work/github_kewton/Anvil-develop/src/agent/loop_run/task_contract_core.rs`
- `/Users/maenokota/share/work/github_kewton/Anvil-develop/src/agent/loop_run/task_contract_completion_policy.rs`
- `/Users/maenokota/share/work/github_kewton/Anvil-develop/src/agent/loop_run/evidence_binding.rs`

### MVP refs

- `/Users/maenokota/share/work/github_kewton/Anvil-develop/mvp/anvilminimal/src/minimal_loop/completion.rs`
- `/Users/maenokota/share/work/github_kewton/Anvil-develop/mvp/anvilminimal/src/minimal_loop/evidence.rs`
- `/Users/maenokota/share/work/github_kewton/Anvil-develop/mvp/anvilminimal/src/planner/runner.rs`

### 問題箇所

TaskContract-lite / CompletionContract の型や evidence derivation は追加されているが、通常 TUI の `/ultra-plan-run` path では external contract として有効化されていない。結果として、controller が source equivalent の completion authority を持たないまま static runtime acceptance に寄る。

### 移植元との比較

source は TaskContract が loop/controller の completion decision に入る。MVP は contract 情報を prompt と eval に寄せているが、実行 controller の completion gate として常時参照できていない。

### 根本原因

contract を「評価指標・prompt 補助」として扱い、通常実行の authoritative completion input として bind する工程が抜けている。

### 受入条件

- TUI/ultra-run でも CompletionContract / external contract が生成・保存・bind される。
- `completion_contract_verification_enabled=false` が interactive app/game の通常実行で残る場合、gate failure として扱う。
- `external_contract_checked=false` のまま final full pass にならない。
- contract paths/capabilities/evidence が plan/phase/step/final acceptance へ同一 run-id で紐づく。

### RECOVERY-002-D status / evidence

- status: `partial`
- implemented:
  - `mvp/anvilminimal/src/planner/runner.rs` に `completion_contract_bound` lifecycle を追加し、明示 `CompletionContract` がない interactive app/game では run dir に `completion-contract-<scope>.json` を生成・保存して plan-run / ultra-plan-run final acceptance に bind する。
  - `plan_final_contract` / `ultra_final_acceptance` events に `completion_contract_verification_enabled`、`completion_contract_path_merge_enabled`、`completion_contract_path`、`completion_contract_generated`、`external_contract_checked`、`external_contract_required`、`external_contract_ok` を追加。
  - `mvp/anvilminimal/src/eval_events.rs` と `mvp/anvilminimal/src/tui/slash.rs` に contract fields を projection / TUI stop / summary 出力へ接続し、`completion_contract_verification_enabled=true` と `external_contract_checked=true` を TUI/summary で確認可能にした。
  - `mvp/anvilminimal/src/minimal_loop/completion.rs` の `CompletionContract` / `DeferredVerifyRequirement` を `Serialize` 可能にし、生成 contract の保存に対応。
- automated evidence:
  - `cargo test --manifest-path mvp/anvilminimal/Cargo.toml planner::runner::tests::ultra_final_acceptance_binds_generated_completion_contract`
  - `cargo test --manifest-path mvp/anvilminimal/Cargo.toml eval_events::tests::completion_projection_renders_contract_binding_state`
  - `cargo test --manifest-path mvp/anvilminimal/Cargo.toml planner::runner::tests::plan_run_nextjs_game_scaffold_only_fails_inferred_capabilities`
  - `cargo test --manifest-path mvp/anvilminimal/Cargo.toml`
  - `pytest mvp/anvilminimal/tests/eval`
- residual risk:
  - step-level `step_obligation_scope` は deliberate に `plan-run-step` side effect を無効化したまま維持する。通常 run の authoritative check は run/final acceptance level の `completion_contract_bound` / final contract event で確認する。

---

## G14: step kind の role authority が弱く、setup/verify/report と implementation の責務境界が曖昧

| 項目 | 内容 |
| --- | --- |
| priority | P2 |
| category | `semantic_drift` |
| current_status | partial |

### 現象

`test0701_004` では `setup-entrypoints` のような setup step が既存の implementation artifacts を expected paths として持ち、verify step が Bash/Read で verification-like work を行う。phase は完走するが、どの role がどの obligation を満たしたかが source ほど明確ではない。

### source refs

- `/Users/maenokota/share/work/github_kewton/Anvil-develop/src/agent/loop_run/task_contract_deliverable_lifecycle.rs`
- `/Users/maenokota/share/work/github_kewton/Anvil-develop/src/agent/loop_run/task_contract_artifact_contract.rs`
- `/Users/maenokota/share/work/github_kewton/Anvil-develop/src/agent/loop_run/evidence_binding.rs`
- `/Users/maenokota/share/work/github_kewton/Anvil-develop/src/agent/minimal_step_runner.rs`

### MVP refs

- `/Users/maenokota/share/work/github_kewton/Anvil-develop/mvp/anvilminimal/src/planner/step_plan.rs`
- `/Users/maenokota/share/work/github_kewton/Anvil-develop/mvp/anvilminimal/src/planner/lint.rs`
- `/Users/maenokota/share/work/github_kewton/Anvil-develop/mvp/anvilminimal/src/planner/runner.rs`
- `/Users/maenokota/share/work/github_kewton/Anvil-develop/mvp/anvilminimal/src/minimal_loop/evidence.rs`

### 問題箇所

step kind は prompt/lint には存在するが、setup/scaffold/implementation/verify/report の authority が controller completion に十分反映されない。setup-only / verify-only / report-only の成果が implementation completion と混ざる余地がある。

### 移植元との比較

source は deliverable lifecycle と artifact contract で、どの artifact が implementation を満たすか、どの evidence が completion に使えるかを分ける。MVP は role を最小化しているが、role authority を completion decision に反映する部分がまだ弱い。

### 根本原因

TaskContract-lite の role を小さく保つ方針は妥当だが、role を「表示情報」ではなく「completion authority」にする接続が不足している。

### 受入条件

- setup/scaffold/verify/report step の artifact は、そのまま implementation obligation を満たさない。
- implementation role の artifact と capability evidence の対応が必要になる。
- expected path ownership warning は role authority と合わせて評価される。
- role authority の不足は planner_quality_warning ではなく acceptance/contract issue として集計される。

### RECOVERY-002-D status / evidence

- status: `partial`
- implemented:
  - `mvp/anvilminimal/src/minimal_loop/evidence.rs` の artifact role authority を維持し、setup/scaffold/style/verification/acceptance_evidence は `satisfies_implementation=false` として implementation obligation を直接満たさない。
  - capability binding の scaffold fallback を削除し、scaffold path が implementation capability の bind target に見えないようにした。
  - verification artifact と report artifact だけでは `implementation_artifact` / `implementation` obligation を満たせない fixture を追加。
- automated evidence:
  - `cargo test --manifest-path mvp/anvilminimal/Cargo.toml minimal_loop::evidence::tests::verification_and_report_artifacts_do_not_satisfy_implementation_obligation`
  - `cargo test --manifest-path mvp/anvilminimal/Cargo.toml planner::runner::tests::plan_run_nextjs_game_scaffold_only_fails_inferred_capabilities`
  - `cargo test --manifest-path mvp/anvilminimal/Cargo.toml`
  - `pytest mvp/anvilminimal/tests/eval`
- residual risk:
  - expected path ownership warning と planner quality warning の集計連動は G12 側で継続管理する。

## Cross-Cutting Root Cause

RECOVERY-001 で多くの部品は戻ったが、まだ source の本質である以下の連鎖が一体の controller semantics になっていない。

```text
task/profile contract
-> phase/step execution
-> deterministic verifier / runtime evidence
-> completion authority
-> repair target / bounded repair
-> recovery handoff
-> user-facing diagnostics
```

MVP ではこの連鎖が以下のように分断している。

- phase completion は進む。
- static acceptance は pass する。
- release gate は partial を出す。
- browser readiness は通常実行で取得されない。
- dev server lifecycle が controller action になっていない。
- CompletionContract / external contract が通常 TUI ultra-run に bind されていない。
- partial は failure / repair / recovery に接続されない。
- planner normalization / quality warning が release risk として十分残らない。
- step role authority が completion decision に十分反映されない。
- summary は最後に complete を出す。

これは単一バグではなく、移植時に source の lifecycle semantics を implementation-level の小部品として戻し、completion/release/user-facing state まで一つの状態機械として戻せていないことが根本である。

## Recovery 002 Exit Gate

Recovery 002 の対象は、以下を満たすまで完了扱いにしない。

- G01〜G04/G11/G13 の P0 gap は `pass` または根拠付き `intentionally_different`。
- `test0701_004` 相当で build pass / browser HTTP 500 が full success にならない。
- release gate partial/fail 時に recovery `.md` / recovery UltraPlan YAML が保存される。
- TUI summary に partial/fail と next action が明示される。
- static capability evidence だけで interactive app/game が release pass にならない。
- runtime Bash の shell control syntax が deterministic verifier evidence として扱われない。
- planner normalization / quality warning が gate/report に残る。
- CompletionContract / external contract が通常 TUI/ultra-run に bind される。
- setup/scaffold/verify/report role が implementation completion と混同されない。
- manual UAT 結果が recovery status に反映される。
