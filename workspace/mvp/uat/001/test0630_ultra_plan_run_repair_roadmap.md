# test0630_001 Ultra Plan Run Repair Roadmap

作成日: 2026-06-30

参照元:

- `workspace/mvp/uat/001/test0630_ultra_plan_run_quality_root_cause.md`

## 目的

`test0630_001` で露出した以下の問題を、実装可能な作業ロードマップへ落とす。

- `ultra-plan-run` が phase 4 の StepPlan scaffold で停止し、成果物が未完了のまま残る。
- verify/planner failure 時に `.md` の recovery prompt は保存されるが、手動で確認・編集・再実行できる recovery UltraPlan YAML が保存されない。
- `npm run build` は通るが、browser readiness では `/` が HTTP 500 になる。
- app/game の expected artifact が作られただけで step が完了し、playable capability が不足する。
- release gate が evidence path の有無に寄っており、browser evidence の `ok=false` / HTTP 500 を十分に fail 扱いできていない。

## 基本方針

- Space Invaders 固有の文字列対応にはしない。
- Next.js interactive app / game の一般的な runtime contract として扱う。
- success を甘くして成功率を上げない。false positive を減らす方向を優先する。
- recovery は無制限自動再実行にしない。bounded repair と明示的 handoff を維持する。
- `.md` の診断 prompt と `.yaml` の recovery plan は役割を分ける。
- eval だけでなく通常 TUI 実行でも、未完了状態と次の回復手段が見えるようにする。
- source の大型機構を丸ごと移植せず、既存の `VerificationReport`、`RuntimeAcceptanceReport`、`RepairContext`、`UltraPlan` 保存処理の薄い拡張で実装する。
- browser readiness は environment-aware にする。browser unavailable は `partial`、browser HTTP 500 は `fail` と分け、通常 unit test の必須依存にしない。
- LLM API に依存する仮説ではなく、現物の events / generated files / browser evidence で再現可能な failure から着手する。prompt/provider 挙動を変える段階では provider probe を別途使う。

## レビュー反映事項

| 観点 | 反映内容 |
| --- | --- |
| 設計思想 | MVP の小さい API 境界を維持し、TaskContract / RepairJob 全体移植は避ける。 |
| 不安定な挙動 | browser gate は availability-aware にし、unit test や通常 TUI 実行を環境依存にしない。 |
| 影響調査 | verify policy、planner lint、step completion、final acceptance、eval gate、TUI summary を横断対象に含める。 |
| 他機能への影響 | non-interactive file task の path-only completion は維持し、interactive app/game のみ強める。 |
| 複雑性 | 新 controller ではなく既存 contract/report/event の拡張で段階的に実装する。 |
| 原因深掘り | phase scaffold failure と runtime/browser failure を分け、Tailwind は version 固定ではなく package/config/dev-server evidence で扱う。 |
| 過適応回避 | Space Invaders 固有 checklist ではなく、interactive app/game capability evidence として一般化する。 |
| 不確実性 | 現段階では LLM API 追加実行は不要。provider-sensitive prompt 変更時のみ live/provider probe を実施する。 |

## Roadmap

### Phase 0: 現状固定と再現 fixture 化

目的:

- `test0630_001` の失敗を再発検知できる fixture / golden evidence に固定する。
- 以降の修正で「正しく失敗を拾えるようになった」のか「単純に劣化した」のかを分ける。

主な作業:

- UAT workspace の観測結果を fixture 化する。
  - phase 4 scaffold failure
  - `verify command may not use shell control syntax`
  - recovery `.md` はあるが recovery `.yaml` はない
  - browser readiness HTTP 500
  - `required_artifacts_satisfied_after_tool` による early stop
- `events.jsonl` から minimal な fixture を作る。
- generated `page.tsx` / `globals.css` の evidence を acceptance fixture に追加する。

編集候補:

