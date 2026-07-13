# Investigation 01: assurance inflation and Run 3 artifact stagnation

- 調査日: 2026-07-14
- 対象リビジョン: `d6342e596471af49fa425d014379129d32221b13`
- 対象: `uat-test0713-data-001` の4 run
- 変更境界: `mvp/anvilminimal/src/`, `tests/`, `docs/` は読み取りのみ。本ファイル以外は、Run 3 の未収集 recovery artifact がある場合だけ追補する。
- 一次データ: 収集済み `workspace/management/runs/uat-test0713-data-001/artifacts/<run>/` を優先し、欠落確認と Run 3 の recovery artifact 同一性確認には元 workspace の `<run>/.anvil/` を用いた。

## 調査①: assurance 疑義

### 契約上の判定基準

`mvp/anvilminimal/docs/data-profile-contract.md` §4 は、`partial` を「パイプライン実行成功＋E1/E3 pass だが E2 または E4 に未達」と定義する。スクリプトが生成されても契約上の実行プローブが完了していなければ `static`、スクリプト未生成・実行失敗・E1違反・再現性違反は `failed` である。

この調査では、単なる生成ステップ内の `python pipeline/main.py` 実行と、§5 の evidence を残す契約上の実行プローブを区別する。4 run とも `pipeline-run.json`, `reconciliation.json`, `claims-binding.json`, `rerun-consistency.json` は全て存在せず、E1/E3 の pass は観測されていない。

### assurance 関連イベントの原文

抽出には次を用いた。

```sh
grep -i '"assurance' events.jsonl
grep -i 'ultra_partial_artifact' events.jsonl
```

以下の JSON は、長大な `stop_reason` と `time_profile` を除き、level の根拠フィールドを `jq` で選択したもの。値は原イベントから変更していない。各 run で `ultra_partial_artifact_summary` は phase の incomplete handoff を記録するが `assurance_level` 自体は持たない。level を最初に投影したのは直後の `tui_command_stop` で、後続の `run_stop` は同じ `partial` / `acceptance_not_full_success` を継承している。

#### Run 1: `data_agg_qwen27_plan_qwen35_exec_preset_profile_001`

`artifacts/data_agg_qwen27_plan_qwen35_exec_preset_profile_001/events.jsonl:53`:

```json
{"completed_phase_ids":[],"event":"ultra_partial_artifact_summary","failed_phase_id":"data-inspection","failure_kind":"phase_execute_error","pending_phase_ids":["data-cleaning","data-aggregation","data-reporting","data-validation"],"recovery_command_targets_valid":true,"recovery_prompt_exists":true,"recovery_prompt_parse_ok":true,"recovery_prompt_path":".anvil/repairs/repair-phase-data-inspection-019f5bff-d645-7060-8f74-b2ad257a8669.md","recovery_ultra_plan_path":".anvil/plans/recovery-ultra-plan-phase-data-inspection-019f5bff-d645-7060-8f74-b2be93aee2b7.yaml","recovery_yaml_exists":true,"recovery_yaml_missing":false,"recovery_yaml_parse_ok":true,"schema_version":"1","status":"incomplete","suggested_recovery_command":"/ultra-plan-run --profile data \"$(cat .anvil/repairs/repair-phase-data-inspection-019f5bff-d645-7060-8f74-b2ad257a8669.md)\"","suggested_recovery_yaml_command":"/run-ultra-plan .anvil/plans/recovery-ultra-plan-phase-data-inspection-019f5bff-d645-7060-8f74-b2be93aee2b7.yaml"}
```

`artifacts/data_agg_qwen27_plan_qwen35_exec_preset_profile_001/events.jsonl:54` (`tui_command_stop`) の根拠フィールド:

```json
{"assurance_level":"partial","assurance_reason":"acceptance_not_full_success","command_completion_state":"failed","completion_status":"incomplete","effective_profile":"data","event":"tui_command_stop","failure_kind":"direct_cli_command_failed","final_acceptance_status":"not_checked","ok":false,"release_gate_status":"not_applicable","runtime_acceptance_status":"not_checked","status":"failed"}
```

