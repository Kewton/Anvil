# Migration Gap Inventory

作成日: 2026-07-01

## 判定基準

本書では、移植漏れを以下の5種類に分ける。

| 種別 | 意味 |
| --- | --- |
| `missing` | source にある lifecycle / controller flow が MVP に実質存在しない。 |
| `semantic_drift` | 名前や一部部品はあるが、source と同じ状態遷移になっていない。 |
| `simplified_risk` | MVP 境界として簡略化した結果、source の安全装置・回復力が不足している。 |
| `verification_gap` | 実装済みかどうかを判定する trace / eval / UAT gate が不足している。 |
| `new_acceptance_gate` | source に完全同等は薄いが、MVP の release 品質上必要な gate。 |

## ステータス定義

REC の状態は以下で管理する。

| status | 意味 | 必須記録 |
| --- | --- | --- |
| `open` | 対応対象として確定したが、実装・fixture・eval・UAT の証跡が揃っていない。 | 次に必要な証跡、blocking condition。 |
| `partial` | 一部修正または一部証跡はあるが、受入条件を満たし切っていない。 | 満たした条件、未充足条件、次の確認タイミング。 |
| `pass` | 受入条件、fixture/eval、必要な trace/UAT evidence が揃っている。 | 証跡ファイル、比較結果、残リスク。 |
| `fail` | 対応後も受入条件を満たさない、または P0/P1 の必須 gate を満たせない。 | 失敗理由、再計画対象、rollback/追加対策。 |
| `intentionally_different` | source と同じ挙動にはしないが、MVP の代替 gate でリスクを管理する。 | source と違う理由、代替 gate、残リスク、再確認タイミング。 |

`pass` は evidence path の存在だけでは認めない。内容を読み、`ok=false`、HTTP 500、missing artifact、blank failure kind、recovery-artifact-only success が残っていないことを確認する。

## レビュー反映サマリ

| 観点 | 判定 | 反映内容 |
| --- | --- | --- |
| 設計思想 | 概ね妥当 | full source graph の移植ではなく、source semantics の薄い写像を前提にした。 |
| 不安定化リスク | 要注意 | browser / provider / network を通常 unit test の必須にしない。release/live gate として分離する。 |
| 影響範囲 | 追記済み | plan-run / ultra-plan-run だけでなく minimal-loop、TUI、provider probe、eval gate へ横展開した。 |
| 複雑性 | 要管理 | 各 REC に「丸ごと移植しない」「既存型の拡張で済むか確認」を前提として残した。 |
| 原因深掘り | 追記済み | 部品不足ではなく lifecycle の断絶として記述した。 |
| 移植漏れ/追加要件の区別 | 追記済み | browser readiness は `new_acceptance_gate` として扱い、純粋な source parity と分けた。 |
| 個別具体への過適応 | 対応済み | Space Invaders ではなく interactive app/game, JS framework, dependency lifecycle として記述した。 |
| 完了条件 | 強化済み | source/MVP trace、positive/negative fixture、manual UAT evidence のどれが必要かを明記した。 |

## 根拠サマリ

- `runtime_semantics_gate_matrix.md` では G-S01〜G-S16 が全て `partial`。
- 最新 `parity_gate_report.json` では MVP aggregate success は anvildev を上回る一方、release evidence は `partial`。
- UAT では TUI `/ultra-plan-run --profile nextjs ...` が `dependency_setup_missing` で未完了。
- browser readiness は HTTP 500、interaction evidence は canvas unavailable。
- MVP 側は failure を検知できるようになっているが、source と同等に「setup/build/repair/acceptance へ運ぶ」runtime bridge がまだ弱い。

## 全体 Pass 条件

この inventory 全体は、以下を満たすまで「移植漏れ解消済み」とは扱わない。

- REC-001〜REC-010 の各項目が `pass` / `fail` / `intentionally_different` のいずれかに落ちている。
- `partial` が残る場合、その理由が「検証未了」「意図的簡略化」「source との差分許容」のどれかに分類されている。
- MVP/anvildev same-condition trace diff がある。
- manual UAT 相当の browser / interaction / TUI evidence content が確認されている。
- success rate だけでなく、failure kind、failure layer、acceptance false positive、release evidence を見る。
- build-only / path-only / recovery-artifact-only の false positive を再許可していない。

## ゴール対応表

`README.md` のゴールと REC の対応は以下。

| goal_id | ゴール | 主対応 REC | 確認方法 |
| --- | --- | --- | --- |
| G-001 | 残存移植漏れを REC-001〜REC-010 として追跡可能にする | REC-001〜REC-010 | source refs / MVP refs / 現象 / 根本原因 / 受入条件が各 REC にあることを確認する。 |
| G-002 | `partial` を放置せず `pass` / `fail` / `intentionally_different` に落とす | REC-007, 全 REC | normalized trace diff、gate report、REC status を確認する。 |
| G-003 | Next.js/Tailwind dependency/setup/build/browser の断絶を lifecycle として検証する | REC-001, REC-002, REC-006, REC-010 | dependency lifecycle event、build rerun、browser readiness、TUI summary を確認する。 |
| G-004 | build-only / path-only / recovery-artifact-only false positive を再許可しない | REC-003, REC-004, REC-005, REC-006 | completion contract、capability evidence、repair follow-through、release evidence content を確認する。 |
| G-005 | MVP 単体成功率ではなく anvildev 比較 / trace / UAT evidence で判断する | REC-007, REC-008, REC-010 | same-condition eval、provider probe、manual UAT evidence を確認する。 |

## 現況ステータス

この表は「今この文書を書いた時点の実装完了状況」ではなく、「次に確認すべき recovery status」を示す。

| REC | priority | current_status | pass に必要な次証跡 | 完了扱いを阻害する条件 |
| --- | --- | --- | --- | --- |
| REC-001 | P0 | partial | manual UAT、full eval、anvildev same-condition trace。 | release-level evidence 未実施。unit/eval では setup authority selected / attempted / passed/failed / rerun taxonomy を確認済み。 |
| REC-002 | P0 | partial | browser readiness evidence、manual UAT、full eval、anvildev same-condition trace。 | release-level evidence 未実施。unit/eval では Tailwind installed evidence、plain CSS pass、browser HTTP 500 fail を確認済み。 |
| REC-003 | P1 | partial | manual UAT、source/MVP trace diff、browser/interaction evidence。 | unit/eval では empty/scaffold/title/style/docs/manifest-only rejection、fallback event、fallback-only non-completion を確認済み。 |
| REC-004 | P1 | partial | source/MVP trace diff、unsupported source obligation の deferred/intentionally_different 整理。 | unit/eval では obligation-to-artifact binding、missing implementation repair target、plan/ultra event 伝播を確認済み。 |
| REC-005 | P1 | pass | 実装レベルの受入条件は unit/fixture/eval/YAML roundtrip で確認済み。release-level manual/TUI evidence は REC-010 で扱う。 | none。source full `RepairJob` は移植せず、MVP の薄い lifecycle 写像として管理する。 |
| REC-006 | P1 | pass | 実装レベルの受入条件は unit/fixture/eval で確認済み。live browser/manual UAT evidence は REC-010 で扱う。 | none。browser/Playwright は通常 unit test 必須にせず、保存済み evidence content gate として管理する。 |
| REC-007 | P1 | pass | 実装レベルの受入条件は normalized trace diff / parity report / eval pytest で確認済み。release-level failed gates は report の `failed_gate_ids` として扱う。 | none。code reference だけの pass と trace 欠損 gate の pass 扱いは schema/gate で禁止済み。 |
| REC-008 | P2 | pass | provider probe metadata / summary、provider-specific args shape observation、tool args recovery classification。 | live provider smoke は credentials/network がある release/targeted gate で再確認する。 |
| REC-009 | P2 | open | verify normalization fixture、original/normalized verify event、runtime Bash policy rejection fixture。 | shell control syntax 許可、setup/build ordering 違反の success、normalization event 欠損。 |
| REC-010 | P2 | open | manual TUI run events、summary、recovery `.md` / `.yaml` parse check、release evidence registration。 | silent exit、summary 欠損、recovery artifact path だけで内容未確認。 |