- `mvp/anvilminimal/tests/eval/`
- `mvp/anvilminimal/scripts/eval_lib/`
- `workspace/mvp/uat/001/`

受入条件:

- `phase_scaffold_error`、`recovery_prompt_saved`、recovery YAML missing を fixture で検出できる。
- build pass / browser fail の組み合わせを fixture で表現できる。
- fixture が Space Invaders 固有判定に依存しない。

検証:

```bash
pytest mvp/anvilminimal/tests/eval
```

### Phase 1: verify command policy violation の deterministic repair

目的:

- planner が `&&` などを含む荒い verify command を出した時、provider retry だけに依存しない。
- 安全に分解できる command は分解し、分解できない command は具体 failure kind で fail-fast する。

主な作業:

- `diagnose_verify_command` の結果に、repairable / unrepairable を持たせる。
- 例: `npm run build && test -f src/app/page.tsx` を `["npm run build", "test -f src/app/page.tsx"]` に変換する。
- `;`, `||`, redirection, command substitution など unsafe な構文は分解せず拒否する。
- planner corrective retry の前に deterministic normalization を試す。
- normalization 結果を event に出す。
- raw command execution policy は緩めない。分解は planner/StepPlan normalization 専用とし、Bash tool や runtime executor に shell control syntax を許可しない。

編集候補:

- `mvp/anvilminimal/src/planner/verify.rs`
- `mvp/anvilminimal/src/planner/step_plan.rs`
- `mvp/anvilminimal/src/planner/lint.rs`
- `mvp/anvilminimal/src/planner/runner.rs`
- `mvp/anvilminimal/src/eval_events.rs`
- `mvp/anvilminimal/tests/`

受入条件:

- safe な shell control verify は複数 verify command に分解される。
- unsafe な shell control verify は引き続き拒否される。
- rejected case は `verify_command_policy_error` の具体 reason を保持する。
- phase 4 相当の planner output が deterministic repair で valid StepPlan へ進める。
- 既存の `validate_verify_command("... && ...")` は引き続き拒否し、別 API の normalization 結果だけが複数 command として採用される。

検証:

```bash
cargo test --manifest-path mvp/anvilminimal/Cargo.toml verify
pytest mvp/anvilminimal/tests/eval
```

### Phase 2: verify/planner failure 時の recovery UltraPlan YAML 保存

目的:

- `.md` の recovery prompt だけでなく、手動で確認・編集・再実行できる `.anvil/plans/recovery-ultra-plan-*.yaml` を保存する。
- TUI に「未完了」「保存した recovery YAML」「推奨コマンド」を明示する。

主な作業:

- `phase_scaffold_error` / `verify_command_policy_error` / bounded repair exhausted 時に recovery UltraPlan を生成する。
- recovery YAML は既存成果物を preserve し、failed phase 以降に focused recovery phase を置く。
- recovery YAML は success 扱いしない。handoff artifact として扱う。
- `.md` は診断と replan prompt、`.yaml` は実行 plan として分離する。
- TUI / summary に recovery YAML path を表示する。
- recovery YAML は `parse_ultra_plan` で検証できる形式にし、生成不能なら success にせず recovery YAML missing を明示する。

編集候補:

- `mvp/anvilminimal/src/planner/repair.rs`
- `mvp/anvilminimal/src/planner/ultra_plan.rs`
- `mvp/anvilminimal/src/planner/runner.rs`
- `mvp/anvilminimal/src/tui/`
- `mvp/anvilminimal/src/eval_events.rs`

受入条件:

- `phase_scaffold_error` で `.anvil/repairs/repair-*.md` と `.anvil/plans/recovery-ultra-plan-*.yaml` が両方保存される。
- TUI summary に `incomplete`, recovery YAML path, suggested command が表示される。
- recovery YAML は original goal / failed phase / missing capability / verify preference を含む。
- recovery YAML 保存だけでは run success にならない。
- recovery YAML は `render_ultra_plan` / `parse_ultra_plan` の roundtrip に通る。