同ファイル `:56` の `run_stop` も `assurance_level="partial"`, `assurance_reason="acceptance_not_full_success"` を記録する。

#### Run 2: `data_agg_qwen27_plan_gemma31_exec_preset_profile_001`

`artifacts/data_agg_qwen27_plan_gemma31_exec_preset_profile_001/events.jsonl:98`:

```json
{"completed_phase_ids":[],"event":"ultra_partial_artifact_summary","failed_phase_id":"data-inspection","failure_kind":"phase_execute_error","pending_phase_ids":["data-cleaning","data-aggregation","data-reporting","data-validation"],"recovery_command_targets_valid":true,"recovery_prompt_exists":true,"recovery_prompt_parse_ok":true,"recovery_prompt_path":".anvil/repairs/repair-phase-data-inspection-019f5c06-9c3d-70d0-a482-63c6b6a37e82.md","recovery_ultra_plan_path":".anvil/plans/recovery-ultra-plan-phase-data-inspection-019f5c06-9c3d-70d0-a482-63d9f11d5070.yaml","recovery_yaml_exists":true,"recovery_yaml_missing":false,"recovery_yaml_parse_ok":true,"schema_version":"1","status":"incomplete","suggested_recovery_command":"/ultra-plan-run --profile data \"$(cat .anvil/repairs/repair-phase-data-inspection-019f5c06-9c3d-70d0-a482-63c6b6a37e82.md)\"","suggested_recovery_yaml_command":"/run-ultra-plan .anvil/plans/recovery-ultra-plan-phase-data-inspection-019f5c06-9c3d-70d0-a482-63d9f11d5070.yaml"}
```

`artifacts/data_agg_qwen27_plan_gemma31_exec_preset_profile_001/events.jsonl:99` (`tui_command_stop`) の根拠フィールド:

```json
{"assurance_level":"partial","assurance_reason":"acceptance_not_full_success","command_completion_state":"failed","completion_status":"incomplete","effective_profile":"data","event":"tui_command_stop","failure_kind":"direct_cli_command_failed","final_acceptance_status":"not_checked","ok":false,"release_gate_status":"not_applicable","runtime_acceptance_status":"not_checked","status":"failed"}
```

同ファイル `:101` の `run_stop` も同じ level/reason を記録する。

#### Run 3: `data_agg_qwen27_plan_qwen35_exec_preset_none_001`

`artifacts/data_agg_qwen27_plan_qwen35_exec_preset_none_001/events.jsonl:86`:

```json
{"completed_phase_ids":[],"event":"ultra_partial_artifact_summary","failed_phase_id":"validate-input-data","failure_kind":"phase_execute_error","pending_phase_ids":["compute-sales-aggregations","generate-final-report"],"recovery_command_targets_valid":true,"recovery_prompt_exists":true,"recovery_prompt_parse_ok":true,"recovery_prompt_path":".anvil/repairs/repair-phase-validate-input-data-019f5c0f-c653-7973-a656-c0df14de43ec.md","recovery_ultra_plan_path":".anvil/plans/recovery-ultra-plan-phase-validate-input-data-019f5c0f-c654-7453-87b0-1e7e2929b0a8.yaml","recovery_yaml_exists":true,"recovery_yaml_missing":false,"recovery_yaml_parse_ok":true,"schema_version":"1","status":"incomplete","suggested_recovery_command":"/ultra-plan-run --profile data \"$(cat .anvil/repairs/repair-phase-validate-input-data-019f5c0f-c653-7973-a656-c0df14de43ec.md)\"","suggested_recovery_yaml_command":"/run-ultra-plan .anvil/plans/recovery-ultra-plan-phase-validate-input-data-019f5c0f-c654-7453-87b0-1e7e2929b0a8.yaml"}
```

`artifacts/data_agg_qwen27_plan_qwen35_exec_preset_none_001/events.jsonl:87` (`tui_command_stop`) の根拠フィールド:

```json
{"assurance_level":"partial","assurance_reason":"acceptance_not_full_success","command_completion_state":"failed","completion_status":"incomplete","effective_profile":"data","event":"tui_command_stop","failure_kind":"direct_cli_command_failed","final_acceptance_status":"not_checked","ok":false,"release_gate_status":"not_applicable","runtime_acceptance_status":"not_checked","status":"failed"}
```

同ファイル `:89` の `run_stop` も同じ level/reason を記録する。

#### Run 4: `data_agg_qwen27_plan_gemma31_exec_preset_none_001`

`artifacts/data_agg_qwen27_plan_gemma31_exec_preset_none_001/events.jsonl:70`:

```json
{"completed_phase_ids":[],"event":"ultra_partial_artifact_summary","failed_phase_id":"load-and-define-validation-rules","failure_kind":"phase_execute_error","pending_phase_ids":["filter-invalid-rows-and-categorize-reasons","compute-aggregations-and-totals","generate-summary-report"],"recovery_command_targets_valid":true,"recovery_prompt_exists":true,"recovery_prompt_parse_ok":true,"recovery_prompt_path":".anvil/repairs/repair-phase-load-and-define-validation-rules-019f5c17-caf0-7710-8520-35cb48dc8198.md","recovery_ultra_plan_path":".anvil/plans/recovery-ultra-plan-phase-load-and-define-validation-rules-019f5c17-caf1-7280-8e1d-0fc31416916e.yaml","recovery_yaml_exists":true,"recovery_yaml_missing":false,"recovery_yaml_parse_ok":true,"schema_version":"1","status":"incomplete","suggested_recovery_command":"/ultra-plan-run --profile data \"$(cat .anvil/repairs/repair-phase-load-and-define-validation-rules-019f5c17-caf0-7710-8520-35cb48dc8198.md)\"","suggested_recovery_yaml_command":"/run-ultra-plan .anvil/plans/recovery-ultra-plan-phase-load-and-define-validation-rules-019f5c17-caf1-7280-8e1d-0fc31416916e.yaml"}
```

`artifacts/data_agg_qwen27_plan_gemma31_exec_preset_none_001/events.jsonl:71` (`tui_command_stop`) の根拠フィールド:

```json
{"assurance_level":"partial","assurance_reason":"acceptance_not_full_success","command_completion_state":"failed","completion_status":"incomplete","effective_profile":"data","event":"tui_command_stop","failure_kind":"direct_cli_command_failed","final_acceptance_status":"not_checked","ok":false,"release_gate_status":"not_applicable","runtime_acceptance_status":"not_checked","status":"failed"}
```

同ファイル `:73` の `run_stop` も同じ level/reason を記録する。

### 実装経路の判定

#### data 用の正しい契約写像

- `mvp/anvilminimal/src/planner/profile.rs:711-725` の `DataProfile::behavior_probe` は `profiles::data::runtime::run_manifest_checks` を呼び、data assurance を `ProfileBehaviorProbeReport.status` へ写像する。
- `mvp/anvilminimal/src/planner/profiles/data/runtime.rs:54-89` の `run_manifest_checks` は pipeline probe、results schema、E1 reconciliation、E2 claims binding、E3 rerun consistency を実行して evidence を書く。
- 同 `:92-127` の `assurance_from_evidence` は、`pipeline/main.py` がなければ `failed`、script はあるが `pipeline-run.json` がなければ `static` とする。
- 同 `:185-199` の `classify` は、pipeline/E1/E3 のいずれかが pass でなければ `failed`、それらが pass した上で E2 または schema check が未達なら初めて `partial`、全 pass なら `full` とする。これは契約 §4 の必要条件と整合する。
- `mvp/anvilminimal/src/planner/assurance.rs:1182-1201` の `assurance_for_completion` も、data probe 未実行時の初期値を `static / data_profile_probe_not_run` としている。同 `:1203-1226` の `earned_assurance_for_completion` は data behavior probe の `partial/static/failed` をそのまま優先する。
- `mvp/anvilminimal/src/planner/final_acceptance.rs:786-825` の `ultra_final_acceptance_report_inner` は data behavior probe を実行し、その結果を `earned_assurance_for_completion` へ渡す。