## 達成確認マトリクス

各 REC は、以下の確認方法と確認タイミングを最低限満たす。

| REC | 主な達成条件 | 確認方法 | 確認タイミング |
| --- | --- | --- | --- |
| REC-001 | dependency/setup/build lifecycle が source semantics に近い形で閉じる | lifecycle event、setup authority、install/setup result、build rerun、failure kind を確認 | targeted fixture 後、manual UAT 後、full eval 後 |
| REC-002 | Next.js/Tailwind contract が setup/build/browser acceptance に伝播する | package/config/CSS/toolchain/dev route evidence を確認 | targeted Next.js fixture 後、browser readiness 後 |
| REC-003 | playable UI fallback/recovery が quality不足を実装 continuation へ戻す | capability evidence、fallback event、smoke quality check を確認 | targeted interactive app/game fixture 後、manual UAT 後 |
| REC-004 | TaskContract-lite obligation が artifact/evidence/repair target に接続される | required obligations、artifact evidence、missing obligation repair target を確認 | unit/fixture 後、plan-run/ultra-plan-run targeted eval 後 |
| REC-005 | repair が target に沿って成果物を変更し、no-change を分類する | repair target、changed paths、rerun result、handoff artifact を確認 | repair fixture 後、failure-heavy eval 後 |
| REC-006 | final acceptance が build-only/path-only を full success にしない | runtime acceptance report、browser/interaction evidence、release gate report を確認 | final acceptance fixture 後、manual UAT 後 |
| REC-007 | source/MVP trace diff で parity を判定できる | normalized event diff、gate report、source trace manifest を確認 | 各 REC 実装後、full eval 後 |
| REC-008 | provider live probe が prompt/tool-call 不確実性を下げる | provider probe summary、skip reason、tool args shape を確認 | provider-sensitive 修正後、provider smoke 後 |
| REC-009 | planner verify normalization と runtime Bash policy の境界が明確 | normalized verify event、original command summary、policy failure kind を確認 | step-plan fixture 後、plan-run targeted eval 後 |
| REC-010 | TUI/manual run の停止理由と recovery action が追跡できる | events.jsonl、summary.md、recovery `.md` / `.yaml`、suggested command を確認 | manual UAT 後、release gate 判定時 |
 
## 確認結果の記録ルール

- 各 REC の実装計画には、上記マトリクスの該当行をコピーし、実行した確認と結果を記録する。
- `pass` にする場合は、fixture / eval / trace / UAT のうちどの証跡で確認したかを明記する。
- `partial` にする場合は、未実施の確認方法と次の確認タイミングを明記する。
- `intentionally_different` にする場合は、source と違う理由、MVP 側の代替 gate、リスクを明記する。
- evidence path の存在だけで pass にしない。内容を読み、`ok=false`、HTTP 500、missing artifact、blank failure kind を確認する。

## 実装結果記録テンプレート

各 REC の実装後は、該当 REC セクションの末尾に以下を追記する。未実施項目は空欄にせず、未実施理由と次の確認タイミングを書く。

```markdown
### 実装結果

| 項目 | 内容 |
| --- | --- |
| status | open / partial / pass / fail / intentionally_different |
| 実施日 | YYYY-MM-DD |
| 実施 step | RECOVERY-001-* |
| 実装概要 | 変更した lifecycle semantics と主な変更点を書く。 |
| source parity 判断 | source と同等 / source と差分あり / intentionally different / 未確認 |
| 証跡 | fixture、eval、trace、UAT、browser/provider evidence のファイルパスを書く。 |
| 通過した確認 | unit / fixture / targeted eval / full eval / anvildev comparison / manual UAT |
| 未確認事項 | 未実施の確認と理由を書く。 |
| blocking condition | まだ完了扱いを阻害する条件を書く。無い場合は none と書く。 |
| 次の確認タイミング | 実装直後 / targeted eval 後 / full eval 後 / manual UAT 後 / release gate 判定時 |
| rollback 判断 | rollback 不要 / rollback 候補 / 追加対策必要 |
```

`pass` にする場合は、`blocking condition` が `none` であり、かつ `証跡` に内容確認済みの結果が含まれていることを必須とする。

---

## REC-001: dependency/setup/build lifecycle

| 項目 | 内容 |
| --- | --- |
| 種別 | `missing` / `semantic_drift` |
| 優先度 | P0 |
| 関連 gate | G-S09, G-S08, G-S10, G-S12 |
| 影響 mode | minimal-loop, plan-run, ultra-plan-run |
| 影響 profile | nextjs, js framework, node test |

### source refs

- `/Users/maenokota/share/work/github_kewton/Anvil-develop/src/agent/loop_run/active_job_arbiter.rs`
- `/Users/maenokota/share/work/github_kewton/Anvil-develop/src/agent/loop_run/tool_policy.rs`
- `/Users/maenokota/share/work/github_kewton/Anvil-develop/src/tools/bash.rs`
- `/Users/maenokota/share/work/github_kewton/Anvil-develop/src/agent/loop_run/auto_test.rs`
- `/Users/maenokota/share/work/github_kewton/Anvil-develop/src/agent/loop_run/project_probe.rs`
- `/Users/maenokota/share/work/github_kewton/Anvil-develop/src/agent/loop_run/project_verifier.rs`
- `/Users/maenokota/share/work/github_kewton/Anvil-develop/src/agent/loop_run/node_request_helpers.rs`
- `/Users/maenokota/share/work/github_kewton/Anvil-develop/src/agent/loop_run/node_runner_manifest.rs`
- `/Users/maenokota/share/work/github_kewton/Anvil-develop/src/agent/minimal_step_runner/profiles/nextjs.rs`

### MVP refs

- `/Users/maenokota/share/work/github_kewton/Anvil-develop/mvp/anvilminimal/src/minimal_loop/dependency_setup.rs`
- `/Users/maenokota/share/work/github_kewton/Anvil-develop/mvp/anvilminimal/src/minimal_loop/build_verifier.rs`
- `/Users/maenokota/share/work/github_kewton/Anvil-develop/mvp/anvilminimal/src/minimal_loop/completion.rs`
- `/Users/maenokota/share/work/github_kewton/Anvil-develop/mvp/anvilminimal/src/planner/verify.rs`
- `/Users/maenokota/share/work/github_kewton/Anvil-develop/mvp/anvilminimal/src/planner/runner.rs`

### 現状

MVP には `NodeDependencySetupAuthority`、`dependency_setup.rs`、`build_verifier.rs` がある。`npm install --ignore-scripts` と build rerun の薄い経路もある。

しかし UAT では `dependency_setup_missing: Cannot find module tailwindcss` で停止した。これは、source の `SetupBootstrap -> safe install -> build rerun -> verification` と同じ lifecycle へ十分に接続できていないことを示す。

### 問題点