検証:

```bash
cargo test --manifest-path mvp/anvilminimal/Cargo.toml recovery
pytest mvp/anvilminimal/tests/eval
```

### Phase 3: partial artifact 状態の TUI / run summary 明示

目的:

- ultra-run 失敗時に、ユーザーが途中成果物を完成品と誤認しないようにする。
- completed phase / failed phase / pending phase / next action を通常実行の summary に残す。

主な作業:

- `ultra_phase_failed` 時に run summary を structured にする。
- `.anvil/runs/<run-id>/summary.md` に以下を出す。
  - status: incomplete
  - completed phases
  - failed phase
  - pending phases
  - recovery prompt path
  - recovery YAML path
  - suggested command
- TUI 上でも短い未完了メッセージを出す。

編集候補:

- `mvp/anvilminimal/src/planner/runner.rs`
- `mvp/anvilminimal/src/tui/`
- `mvp/anvilminimal/src/eval_events.rs`

受入条件:

- `test0630_001` 相当の停止で summary が `TUI command failed:` だけにならない。
- completed/pending/failed phase が summary から読める。
- recovery YAML がない場合は gate failure として表示できる。

検証:

```bash
cargo test --manifest-path mvp/anvilminimal/Cargo.toml tui
```

### Phase 4: app/game capability evidence の導出

目的:

- prompt / phase task から、interactive app/game に必要な capability evidence を controller 側へ渡す。
- `required_final_capabilities` があるのに `required_final_evidence` が空、という状態を減らす。

主な作業:

- profile / goal / phase prompt から generic capability evidence を導出する。
- interactive app/game の最低 evidence を定義する。
  - visible interactive surface
  - user input handler
  - stateful update
  - adversary or challenge
  - score/progression
  - failure or collision rule
  - restart or recoverable game state
- evidence は scenario 固有語ではなく、コード構造・DOM/canvas・state transition の観点で見る。
- 静的 evidence だけで満点にしない。browser/interaction evidence がない場合は release-grade では `partial` に留める。

編集候補:

- `mvp/anvilminimal/src/minimal_loop/evidence.rs`
- `mvp/anvilminimal/src/minimal_loop/completion.rs`
- `mvp/anvilminimal/src/planner/runner.rs`
- `mvp/anvilminimal/scripts/eval_lib/plan_capability_contract.py`
- `mvp/anvilminimal/scripts/eval_lib/acceptance_contract.py`

受入条件:

- interactive game task で `required_final_evidence` が空にならない。
- docs-only / style-only / title-only output は acceptance failure になる。
- evidence 導出は Space Invaders 固有語に依存しない。
- evidence 不足は実装 continuation / final acceptance failure として扱い、単なる eval diagnostic に閉じない。

検証:

```bash
cargo test --manifest-path mvp/anvilminimal/Cargo.toml evidence
pytest mvp/anvilminimal/tests/eval
```

### Phase 5: implement step の path-only completion 抑制

目的:

- app/game の実装 step で `required_artifacts_satisfied_after_tool` だけで早期終了しない。
- path existence と capability evidence を分ける。

主な作業:

- app/game profile では completion contract verification を有効化する。
- expected paths が揃っても、capability evidence が不足していれば continuation feedback を出す。
- setup/scaffold/implementation/verify/acceptance の role を使い、setup-only / scaffold-only / style-only を completion から外す。
- 適用条件は profile / required capability / task kind で絞り、単純なファイル作成タスクには従来の path completion を残す。

編集候補:

- `mvp/anvilminimal/src/minimal_loop/loop_run.rs`
- `mvp/anvilminimal/src/minimal_loop/completion.rs`
- `mvp/anvilminimal/src/minimal_loop/evidence.rs`
- `mvp/anvilminimal/src/planner/runner.rs`

受入条件:

- `src/app/page.tsx` が存在するだけでは interactive app/game step が完了しない。
- stateful interaction / challenge / failure rule が不足する場合、continuation feedback が出る。
- non-interactive file creation task では path-only completion の利点を壊さない。

検証:

```bash
cargo test --manifest-path mvp/anvilminimal/Cargo.toml completion
pytest mvp/anvilminimal/tests/eval
```

### Phase 6: Next.js browser readiness を通常 final acceptance に接続

目的:

- Next.js interactive app/game では build-only を合格にしない。
- dev server route / browser render / interaction evidence を final acceptance で扱う。

主な作業:

- final acceptance に browser readiness requirement を接続する。
- browser が使えない環境では full pass にせず `partial` とする。
- `npm run dev -p 3011` の起動、HTTP 200、canvas or interactive DOM、console/page error を evidence 化する。
- sandbox/CI で browser が使えない場合の skip/partial reason を明確にする。
- 通常 unit test は browser を必須にしない。release/UAT gate で browser evidence を必須化する。

編集候補:

- `mvp/anvilminimal/src/planner/runner.rs`
- `mvp/anvilminimal/src/planner/profiles/nextjs.rs`
- `mvp/anvilminimal/scripts/eval_lib/browser_oracle.py`
- `mvp/anvilminimal/scripts/eval_lib/runtime_scoring.py`
- `mvp/anvilminimal/tests/eval/`

受入条件:

- `npm run build` pass / browser HTTP 500 は final acceptance pass にならない。
- browser unavailable は `partial`, browser failed は `fail` と区別される。
- static title-only app は interactive game acceptance を満たさない。
- TUI 実行時に browser check を実行できない場合も、full success と表示しない。

検証:

```bash
pytest mvp/anvilminimal/tests/eval
```

### Phase 7: Tailwind / CSS pipeline の profile verifier 強化

目的:

- `@tailwind` directive を使う Next.js app で、build pass だが dev route fail になるケースを早期に検出する。
- Tailwind を使うなら toolchain 完備、使わないなら plain CSS に寄せる。

主な作業:

- `globals.css` に `@tailwind` がある場合、package / postcss / tailwind config の整合性を検査する。
- Next.js / Tailwind / PostCSS の package/config/dev-server evidence を profile contract として扱う。
- dev server readiness failure を `profile_runtime_contract_error` として分類する。
- 単一 version 固定の matrix にせず、installed dependency と config の整合性を優先する。

編集候補:

- `mvp/anvilminimal/src/planner/profiles/nextjs.rs`
- `mvp/anvilminimal/src/minimal_loop/build_verifier.rs`
- `mvp/anvilminimal/src/minimal_loop/dependency_setup.rs`
- `mvp/anvilminimal/scripts/eval_lib/failure_classification.py`

受入条件:

- `@tailwind` directive と不整合な config/dependency が fixture で fail する。
- plain CSS app は Tailwind toolchain を要求されない。
- dev route HTTP 500 が build success と別 failure kind で集計される。
- exact version pinning ではなく、package/config/runtime error evidence に基づく failure reason が出る。

検証:

```bash
cargo test --manifest-path mvp/anvilminimal/Cargo.toml nextjs
pytest mvp/anvilminimal/tests/eval
```

### Phase 8: release gate evidence content validation

目的:

- release gate が evidence path だけで full pass にならないようにする。
- browser/interaction/TUI evidence の内容を読む。

主な作業:

- `browser-readiness.json` の `ok`, `status`, `http_status`, `page_errors` を gate 判定に使う。
- `interaction-evidence.json` の `ok`, state change, screenshot/canvas evidence を gate 判定に使う。
- TUI events で `tui_command_stop.ok=false` があれば full pass にしない。
- report に failure reason を残す。
- evidence file が壊れている/読めない場合も full pass にしない。

編集候補:

- `mvp/anvilminimal/scripts/eval_lib/parity_gate.py`
- `mvp/anvilminimal/scripts/eval-run.py`
- `mvp/anvilminimal/tests/eval/`