この data 専用経路自体に、今回の `partial` インフレはない。

#### live failure terminal が実際に通った経路

4 run とも phase 1 で失敗し、イベント列に `plan_final_contract` も `ultra_final_acceptance` もない。したがって上記の data behavior probe / earned assurance へ到達していない。

実際の経路は次のとおり。

1. `mvp/anvilminimal/src/tui/slash.rs:488-528` の `emit_tui_command_stop_with_status` が、最新 completion snapshot を取得して terminal projection を作る。
2. 同 `:617-646` の `apply_config_completion_metadata` は `generic` 以外を一律に `assurance_level="full"` へ初期化する。`data` の分岐も、`assurance_for_completion` / data adapter の呼び出しもない。
3. `mvp/anvilminimal/src/eval_events.rs:732-754` の `project_completion` が profile 非依存の `projected_assurance_from_snapshot` を呼ぶ。
4. 同 `:850-884` の `projected_assurance_from_snapshot` は、seed が `full` で `final_acceptance!="full_success"`（今回 `not_checked`）なら `partial / acceptance_not_full_success` へ落とす。この写像は `effective_profile="data"` を確認するだけで data 契約を dispatch しない。
5. `mvp/anvilminimal/src/lib.rs:621-665` の `emit_run_stop` は `tui_command_stop` projection を読み、同 `:730-756` の複製された `apply_config_completion_metadata` と同じ汎用投影を通した後、`apply_tui_command_stop_projection` で `partial` を継承する。

従って live early-failure 経路には、data 用契約写像ではなく、data 導入前から存在する「非 generic を full で seed し、acceptance 未達なら partial へ落とす」汎用 assurance projection が適用されている。

### 判定表

| Run | 報告 level | pipeline 実行 | E1 実施/pass | E3 実施/pass | 契約 §4 上の正しい level | 準拠判定 |
| --- | --- | --- | --- | --- | --- | --- |
| 1 `qwen35/profile` | `partial` | なし。`pipeline/main.py` 自体が未生成 | なし / pass evidence なし | なし / pass evidence なし | `failed`（script 未生成かつ run failure） | **違反（2段階インフレ）** |
| 2 `gemma31/profile` | `partial` | あり。生成ステップの `python pipeline/main.py` は成功し output も生成 | なし / pass evidence なし | なし / pass evidence なし | `static`（契約プローブ未完。`pipeline-run.json` なし） | **違反（インフレ）** |
| 3 `qwen35/none` | `partial` | なし。実行 command は path confinement で process 起動前に拒否 | なし / pass evidence なし | なし / pass evidence なし | `static`（script 生成、契約プローブ未完） | **違反（インフレ）** |
| 4 `gemma31/none` | `partial` | なし。verify は setup authority 判定で実行前に停止 | なし / pass evidence なし | なし / pass evidence なし | `static`（script 生成、契約プローブ未完） | **違反（インフレ）** |

Run 2 の output は存在するが、契約 schema に不適合で、契約による E1/E3 adjudication は実施されていない。「実行した」という事実だけでは `partial` の E1/E3 pass 条件を代替しない。

**結論: 契約違反（インフレ）——修正すべき経路は `tui::slash::apply_config_completion_metadata` → `eval_events::projected_assurance_from_snapshot`（および複製された `lib::apply_config_completion_metadata`）。**

## 調査②: Run 3 `artifact_follow_through_exhausted`

### repair / recovery artifact の収集確認

元 `.anvil/` に存在する対象は repair 1件、recovery YAML 1件である。両方とも収集先に既存で、SHA-256 が一致したため、未コピーの追補はなかった。