- verify failure から setup authority へ戻る判定が source より狭い。
- setup step が存在しても、実際に `node_modules` / required binary / Tailwind toolchain が ready かを controller が最後まで運べていない。
- setup が blocked / failed / timed out の時に、repair target と build rerun の関係が弱い。
- `pnpm` / `yarn` / mixed lockfile は現状 deferred で、source 同等か未検証。

### 根本原因

source では install は verifier に混ぜない一方、controller が `SetupBootstrap` 権限を選び、command-level policy で setup command のみ許可する。MVP は型だけは近づいたが、plan-run / ultra-plan-run / minimal-loop の各 runtime が同じ setup lifecycle を共有する状態まで閉じていない。

### 受入条件

- `@tailwind` 使用、`package.json` あり、`node_modules` なしの Next.js fixture で以下の event sequence が出る。
  - dependency check
  - setup authority selected
  - setup attempted
  - setup passed/failed/timed_out
  - build rerun attempted when setup passed
  - verification passed/failed with concrete failure kind
- setup authority が無い場合は `dependency_setup_missing` / `setup_blocked` として fail-fast する。
- setup-only / manifest-only は success にならない。
- plan-run / ultra-plan-run / minimal-loop で同じ taxonomy を使う。
- manual UAT で `Cannot find module tailwindcss` が silent/incomplete artifact ではなく、setup lifecycle failure か successful setup/build rerun のどちらかになる。
- source の `SetupBootstrap` と同一実装でなくても、setup authority selection、command-level setup-only policy、rerun observation が trace 上で説明できる。
- network unavailable / install timeout / package manager unsupported は success ではなく、具体 failure kind と recovery/handoff に落ちる。

### 実装結果

| 項目 | 内容 |
| --- | --- |
| status | partial |
| 実施日 | 2026-07-01 |
| 実施 step | RECOVERY-001-A |
| 実装概要 | Next.js build dependency readiness を `node_modules/.bin/next` 単体ではなく Tailwind runtime toolchain installed evidence まで拡張し、`dependency_build_lifecycle` に `setup_authority_selected` / `setup_authority_missing` / `setup_attempted` / `build_rerun_attempted` を追加した。plan-run / ultra-plan-run / minimal-loop の standalone lifecycle event を eval scoring/classification でも同じ taxonomy として扱うようにした。 |
| source parity 判断 | source と差分あり。source の `SetupBootstrap` actor/policy 丸ごと移植ではなく、MVP の `NodeDependencySetupAuthority` と build verifier lifecycle へ薄く写像した。 |
| 証跡 | `mvp/anvilminimal/src/minimal_loop/dependency_setup.rs` unit tests: Tailwind declared but not installed setup candidate、missing package contract setup block。`mvp/anvilminimal/src/minimal_loop/build_verifier.rs` unit tests: setup blocked / setup failed / setup passed + build rerun lifecycle。`mvp/anvilminimal/tests/eval/test_failure_classification.py` と `test_runtime_scoring.py`: standalone dependency lifecycle taxonomy。 |
| 通過した確認 | unit / fixture / targeted eval。`cargo test --manifest-path mvp/anvilminimal/Cargo.toml`、`pytest mvp/anvilminimal/tests/eval` 通過。 |
| 未確認事項 | manual UAT、full eval、anvildev same-condition comparison は未実施。ネットワーク install の live success/failure は fake npm と unit taxonomy で代替確認した。 |
| blocking condition | release-level evidence 未実施。 |
| 次の確認タイミング | manual UAT 後 / full eval 後 / release gate 判定時 |
| rollback 判断 | rollback 不要 |

---

## REC-002: Next.js/Tailwind runtime contract

| 項目 | 内容 |
| --- | --- |
| 種別 | `semantic_drift` |
| 優先度 | P0 |
| 関連 gate | G-S09, G-S12 |
| 影響 mode | plan-run, ultra-plan-run |
| 影響 profile | nextjs |

### source refs

- `/Users/maenokota/share/work/github_kewton/Anvil-develop/src/agent/minimal_step_runner/profiles/nextjs.rs`
- `/Users/maenokota/share/work/github_kewton/Anvil-develop/src/agent/loop_run/project_probe.rs`
- `/Users/maenokota/share/work/github_kewton/Anvil-develop/src/agent/loop_run/project_verifier.rs`

### MVP refs

- `/Users/maenokota/share/work/github_kewton/Anvil-develop/mvp/anvilminimal/src/planner/profiles/nextjs.rs`
- `/Users/maenokota/share/work/github_kewton/Anvil-develop/mvp/anvilminimal/src/minimal_loop/build_verifier.rs`
- `/Users/maenokota/share/work/github_kewton/Anvil-develop/mvp/anvilminimal/src/minimal_loop/dependency_setup.rs`

### 現状

MVP profile verifier は `@tailwind` directive と package/config の整合性を見ている。だが UAT では build が通っても dev route で CSS parse error になった。

### 問題点

- Tailwind の static contract と dependency install/build/browser runtime が分断している。
- `@tailwind` を使うなら toolchain 完備、そうでなければ plain CSS へ寄せるという source の runtime contract が、実成果物の acceptance まで届いていない。
- build success と dev server route failure が別 failure kind として扱われ始めたが、通常実行で recovery target に戻す力がまだ弱い。

### 根本原因

Tailwind は package/config/file の存在だけでは十分ではない。source の profile contract は「Tailwind を使うなら一貫した toolchain、使わないなら plain CSS」と表現しているが、MVP は生成/verify/repair/final acceptance の各段でその契約を一貫して使えていない。

### 受入条件

- `@tailwind` directive がある場合、package/config/install/build/dev route evidence のいずれかが欠ければ precise failure になる。
- Tailwind を使わない Next.js app は plain CSS として pass できる。
- `build pass / browser HTTP 500` が full success にならない。
- failure は `tailwind_contract_failure` / `profile_runtime_contract_error` / `browser_readiness_failed` などに分類される。
- Tailwind を使うよう planner が計画した場合は setup/build lifecycle と final acceptance の両方へ contract が伝播する。
- Tailwind を使わない方針へ repair する場合は、`@tailwind` directive と Tailwind utility 前提の JSX/CSS を残さない。

### 実装結果

| 項目 | 内容 |
| --- | --- |
| status | partial |
| 実施日 | 2026-07-01 |
| 実施 step | RECOVERY-001-A |
| 実装概要 | Tailwind contract failure を `tailwind_contract_failure` として profile verifier / terminal stop reason / eval classifier へ伝播した。Tailwind config variants を `.js/.cjs/.mjs/.ts` へ拡張し、Tailwind を使う場合は package/config/install/build evidence の欠落を precise failure に寄せた。Tailwind を使わない Next.js app は plain CSS として pass する既存 gate を維持した。 |
| source parity 判断 | source と差分あり。source profile contract の語彙を MVP profile/build/browser acceptance に写像し、full source verifier graph は移植していない。 |
| 証跡 | `mvp/anvilminimal/src/planner/profiles/nextjs.rs` unit tests: missing Tailwind toolchain failure prefix、CJS config variants、plain CSS pass。`mvp/anvilminimal/src/minimal_loop/build_verifier.rs` unit test: next binary presentでも Tailwind node_modules missing を dependency missing に分類。`mvp/anvilminimal/tests/eval/test_acceptance_outcome.py`: browser HTTP 500 acceptance failure 既存テスト通過。 |
| 通過した確認 | unit / fixture / targeted eval。`cargo test --manifest-path mvp/anvilminimal/Cargo.toml`、`pytest mvp/anvilminimal/tests/eval` 通過。 |
| 未確認事項 | manual browser readiness evidence、full eval、anvildev same-condition comparison は未実施。Tailwind utility class の静的検出は今回の対象外で、`@tailwind` directive / package / config / install / build / browser evidence を対象にした。 |
| blocking condition | release-level evidence 未実施。 |
| 次の確認タイミング | browser readiness UAT 後 / full eval 後 / release gate 判定時 |
| rollback 判断 | rollback 不要 |