受入条件:

- evidence path が存在しても `ok=false` なら release gate full pass にならない。
- HTTP 500 は release gate fail/partial reason に出る。
- TUI command failed は release evidence 上も未完了扱いになる。
- malformed evidence JSON は `evidence_invalid` として分類される。

検証:

```bash
pytest mvp/anvilminimal/tests/eval
```

### Phase 9: 回帰評価と manual UAT gate

目的:

- 修正が eval だけに過適応していないことを確認する。
- MVP と anvildev の比較だけでなく、実ブラウザ/TUI の成果物品質を確認する。

主な作業:

- MVP 全量 eval を実行する。
- anvildev `--engine minimal` と同条件比較を実行する。
- `test0630_001` 相当の manual UAT を新規 workspace で実施する。
- UAT で以下を確認する。
  - ultra-run が完走、または未完了として明確に停止する。
  - recovery `.md` と recovery `.yaml` が保存される。
  - browser `/` が HTTP 200。
  - canvas or interactive DOM が表示される。
  - keyboard/pointer input で状態が変わる。
  - game task で title-only / style-only にならない。

受入条件:

- MVP の release gate が evidence 内容込みで `pass` または納得できる `partial` になる。
- plan-run / ultra-plan-run の false positive が減る。
- successful artifact の品質が、manual UAT で「ビルド可能」ではなく「実ブラウザで操作可能」と確認できる。
- anvildev 比較で下回る場合は、runtime semantics 差分と intentional difference が report に残る。

検証:

```bash
cargo build --release --manifest-path mvp/anvilminimal/Cargo.toml
pytest mvp/anvilminimal/tests/eval
python3 mvp/anvilminimal/scripts/eval-run.py --suite mvp-smoke --modes minimal-loop,step-plan,plan-run,ultra-plan-run --no-local-llm --parallel 5
```

## 依存関係と実施順

推奨順:

1. Phase 0
2. Phase 1
3. Phase 2
4. Phase 3
5. Phase 4
6. Phase 5
7. Phase 6
8. Phase 7
9. Phase 8
10. Phase 9

理由:

- まず再現 fixture を固定しないと、改善と劣化の区別がつかない。
- verify command policy violation を解消しないと、phase 4 以降へ進めない。
- recovery YAML / TUI summary を先に入れることで、以降の失敗が診断可能になる。
- capability evidence / path-only completion / browser readiness は、false positive を減らす中核であり、順に強める必要がある。
- release gate content validation は、通常実行の改善後に評価として閉じる。

## 完了条件

- `test0630_001` と同種の failure が、少なくとも以下のどちらかになる。
  - valid recovery UltraPlan YAML が保存され、TUI/summary から手動再実行できる。
  - deterministic repair により phase scaffold が通り、final acceptance まで進む。
- Next.js interactive app/game で build-only success が full success にならない。
- required artifacts の存在だけで playable game completion と判定されない。
- browser readiness / interaction evidence の失敗が release gate に反映される。
- UAT summary から、完成 / 未完了 / recovery next action が一目で分かる。

## 主なリスクと対策

| リスク | 影響 | 対策 |
| --- | --- | --- |
| acceptance を強くしすぎて成功率が下がる | eval 上は悪化して見える | correct failure detection と regression を report で分ける |
| browser readiness が環境依存になる | CI / sandbox で不安定になる | browser unavailable は partial、HTTP 500 は fail と分類する |
| verify command repair が unsafe command を通す | セキュリティ・workspace confinement のリスク | 分解対象を allowlist に限定し、redirection/substitution は拒否する |
| recovery YAML 自動実行でループ化する | 無限 recovery / コスト増 | YAML 保存と suggested command に留め、自動再帰は入れない |
| capability evidence が scenario 固有になる | eval 過適応 | generic app/game capability contract として実装する |