| 種別 | 収集先 | SHA-256 | 判定 |
| --- | --- | --- | --- |
| repair | `artifacts/data_agg_qwen27_plan_qwen35_exec_preset_none_001/anvil-repairs/repair-phase-validate-input-data-019f5c0f-c653-7973-a656-c0df14de43ec.md` | `5f5ebf07c578cf0c284c70588ba39e80ce72723abada151f06975d48f4d47cb6` | 元 `.anvil/repairs/` と同一 |
| recovery YAML | `artifacts/data_agg_qwen27_plan_qwen35_exec_preset_none_001/anvil-plans/recovery-ultra-plan-phase-validate-input-data-019f5c0f-c654-7453-87b0-1e7e2929b0a8.yaml` | `163ca6a917691452ec9e7f7cea63c24fe497fd278b8b0d362fe297bb7672a995` | 元 `.anvil/plans/` と同一 |

### stop reason 全文

`artifacts/data_agg_qwen27_plan_qwen35_exec_preset_none_001/events.jsonl:87` の `stop_reason`:

```text
phase validate-input-data failed: artifact_follow_through_exhausted: missing expected paths: output/results.json, output/report.md; artifact_stagnation_feedback_count: 2; incomplete; Recovery artifact check: prompt_parse_ok=true, yaml_parse_ok=true, command_targets_valid=true
Paths:
- repair prompt saved: .anvil/repairs/repair-phase-validate-input-data-019f5c0f-c653-7973-a656-c0df14de43ec.md
- Recovery UltraPlan YAML saved: .anvil/plans/recovery-ultra-plan-phase-validate-input-data-019f5c0f-c654-7453-87b0-1e7e2929b0a8.yaml
Commands:
- suggested command: /ultra-plan-run --profile data "$(cat .anvil/repairs/repair-phase-validate-input-data-019f5c0f-c653-7973-a656-c0df14de43ec.md)"
- suggested YAML command: /run-ultra-plan .anvil/plans/recovery-ultra-plan-phase-validate-input-data-019f5c0f-c654-7453-87b0-1e7e2929b0a8.yaml
```

### 停止直前の原文イベント

`events.jsonl:63`。`pipeline/main.py` は存在する状態になったが、次の target `output/results.json` に進まず同じ source を再編集したため、成果物 follow-through の1回目が発火した。

```json
{"attempt":1,"attempt_limit":3,"event":"artifact_stagnation_feedback","last_model_action":"edit_without_required_artifact_progress","missing_paths":["output/results.json","output/report.md"],"non_edit_streak":5,"schema_version":"1","target_attempt":1,"target_path":"output/results.json"}
```

`events.jsonl:67`。生成済み pipeline の実行を試みたが、workspace 絶対パスを含む `cd` が path confinement で拒否された。`python3 pipeline/main.py` という workspace-relative command への再試行はない。

```json
{"command":"cd /Users/<user>/share/work/localwork/commandagent_mvp/01/test0713_b2-_001/test0713_data_001/data_agg_qwen27_plan_qwen35_exec_preset_none_001 && python3 pipeline/main.py","event":"bash_path_confinement_rejected","nearest_relative":"Users","path":"/Users/","root":"/Users/maenokota/share/work/localwork/commandagent_mvp/01/test0713_b2-_001/test0713_data_001/data_agg_qwen27_plan_qwen35_exec_preset_none_001","schema_version":"1"}
```

`events.jsonl:77`。拒否後も `pipeline/main.py` と input の Read を反復し、同じ target に対する2回目が発火した。

```json
{"attempt":2,"attempt_limit":3,"event":"artifact_stagnation_feedback","last_model_action":"non_edit_tool","missing_paths":["output/results.json","output/report.md"],"non_edit_streak":4,"schema_version":"1","target_attempt":2,"target_path":"output/results.json"}
```

`events.jsonl:81`。最後の provider turn も `pipeline/main.py` の Read だけで、read-only intervention に戻った。

```json
{"event":"read_only_stagnation_feedback","objective":"Execute exactly one StepPlan step. Overall goal: data/sales.csv を読み込み、月次×地域の売上集計と全体合計を計算し、無効な行は理由別に除外して件数を明記した上で、要約レポートを作成してください。 Current step id: implement-pipeline Current step kind: implement Current step instruction: Create pipeline/main.py to read data/sales.csv, validate rows (checking missing fields, invalid dates, non-numeric sales), categorize invalid rows by reason, calculate monthly and regional sales aggregations, and write output/results.json and output/report.md. Ensure determinist","pending_capability_evidence":[],"phase_scope":"validate-input-data","read_only_streak":3,"schema_version":"1","selected_targets":[],"selection_reason":"","session_scope":"plan-run-step","stage":"intervention","step_kind":"implement","target_path":""}
```