---

## REC-003: playable UI / framework fallback / smoke quality recovery

| 項目 | 内容 |
| --- | --- |
| 種別 | `missing` / `simplified_risk` |
| 優先度 | P1 |
| 関連 gate | G-S11, G-S12 |
| 影響 mode | minimal-loop, plan-run, ultra-plan-run |
| 影響 profile | nextjs, react, nuxt, sveltekit |

### source refs

- `/Users/maenokota/share/work/github_kewton/Anvil-develop/src/agent/loop_run/quality.rs`
- `/Users/maenokota/share/work/github_kewton/Anvil-develop/src/agent/loop_run/scaffold_pipeline.rs`
- `/Users/maenokota/share/work/github_kewton/Anvil-develop/src/agent/loop_run/no_progress_recovery.rs`

### MVP refs

- `/Users/maenokota/share/work/github_kewton/Anvil-develop/mvp/anvilminimal/src/minimal_loop/evidence.rs`
- `/Users/maenokota/share/work/github_kewton/Anvil-develop/mvp/anvilminimal/src/minimal_loop/completion.rs`
- `/Users/maenokota/share/work/github_kewton/Anvil-develop/mvp/anvilminimal/src/planner/runner.rs`
- `/Users/maenokota/share/work/github_kewton/Anvil-develop/mvp/anvilminimal/src/planner/profiles/nextjs.rs`

### 現状

source には `deterministic_empty_framework_game_files`、`react_canvas_game_template`、`smoke-test.mjs`、playable UI polish fallback がある。MVP には capability evidence / acceptance gate はあるが、source のように「空/薄い framework app を playable scaffold に戻す」回復経路は薄い。

### 問題点

- MVP は title-only / docs-only / style-only を失敗にできるが、失敗後に実装品質を押し上げる deterministic recovery が弱い。
- `src/app/page.tsx` が存在し canvas やイベントが多少あっても、ゲームとしての状態遷移・restart・challenge が薄い場合に continuation へ戻す力が不足する。

### 根本原因

source は acceptance だけでなく、fallback/recovery に具体的な playable scaffold と smoke quality marker を持っている。MVP は「検知」を優先して整備してきたため、「良質な実装へ戻す」側の source semantics が残っている。

### 受入条件

- interactive app/game request で empty/scaffold/title-only の場合、completion ではなく implementation continuation または deterministic scaffold recovery へ進む。
- recovery は Space Invaders 固有ではなく、interactive game / web app の一般 capability で動く。
- source と同じ template 丸コピーではなくても、visible interactive surface、input handler、stateful loop、challenge/failure/restart、score/progression の evidence を満たす。
- smoke quality script または同等の deterministic check が final acceptance に接続される。
- deterministic fallback を使った場合は、fallback 使用を event/summary に残し、通常生成の成功と区別する。
- fallback は completion ではなく continuation target として扱う。fallback だけで final success にしない。

### 実装結果

| 項目 | 内容 |
| --- | --- |
| status | partial |
| 実施日 | 2026-07-01 |
| 実施 step | RECOVERY-001-B |
| 実装概要 | Next.js deterministic profile fallback を Space Invaders 固有名から generic interactive challenge scaffold へ寄せ、fallback 使用時に `deterministic_scaffold_recovery` event を出すようにした。auto repair 後の continuation が repair target に沿わない場合は `profile_auto_repair_continuation_incomplete` として完了扱いせず、bounded profile repair へ戻す。eval contract/source semantic oracle では interactive app/game の `empty_output` / `scaffold_only` / `static_title_only` / `style_only` / `docs_only` / `manifest_only` を forbidden minimal output として扱う。 |
| source parity 判断 | source と差分あり。source の playable fallback/smoke pipeline は丸ごと移植せず、MVP の profile auto repair、RuntimeAcceptanceReport、eval oracle へ薄く写像した。 |
| 証跡 | `mvp/anvilminimal/src/planner/runner.rs` unit test `deterministic_profile_fallback_requires_targeted_continuation_before_success`。`mvp/anvilminimal/src/minimal_loop/evidence.rs` unit tests: title/scaffold/style/docs-only rejection、generic interactive capability evidence。`mvp/anvilminimal/tests/eval/test_source_semantic_oracle.py` minimal output negative cases。 |
| 通過した確認 | unit / fixture / targeted eval / full eval。`cargo test --manifest-path mvp/anvilminimal/Cargo.toml`、`pytest mvp/anvilminimal/tests/eval` 通過。 |
| 未確認事項 | manual UAT、browser/interaction evidence、source/MVP normalized trace diff は未実施。source の smoke-test.mjs 相当は今回 deterministic source semantic / runtime acceptance evidence で代替し、実ブラウザ smoke は REC-006/REC-010 側に残る。 |
| blocking condition | release-level browser/UAT evidence と source/MVP trace diff 未実施。 |
| 次の確認タイミング | manual UAT 後 / REC-006 browser gate 後 / REC-007 trace diff 後 |
| rollback 判断 | rollback 不要 |

---

## REC-004: TaskContract-lite obligation recovery

| 項目 | 内容 |
| --- | --- |
| 種別 | `simplified_risk` |
| 優先度 | P1 |
| 関連 gate | G-S01, G-S02, G-S11 |
| 影響 mode | all |
| 影響 profile | all |

### source refs

- `/Users/maenokota/share/work/github_kewton/Anvil-develop/src/agent/loop_run/task_contract.rs`
- `/Users/maenokota/share/work/github_kewton/Anvil-develop/src/agent/loop_run/task_contract_core.rs`
- `/Users/maenokota/share/work/github_kewton/Anvil-develop/src/agent/loop_run/task_contract_completion_policy.rs`
- `/Users/maenokota/share/work/github_kewton/Anvil-develop/src/agent/loop_run/task_contract_artifact_contract.rs`
- `/Users/maenokota/share/work/github_kewton/Anvil-develop/src/agent/loop_run/task_contract_recovery_planning.rs`
- `/Users/maenokota/share/work/github_kewton/Anvil-develop/src/agent/loop_run/evidence_binding.rs`

### MVP refs

- `/Users/maenokota/share/work/github_kewton/Anvil-develop/mvp/anvilminimal/src/minimal_loop/completion.rs`
- `/Users/maenokota/share/work/github_kewton/Anvil-develop/mvp/anvilminimal/src/minimal_loop/evidence.rs`
- `/Users/maenokota/share/work/github_kewton/Anvil-develop/mvp/anvilminimal/src/minimal_loop/repair_target.rs`
- `/Users/maenokota/share/work/github_kewton/Anvil-develop/mvp/anvilminimal/src/planner/runner.rs`

### 現状

MVP は `CompletionContract.required_obligations` と evidence を導入し、setup/scaffold/style/docs-only を completion から外す fixture を持つ。ただし source の artifact identity、behavior coverage、recovery target planning の全体は未移植。

### 問題点

- 「何を作るべきか」と「どの artifact がそれを満たしたか」の binding が source より弱い。
- missing obligation から次の repair target へ戻す分類が限定的。
- source trace が無いため、どの obligation が source と違うかを gate で断定できていない。

### 根本原因

