# Parity Gate Fixture Plan

作成日: 2026-06-29

## 1. 方針

fixture は eval scenario の文言に過適応させない。各 fixture は lifecycle stage の一般契約を検査する。

実装候補:

- Rust unit test: controller / verifier / repair target / completion policy
- Python pytest: eval summary / scoring / failure taxonomy / report schema
- Golden fixture: plan yaml、events jsonl、acceptance artifact tree

## 2. Required Fixture Coverage

| gate_id | positive fixture | negative fixture | planned test |
| --- | --- | --- | --- |
| G-S01 | app/game prompt から artifact/capability obligation を抽出できる | app要求にdocs-only outputを返すとacceptance failure | `test_request_understanding_contract_fixture.py` |
| G-S02 | setup/scaffold/implementation/verify/acceptance role が分離される | setup-only / scaffold-only が completion にならない | `test_task_contract_lite_roles.py` |
| G-S03 | valid StepPlan/UltraPlan が schema/lint を通る | missing goal / bad verify shell control syntax が具体 failure kind になる | `test_plan_generation_schema_lint_gate.py` |
| G-S04 | profile rules を含む UltraPlan が phase 化される | deterministic fallback thin plan を success plan と扱わない | `test_ultra_plan_generation_gate.py` |
| G-S05 | phase2 prompt に phase1 outcome / repair target が含まれる | phase failure context が次 phase prompt から消える | `test_ultra_phase_context_fixture.rs` |
| G-S06 | step prompt に overall goal / expected paths / verify / expected_result が含まれる | expected_result 欠落 prompt は prompt-contract failure | `test_step_prompt_contract_fixture.rs` |
| G-S07 | recoverable rough tool args は正規化される | unsafe path / shell control syntax / invalid tool name は拒否 | `test_tool_policy_compatibility_fixture.rs` |
| G-S08 | deterministic verify が dependency boundary を尊重する | shell control syntax verify が blank failure kind にならない | `test_verify_command_policy_failure_kind.py` |
| G-S09 | dependency missing -> setup allowed -> build rerun が event 化される | manifest なし build/test verify が要求されると policy failure | `test_dependency_lifecycle_fixture.rs` |
| G-S10 | missing entrypoint が repair target になり follow-through される | no-change repair は classified + handoff required | `test_repair_target_followthrough_fixture.rs` |
| G-S11 | scaffold 後に implementation continuation target が出る | scaffold-only app が success になると failure | `test_scaffold_only_not_success_fixture.rs` |
| G-S12 | build + route + capability evidence で acceptance pass | title-only/build-only/static shell は acceptance failure | `test_uat_acceptance_contract.py` |
| G-S13 | repair exhaustion で repair prompt と suggested command が保存される | exhausted repair without handoff は failure | `test_recovery_handoff_fixture.rs` |
| G-S14 | rc!=0/process_failure は concrete failure kind を持つ | failure kind 空欄 row は gate failure | `test_failure_kind_coverage_gate.py` |
| G-S15 | OpenAI/Gemini/Ollama probe が shape summary を記録する | API key なし環境で通常 test が failure にならない | `test_provider_probe_metadata_gate.py` |
| G-S16 | TUI run が `.anvil/runs/<run-id>/events.jsonl` を残す | silent exit without run events は gate failure | `test_tui_observability_contract.py` |

## 3. Minimal Test Set To Add

| Test | Scope | Network |
| --- | --- | --- |
| `test_runtime_semantics_gate_matrix.py` | matrix rows/columns/status completeness | no |
| `test_parity_gate_fixture_coverage.py` | each gate has positive + negative fixture | no |
| `test_failure_kind_coverage_gate.py` | failure summary blank kind rejection | no |
| `test_source_mvp_trace_manifest.py` | trace manifest required fields/redaction | no |
| `test_uat_acceptance_contract.py` | build-only/title-only/capability evidence | no |
| `test_parity_gate_report_schema.py` | report json schema and gate partition | no |
| `test_parity_gate_comparison_threshold.py` | warn/fail threshold calculation | no |
| `test_trace_redaction_contract.py` | secrets are not persisted in trace manifest | no |
| `test_provider_probe_metadata_gate.py` | provider probe metadata exists when opted in | optional |
| `test_tui_observability_contract.py` | manual/TUI run event contract via fake run | no |

## 4. Fixture Naming Rules

- eval scenario id を fixture 名に入れない。
- `space_invaders` のような個別 prompt 名を contract fixture に使わない。
- capability fixture は `interactive_game`, `web_app`, `cli_tool`, `library`, `docs`, `data_transform` のような domain level にする。
- negative fixture は必ず「なぜ失敗すべきか」を failure kind と紐づける。

## 5. Acceptance For Phase 4

- G-S01〜G-S16 のすべてに positive / negative fixture 案がある。
- `scaffold-only`, `setup-only`, `docs-only app`, `title-only app`, `blank failure kind` の negative fixture が明示されている。
- fixture は local gate で実行可能な設計になっている。