`events.jsonl:82`。非 scaffold の不足 path が残ったため、read-only/no-progress より `artifact_follow_through_exhausted` が terminal reason として優先された。

```json
{"artifact_stagnation_feedback_count":2,"event":"loop_stop","last_blocking_reason":"model_stagnation:read_only_loop","last_provider_error":null,"missing_capabilities":[],"missing_evidence":[],"missing_obligations":[],"missing_paths":["output/results.json","output/report.md"],"non_scaffold_missing_paths":["output/results.json","output/report.md"],"read_only_streak":3,"reason":"artifact_follow_through_exhausted","schema_version":"1","verify_attempts":0}
```

`events.jsonl:84` の phase terminal:

```json
{"event":"ultra_phase_failed","final_phase":false,"ok":false,"phase_id":"validate-input-data","phase_index":1,"reason":"artifact_follow_through_exhausted: missing expected paths: output/results.json, output/report.md; artifact_stagnation_feedback_count: 2","schema_version":"1","stage":"execute","step_count":null,"total_phases":3}
```

### 枯渇内容の要約

1. Step の初期 required paths は `pipeline/main.py`, `output/results.json`, `output/report.md` の3件だった（`events.jsonl:30`）。最初は `pipeline/main.py` が target となり、stagnation feedback 1回の後、モデルは full-file Write を2回行った（7497 bytes 相当と 8563 bytes 相当）。この最初の source target の attempt は、後に target が `output/results.json` へ移った時点でリセットされており、terminal の `artifact_stagnation_feedback_count=2` には含まれない。
2. `pipeline/main.py` 生成後、follow-through target は `output/results.json` になった。これに対する bounded feedback は上記 `attempt=1` と `attempt=2` の **2回**。`output/report.md` も同じ pipeline 実行で生成される設計だったため、両方が一貫して missing のまま残った。
3. モデルは pipeline 実行を1回だけ試みたが、絶対 `cd` を含むため tool validation で拒否された。process は起動せず、`verify_attempts=0`。その後、相対 command への修正、output の直接 Write、または source の再修正は行わず Read を反復した。
4. 生成済み `pipeline/main.py` は、起動されれば `output/results.json` と `output/report.md` を書く実装コードだった。しかし contract 固定 schema の `reconciliation` / `values` ではなく `summary` 等を用い、負数を `negative_amount` として除外しないため、内容も契約上は不十分だった。
5. 最後に生成していた内容の種別は **部分コード**。最後の artifact-producing response は `pipeline/main.py` の full-file code であり、計画文でも空応答でもない。ただし枯渇直前の最終 model action 自体は新規 content を伴わない `Read("pipeline/main.py")` だった。

### nextjs 時代の既知クラスとの照合

症状は nextjs 時代の `write_required` / `model_stagnation:no_progress_recorded` と同属で、実装 gap が残るのに Read・非 target action を反復し、上限で正直に fail する点は同じである。違いは、Run 3 では `read_only_stagnation_feedback` が `stage="intervention"` のままで `selected_targets=[]`、`write_required`、`read_only_tool_rejected`、`no_progress_feedback` のいずれも発火していないことにある。ここでは明示された非 scaffold required paths が残っていたため、`mvp/anvilminimal/src/minimal_loop/loop_run.rs:2635-2694` の終端優先順位により pressure state の `model_stagnation:read_only_loop` より具体的な `artifact_follow_through_exhausted` が選ばれ、missing output paths と per-target feedback count を報告した。`write_required` 系は具体的な repair source を選んで Read を強制拒否する経路、`no_progress` 系は required path が既に満たされていても決定的完了が進まない経路であり、Run 3 は「source は書けたが、実行して派生成果物へ到達しない」という artifact follow-through 固有の形である。