full TaskContract を避ける方針は妥当だが、TaskContract が担っていた artifact identity / behavior obligation / recovery planning を MVP の薄い型へ写像し切れていない。

### 受入条件

- app/game request に docs-only output を出しても success にならない。
- missing implementation obligation は concrete artifact repair target に変換される。
- behavior coverage が required capability と artifact evidence に紐づく。
- source/MVP trace diff で task contract stage の差分が説明できる。
- `TaskContract-lite` が扱わない source obligation は `intentionally_different` または `deferred` として記録し、暗黙に pass しない。
- artifact identity が曖昧な場合は success ではなく target discovery / repair planning に落ちる。

### 実装結果

| 項目 | 内容 |
| --- | --- |
| status | partial |
| 実施日 | 2026-07-01 |
| 実施 step | RECOVERY-001-B |
| 実装概要 | `RuntimeAcceptanceReport` に `capability_evidence_bindings` と `obligation_repair_targets` を追加し、required capability ごとに required/satisfied/missing evidence と artifact paths を出すようにした。missing implementation obligation は `src/app/page.tsx` などの concrete target path へ写像し、completion / plan final contract / ultra final acceptance event と repair prompt expected paths へ伝播する。 |
| source parity 判断 | source と差分あり。full TaskContract graph は移植せず、MVP の CompletionContract / RuntimeAcceptanceReport / RepairTarget へ obligation tracking を薄く写像した。 |
| 証跡 | `mvp/anvilminimal/src/minimal_loop/evidence.rs` unit tests: `required_capability_maps_to_expected_artifact_evidence`, `missing_capability_binding_points_at_partial_artifact_evidence`, explicit implementation obligation repair target。`mvp/anvilminimal/src/minimal_loop/repair_target.rs` unit test: missing implementation obligation target classification。`mvp/anvilminimal/src/planner/runner.rs` plan final contract event assertions for `capability_evidence_bindings` / `obligation_repair_targets`。 |
| 通過した確認 | unit / fixture / targeted eval / full eval。`cargo test --manifest-path mvp/anvilminimal/Cargo.toml`、`pytest mvp/anvilminimal/tests/eval` 通過。 |
| 未確認事項 | source/MVP trace diff は未実施。source の ArtifactRole/RecoveryTargetHint 全体、docs/data/research の詳細 obligation は今回対象外で deferred。 |
| blocking condition | source/MVP trace diff と unsupported source obligation の明示的 deferred/intentionally_different 整理が未実施。 |
| 次の確認タイミング | REC-007 trace diff 後 / full recovery status 更新時 |
| rollback 判断 | rollback 不要 |

---

## REC-005: repair targeting / follow-through

| 項目 | 内容 |
| --- | --- |
| 種別 | `semantic_drift` |
| 優先度 | P1 |
| 関連 gate | G-S10, G-S13 |
| 影響 mode | minimal-loop, plan-run, ultra-plan-run |
| 影響 profile | all |

### source refs

- `/Users/maenokota/share/work/github_kewton/Anvil-develop/src/agent/loop_run/verifier_repair_targeting.rs`
- `/Users/maenokota/share/work/github_kewton/Anvil-develop/src/agent/loop_run/repair_lifecycle.rs`
- `/Users/maenokota/share/work/github_kewton/Anvil-develop/src/agent/loop_run/verifier_driver.rs`
- `/Users/maenokota/share/work/github_kewton/Anvil-develop/src/agent/minimal_step_runner/repair.rs`

### MVP refs

- `/Users/maenokota/share/work/github_kewton/Anvil-develop/mvp/anvilminimal/src/minimal_loop/repair_target.rs`
- `/Users/maenokota/share/work/github_kewton/Anvil-develop/mvp/anvilminimal/src/planner/repair.rs`
- `/Users/maenokota/share/work/github_kewton/Anvil-develop/mvp/anvilminimal/src/planner/runner.rs`
- `/Users/maenokota/share/work/github_kewton/Anvil-develop/mvp/anvilminimal/src/minimal_loop/evidence.rs`

### 現状

MVP は no-change repair / target-misdirected repair / handoff を分類し始めた。ただし eval では `verify_repair_no_change` が残っている。

### 問題点

- repair が target に沿った artifact を変更したかの判定が source ほど強くない。
- bounded repair exhausted 後の handoff は保存されるが、その handoff が実際に回復できるかは gate になっていない。
- diagnostic-before-safe-stop や fresh read / target discovery 相当が薄い。

### 根本原因

source の repair は failure -> target -> allowed action -> follow-through -> rerun -> handoff の lifecycle で成立している。MVP は failure kind と handoff は増えたが、repair target を成果物更新と verifier rerun に結びつける controller semantics が不足している。

### 受入条件

- missing entrypoint repair が expected artifact を作る。
- no-change repair は retry ではなく no-change として分類される。
- target not followed は別 failure kind になる。
- bounded repair exhausted で recovery `.md` と `.yaml` が保存される。
- 保存された recovery YAML を使った manual/fixture recovery 成功可否を gate に入れる。
- repair が実行された場合、before/after changed paths と target relation を event に残す。
- repair が verifier を弱める、package script を no-op 化する、または unrelated artifact だけを変更する場合は success にしない。

### 実装結果

| 項目 | 内容 |
| --- | --- |
| status | pass |
| 実施日 | 2026-07-01 |
| 実施 step | RECOVERY-001-C / REC-005 |
| 実装概要 | MVP の repair lifecycle を failure -> `RepairTarget` -> allowed action -> repair turn changed paths -> target relation -> verifier rerun -> failure handoff の薄い写像へ寄せた。`no_change` は即座に `verify_repair_no_change` として分類し、target artifact を外した変更は `repair_target_not_followed`、unrelated artifact だけの変更は `repair_unrelated_change` として success にしない。repair 実行時は before/after changed paths、repair turn changed paths、target relation、allowed action を event に残す。bounded repair exhausted では recovery `.md` と recovery UltraPlan `.yaml` を保存するが、保存自体を success にはしない。 |
| source parity 判断 | source と差分あり。source の full `RepairJob` / diagnostic-before-safe-stop / fresh read graph は丸ごと移植せず、MVP の `RepairTarget` と runner event に source semantics を写像した。 |
| 証跡 | `mvp/anvilminimal/src/minimal_loop/repair_target.rs` unit tests: `target_not_followed` / `unrelated_change` / no-change follow-through classification。`mvp/anvilminimal/src/planner/runner.rs` unit tests: `step_repair_missing_entrypoint_followthrough_creates_expected_artifact`、`step_repair_no_change_is_classified_and_handoff_saved`、`step_repair_target_not_followed_is_classified_and_handoff_saved`、`step_repair_unrelated_change_is_classified_and_handoff_saved`、`saved_recovery_ultra_plan_can_drive_fixture_recovery_success`。`mvp/anvilminimal/src/minimal_loop/loop_run.rs` unit test: verify repair no-change handoff and recovery yaml saved。`mvp/anvilminimal/tests/eval/test_failure_classification.py`、`test_failure_snapshot_classification.py`、`test_runtime_scoring.py`: repair target relation taxonomy / scoring。 |
| 通過した確認 | unit / fixture / targeted eval / full eval。`cargo test --manifest-path mvp/anvilminimal/Cargo.toml` は 409 passed。`pytest -q mvp/anvilminimal/tests/eval` は 202 passed, 1 skipped, 65 subtests passed。 |
| 未確認事項 | manual TUI recovery UAT と source/MVP same-condition normalized trace diff は未実施。REC-005 の acceptance は fixture recovery gate で確認済みとし、manual/TUI evidence は REC-010、source trace diff は REC-007 で扱う。 |
| blocking condition | none |
| 次の確認タイミング | REC-007 source/MVP trace diff 後 / REC-010 manual TUI recovery UAT 後 / release gate 判定時 |
| rollback 判断 | rollback 不要。残リスクは full source `RepairJob` を移植しないことによる診断力の差で、MVP では target relation taxonomy、event evidence、recovery YAML fixture gate で管理する。 |

---

## REC-006: final acceptance / browser readiness / interaction evidence

| 項目 | 内容 |
| --- | --- |
| 種別 | `new_acceptance_gate` / `semantic_drift` |
| 優先度 | P1 |
| 関連 gate | G-S12, G-S16 |
| 影響 mode | plan-run, ultra-plan-run, TUI |
| 影響 profile | nextjs, interactive app/game |

### source refs

- `/Users/maenokota/share/work/github_kewton/Anvil-develop/src/agent/loop_run/verifier.rs`
- `/Users/maenokota/share/work/github_kewton/Anvil-develop/src/agent/loop_run/verifier_driver.rs`
- `/Users/maenokota/share/work/github_kewton/Anvil-develop/src/agent/loop_run/task_contract_completion_policy.rs`
- `/Users/maenokota/share/work/github_kewton/Anvil-develop/src/tools/bash.rs`

### MVP refs

- `/Users/maenokota/share/work/github_kewton/Anvil-develop/mvp/anvilminimal/src/minimal_loop/evidence.rs`
- `/Users/maenokota/share/work/github_kewton/Anvil-develop/mvp/anvilminimal/src/minimal_loop/completion.rs`
- `/Users/maenokota/share/work/github_kewton/Anvil-develop/mvp/anvilminimal/src/planner/runner.rs`
- `/Users/maenokota/share/work/github_kewton/Anvil-develop/mvp/anvilminimal/scripts/eval_lib/browser_oracle.py`
- `/Users/maenokota/share/work/github_kewton/Anvil-develop/mvp/anvilminimal/scripts/eval_lib/parity_gate.py`

### 現状

MVP は browser/interaction evidence を release gate に接続し始めた。UAT では HTTP 500 と canvas unavailable を検出している。

### 問題点

- browser readiness は source に完全同等の汎用 gate が薄いため、source parity ではなく MVP release gate として扱う必要がある。
- 通常 TUI 実行では browser check が常に実行可能とは限らず、unavailable / failed / passed の扱いをさらに安定化する必要がある。
- release evidence content validation は強化済みだが、manual UAT で pass する状態にはまだ至っていない。

### 根本原因

source は build/verifier と dev script contract に寄っており、browser-level acceptance は薄い。MVP では実成果物品質を重視するため、source parity とは別の acceptance gate として設計・運用する必要がある。

### 受入条件

- browser unavailable は partial、HTTP 500 は fail。
- build-only / title-only / canvas unavailable は full success にならない。
- interactive app/game では route render と basic interaction evidence が release full pass 条件になる。
- TUI summary に full success / partial / incomplete が区別して表示される。
- browser evidence JSON が壊れている、`ok=false`、HTTP 4xx/5xx、canvas/interactive surface missing の場合は full pass にしない。
- browser check を実行できない環境では release full pass ではなく partial に固定する。

### 実装結果

| 項目 | 内容 |
| --- | --- |
| status | pass |
| 実施日 | 2026-07-01 |
| 実施 step | RECOVERY-001-D / REC-006 |
| 実装概要 | 通常 plan-run / ultra-plan-run の final acceptance に browser/interaction evidence content gate を接続した。Next.js interactive app/game では static capability evidence と route render / basic interaction evidence の両方を確認し、browser unavailable は `partial`、HTTP 4xx/5xx・`ok=false`・malformed JSON・canvas unavailable・interactive surface missing は full success 不可にした。TUI/run summary には `Final acceptance: full_success` / `partial` / `incomplete` を出し、event には `final_acceptance_status`、browser readiness status、interaction evidence status を残す。 |
| source parity 判断 | intentionally different。source は build/verifier と deterministic completion authority に寄るが、MVP では release-grade acceptance として browser/interaction evidence gate を追加する。runtime に LLM judge は入れず、保存済み JSON evidence と source/static evidence の deterministic 判定に限定した。 |
| 証跡 | `mvp/anvilminimal/src/minimal_loop/evidence.rs` unit tests: browser unavailable inconclusive、route+interaction evidence pass、HTTP 500 fail、canvas unavailable fail。`mvp/anvilminimal/src/planner/runner.rs` unit tests: partial release gate、HTTP 500 failure、browser ready without interaction partial、browser+interaction pass、browser ok without render detail partial、canvas unavailable failure。`mvp/anvilminimal/tests/eval/test_browser_interaction_oracle.py`: adapter unavailable、HTTP 500、ready route without render unavailable、saved evidence pass、ok-only unavailable、canvas unavailable failure。`mvp/anvilminimal/tests/eval/test_parity_gate_report.py`: release evidence content validation、malformed JSON rejection、ok-only partial、canvas unavailable blocker。 |
| 通過した確認 | unit / fixture / targeted eval / full eval。`cargo test --manifest-path mvp/anvilminimal/Cargo.toml --quiet` は 414 passed。`pytest -q mvp/anvilminimal/tests/eval` は 206 passed, 1 skipped, 65 subtests passed。 |
| 未確認事項 | live Playwright/browser UAT と source/MVP same-condition trace diff は未実施。browser を通常 unit test 必須依存にしない制約に従い、REC-006 では保存済み evidence JSON の内容 gate で確認した。manual live evidence は REC-010、source trace diff は REC-007 で扱う。 |
| blocking condition | none |
| 次の確認タイミング | REC-007 source/MVP trace diff 後 / REC-010 manual TUI browser UAT 後 / release gate 判定時 |
| rollback 判断 | rollback 不要。残リスクは live browser 実行環境依存で、通常 test では adapter unavailable を partial として扱い、release gate で evidence content を必須化することで管理する。 |

---

## REC-007: source/MVP trace diff gate completion

| 項目 | 内容 |
| --- | --- |
| 種別 | `verification_gap` |
| 優先度 | P1 |
| 関連 gate | G-S01〜G-S16 |
| 影響 mode | all |
| 影響 profile | all |

### source refs

- `/Users/maenokota/share/work/github_kewton/Anvil-develop/src/agent/minimal_step_runner.rs`
- `/Users/maenokota/share/work/github_kewton/Anvil-develop/src/agent/minimal_step_runner/profile.rs`
- `/Users/maenokota/share/work/github_kewton/Anvil-develop/src/agent/minimal_step_runner/repair.rs`
- `/Users/maenokota/share/work/github_kewton/Anvil-develop/src/agent/loop_run/*.rs`

### MVP refs

- `/Users/maenokota/share/work/github_kewton/Anvil-develop/mvp/anvilminimal/src/eval_events.rs`
- `/Users/maenokota/share/work/github_kewton/Anvil-develop/mvp/anvilminimal/scripts/eval-trace.py`
- `/Users/maenokota/share/work/github_kewton/Anvil-develop/mvp/anvilminimal/scripts/eval_lib/runtime_trace.py`
- `/Users/maenokota/share/work/github_kewton/Anvil-develop/workspace/mvp/eval/022/source_mvp_trace_manifest.md`

### 現状

normalized trace writer はあるが、gate matrix では全 G-S01〜G-S16 が `partial`。source same-condition trace や latest manual trace の登録が不足している。

### 問題点

- 「実装したか」ではなく「source と同じ状態遷移か」を機械判定できていない。
- code reference だけで pass にできないため、修正後も漏れが残る可能性が高い。

### 根本原因

過去の移植は機能一覧ベースで進み、trace-based parity gate が後追いになった。これにより、部品追加後も lifecycle の断絶が残った。

### 受入条件

- MVP/anvildev same-condition eval の normalized trace diff を生成する。
- G-S01〜G-S16 の各 gate が pass / fail / intentionally_different のいずれかへ落ちる。
- `partial` は「検証未了」として残作業扱いにする。
- release gate では browser/interaction/TUI evidence content を含める。
- trace には raw prompt、API key、provider raw response body を保存しない。
- source trace が取得できない場合は code reference だけで pass にしない。

### 実装結果

| 項目 | 内容 |
| --- | --- |
| status | pass |
| 実施日 | 2026-07-01 |
| 実施 step | RECOVERY-001-E |
| 実装概要 | `runtime_trace.py` の normalized stage mapping と `compare_trace_reports` を拡張し、source/MVP same-condition trace diff が G-S01〜G-S16 の各 gate を `pass` / `fail` / `intentionally_different` のいずれかへ解決するようにした。`parity_gate.py` と `eval-preflight.py` に source/MVP trace report / precomputed diff 入力を追加し、comparative/release gate では `partial_gate_ids` を通過不能にした。 |
| source parity 判断 | source と差分あり。0229 normalized diff では G-S02/G-S03/G-S08/G-S14 は pass、G-S01/G-S04/G-S05/G-S06/G-S07/G-S09/G-S10/G-S11/G-S13/G-S15 は source trace 側未観測で fail、G-S12 は browser/interaction evidence failure で release fail、G-S16 は trace 未観測かつ TUI evidence failure で fail。 |
| 証跡 | `workspace/mvp/eval/022/runtime-semantics-trace-diff.json`、`workspace/mvp/eval/022/parity_gate_report.json`、`workspace/mvp/eval/022/runtime_semantics_gate_matrix.md`、`workspace/mvp/eval/022/source_mvp_trace_manifest.md`、source trace `/private/tmp/anvilminimal-eval-0229-anvildev-net-timeout/runtime-semantics-trace-report.json`、MVP trace `/private/tmp/anvilminimal-eval-0229-mvp-net-timeout/runtime-semantics-trace-report.json`。 |
| 通過した確認 | fixture / eval tooling / anvildev comparison report。`pytest -q mvp/anvilminimal/tests/eval/test_runtime_semantics_trace.py mvp/anvilminimal/tests/eval/test_parity_gate_report.py mvp/anvilminimal/tests/eval/test_eval_cli_contract.py` 通過。`pytest mvp/anvilminimal/tests/eval` 通過。 |
| 未確認事項 | live 再計測は実施せず、既存 0229 run root を新 normalizer で再生成した。browser/interaction/TUI の内容は保存済み evidence を読み、HTTP 500 / canvas unavailable / TUI failure として release fail に反映済み。 |
| blocking condition | none。REC-007 の gate 機構は完了。残る release failure は各 gate の `failed_gate_ids` と後続 REC-010/manual UAT の対象。 |
| 次の確認タイミング | full eval 後 / REC-010 manual UAT 後 / release gate 判定時 |
| rollback 判断 | rollback 不要 |

---

## REC-008: provider live probe parity

| 項目 | 内容 |
| --- | --- |
| 種別 | `verification_gap` |
| 優先度 | P2 |
| 関連 gate | G-S07, G-S15 |
| 影響 mode | all |
| 影響 profile | all |

### source refs

- `/Users/maenokota/share/work/github_kewton/Anvil-develop/src/ollama/client.rs`
- `/Users/maenokota/share/work/github_kewton/Anvil-develop/src/ollama/xml_fallback.rs`
- `/Users/maenokota/share/work/github_kewton/Anvil-develop/src/agent/minimal_step_runner.rs`
- `/Users/maenokota/share/work/github_kewton/Anvil-develop/src/agent/minimal_step_runner/repair.rs`

### MVP refs

- `/Users/maenokota/share/work/github_kewton/Anvil-develop/mvp/anvilminimal/src/providers/openai.rs`
- `/Users/maenokota/share/work/github_kewton/Anvil-develop/mvp/anvilminimal/src/providers/gemini.rs`
- `/Users/maenokota/share/work/github_kewton/Anvil-develop/mvp/anvilminimal/src/providers/ollama.rs`
- `/Users/maenokota/share/work/github_kewton/Anvil-develop/mvp/anvilminimal/src/tools/args_recovery.rs`
- `/Users/maenokota/share/work/github_kewton/Anvil-develop/mvp/anvilminimal/tests/live_provider.rs`

### 現状

MVP は provider parser fixture と tool args recovery を持つ。REC-008 では、OpenAI/Gemini/Ollama の provider probe metadata を suite に明示し、provider probe JSONL summary に provider 別の観測結果と tool args recovery classification を保存するようにした。live probe は API key がある場合だけ実行し、API key が無い場合は skip event として扱い、通常 test failure にはしない。

### 問題点

- fake client fixture だけでは、Gemini/OpenAI/Ollama の実レスポンス形状変化を保証できない。
- prompt-sensitive fix を入れた時、provider live behavior が悪化しても通常 test では検出できない。

### 根本原因

provider-specific instability は runtime success と混同すべきではないが、修正前後の不確実性を下げる probe として gate に接続する必要がある。

### 受入条件

- API key がある場合のみ live probe を実行する。
- API key が無い場合は skip として記録し、通常 test failure にしない。
- probe 結果は success rate ではなく schema/tool args/repair prompt の観測として summary に入る。
- provider-sensitive 修正では provider probe 要否を必須記載する。
- unsafe path/workspace confinement 違反は recovery せず拒否する。
- recoverable tool args と unsafe tool args の分類を provider 別に記録する。

### 実装結果

| 項目 | 内容 |
| --- | --- |
| status | pass |
| 実施日 | 2026-07-01 |
| 実施 step | RECOVERY-001-F / REC-008 |
| 実装概要 | `mvp-provider-smoke.yaml` に provider-sensitive fix 用の probe requirement を残し、OpenAI tool args shape、Gemini function calling/schema、Ollama XML fallback/tool-like output に加えて、provider 別 `tool_args_recovery_classification` を記録するようにした。`eval-run.py` は provider probe JSONL を runtime success ではなく separate summary として集計し、provider ごとの observed probes、recoverable args、unsafe args rejection を `provider_probe_summary.json` に残す。`live_provider.rs` は live probe を `ANVIL_PROVIDER_PROBE=1` かつ API key ありの時だけ実行し、fixture では recoverable alias args が実行でき、`../secret.txt` のような unsafe path は `path_confinement_error` かつ non-recoverable として拒否されることを provider 別に確認する。 |
| source parity 判断 | source と差分あり。source は Ollama native tools / XML fallback を runtime loop 内で扱うが、MVP は OpenAI/Gemini/Ollama を薄い provider 実装と probe gate に分ける。provider probe は runtime success ではなく、prompt/tool-call/provider-sensitive fix の不確実性を下げる gate として扱う。 |
| 証跡 | `mvp/anvilminimal/tests/live_provider.rs`: OpenAI/Gemini live probe skip/live execution guard、Ollama XML fallback probe、provider 別 recoverable/unsafe tool args classification。`mvp/anvilminimal/eval/suites/mvp-provider-smoke.yaml`: `required_for_prompt_sensitive_fix: true` と provider 別 observes/classifies metadata。`mvp/anvilminimal/scripts/eval-run.py`: provider probe summary の `probes_by_provider` / `tool_args_recovery_classifications` / `unsafe_tool_args_rejected` / `recoverable_tool_args_classified`。`mvp/anvilminimal/tests/eval/test_eval_cli_contract.py`: provider suite metadata と summary roundtrip。 |
| 通過した確認 | `cargo test --manifest-path mvp/anvilminimal/Cargo.toml --test live_provider provider_probe -- --nocapture` は 5 passed。`pytest -q mvp/anvilminimal/tests/eval/test_eval_cli_contract.py` は 6 passed。`pytest -q mvp/anvilminimal/tests/eval` は 212 passed, 1 skipped, 65 subtests passed。`cargo test --manifest-path mvp/anvilminimal/Cargo.toml` は通過。 |
| 未確認事項 | 実 API/network を使う OpenAI/Gemini live provider smoke はこの作業では実行していない。API key がある release/targeted provider probe では、実レスポンスの tool args shape / function schema を再観測する。 |
| blocking condition | none |
| 次の確認タイミング | provider-sensitive prompt/tool-call 修正後 / release provider smoke 後 / source/MVP same-condition eval 再計測時 |
| rollback 判断 | rollback 不要。残リスクは provider 実サービスの schema drift で、通常 unit test では必須にせず provider probe summary と skip/fail classification で管理する。 |

---

## REC-009: deterministic verify / planner repair normalization

| 項目 | 内容 |
| --- | --- |
| 種別 | `semantic_drift` |
| 優先度 | P2 |
| 関連 gate | G-S03, G-S08 |
| 影響 mode | step-plan, plan-run, ultra-plan-run |
| 影響 profile | all |

### source refs

- `/Users/maenokota/share/work/github_kewton/Anvil-develop/src/agent/loop_run/verifier_command_policy.rs`
- `/Users/maenokota/share/work/github_kewton/Anvil-develop/src/agent/loop_run/verifier.rs`
- `/Users/maenokota/share/work/github_kewton/Anvil-develop/src/agent/loop_run/verifier_driver.rs`
- `/Users/maenokota/share/work/github_kewton/Anvil-develop/src/agent/minimal_step_runner.rs`

### MVP refs

- `/Users/maenokota/share/work/github_kewton/Anvil-develop/mvp/anvilminimal/src/planner/verify.rs`
- `/Users/maenokota/share/work/github_kewton/Anvil-develop/mvp/anvilminimal/src/planner/lint.rs`
- `/Users/maenokota/share/work/github_kewton/Anvil-develop/mvp/anvilminimal/src/planner/step_plan.rs`
- `/Users/maenokota/share/work/github_kewton/Anvil-develop/mvp/anvilminimal/src/planner/runner.rs`

### 現状

safe `&&` split など deterministic normalization は追加済み。ただし latest matrix では smoke/source trace 再計測が未完で、OpenAI verify policy 違反も残る。

### 問題点

- planner が荒い verify を出した時、provider retry と deterministic repair の責任境界がまだ不明瞭。
- unsafe shell control を緩めず、safe normalization だけを使う gate が必要。

### 根本原因

source では verifier policy と setup boundary が明確に分かれる。MVP は StepPlan/UltraPlan YAML の生成品質と runtime verifier の境界で policy error が出やすい。

### 受入条件

- safe split は planner normalization 専用であり、runtime Bash policy は緩めない。
- unsafe verify は具体 failure kind で fail-fast。
- dependency setup before build の plan order が具体分類される。
- step-plan score と plan-run failure の相関で false positive が減る。
- verify normalization が行われた場合は normalized command list と original command hash/summary を event に残す。
- provider retry で直った case と deterministic normalization で直った case を区別して集計する。

---

## REC-010: TUI/manual run observability completeness

| 項目 | 内容 |
| --- | --- |
| 種別 | `verification_gap` / `semantic_drift` |
| 優先度 | P2 |
| 関連 gate | G-S16 |
| 影響 mode | TUI, ultra-plan-run |
| 影響 profile | all |

### source refs

- `/Users/maenokota/share/work/github_kewton/Anvil-develop/src/agent/loop_run/actor_loop_flow.rs`
- `/Users/maenokota/share/work/github_kewton/Anvil-develop/src/agent/minimal_step_runner.rs`
- `/Users/maenokota/share/work/github_kewton/Anvil-develop/src/agent/minimal_step_runner/repair.rs`

### MVP refs

- `/Users/maenokota/share/work/github_kewton/Anvil-develop/mvp/anvilminimal/src/eval_events.rs`
- `/Users/maenokota/share/work/github_kewton/Anvil-develop/mvp/anvilminimal/src/planner/runner.rs`
- `/Users/maenokota/share/work/github_kewton/Anvil-develop/mvp/anvilminimal/src/tui`
- `/Users/maenokota/share/work/github_kewton/Anvil-develop/mvp/anvilminimal/src/lib.rs`

### 現状

TUI/manual run events と summary は改善済み。UAT では incomplete と recovery handoff は確認できた。

### 問題点

- manual UAT trace が gate manifest に十分登録されていない。
- TUI の表示が「未完了」「partial artifact」「recovery next action」をどれだけ明確に出すかは継続検証が必要。

### 根本原因

過去は eval 用 event と手動実行ログが分かれていた。改善は入ったが、release gate と manual UAT の証跡管理がまだ運用まで閉じていない。

### 受入条件

- TUI run で `.anvil/runs/<run-id>/events.jsonl` と `summary.md` が必ず残る。
- failed phase / pending phase / recovery `.md` / recovery `.yaml` / suggested command が summary と画面に出る。
- silent exit は gate failure。
- recovery command が提示された場合、対象 `.md` / `.yaml` が存在し parse 可能であることを fixture で確認する。
- manual UAT trace を `source_mvp_trace_manifest.md` または対応する release evidence report に登録する。

## 対象外または慎重扱い

| 項目 | 理由 |
| --- | --- |
| source の full `TaskContract` graph 丸ごと移植 | MVP の小さい API 境界を壊すリスクが高い。必要な semantics を薄く写像する。 |
| source の full `RepairJob` 丸ごと移植 | repair complexity が急増する。`RepairTarget` / `CompletionContract` / `RuntimeAcceptanceReport` の拡張で足りるかを先に検証する。 |
| runtime LLM judge | 不安定化とコスト増。deterministic evidence / browser oracle / provider probe に留める。 |
| Space Invaders 固有判定 | 過適応になる。interactive game/web app capability として扱う。 |

## 次アクション案

1. REC-001 と REC-002 を同一 recovery plan にする。
   - 理由: UAT の直接 failure が `tailwindcss` missing と browser 500 であり、setup/build/browser lifecycle が一体で壊れているため。
2. REC-003 と REC-004 を次の plan にする。
   - 理由: dependency が直っても、ゲーム品質が浅ければ release acceptance に届かないため。
3. REC-007 を並行 gate として維持する。
   - 理由: 以降の修正を「また部品追加だけで完了」と誤判定しないため。
4. REC-005 を REC-001/002 の後に必ず確認する。
   - 理由: setup/build failure を検知しても、repair が target に沿って成果物を変えられなければ実成果物品質は上がらないため。
5. REC-008/009/010 は横断 gate として各実装修正に付随させる。
   - 理由: provider、verify policy、TUI evidence は単独修正ではなく、各 lifecycle 修正の回帰検出として機能させる必要がある。
