import tempfile
import unittest
from pathlib import Path
import sys

ROOT = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(ROOT / "scripts"))

from eval_lib.report import generate_report
from eval_lib.run_summary import SUMMARY_HEADER, write_summary


class EvalEventReportTest(unittest.TestCase):
    def test_report_summarizes_planner_raw_output_shapes_from_extras(self):
        with tempfile.TemporaryDirectory() as td:
            run_root = Path(td)
            row = {key: "" for key in SUMMARY_HEADER}
            row.update(
                {
                    "run_id": "shape",
                    "suite": "s",
                    "scenario": "scenario",
                    "size": "small",
                    "category": "planner",
                    "mode": "step-plan",
                    "planner_provider": "gemini",
                    "planner_model": "gemini-3.5-flash",
                    "success": "false",
                    "rc": "1",
                    "extras_json": {
                        "failure_kind": "planner_schema_error",
                        "planner_raw_output_shapes": [
                            {
                                "planner_provider": "gemini",
                                "planner_model": "gemini-3.5-flash",
                                "json_extract_status": "missing",
                                "has_json_object": False,
                                "contains_goal_key": False,
                                "contains_steps_key": True,
                            }
                        ],
                    },
                }
            )
            write_summary(run_root / "summary.eval.tsv", [row])
            report = generate_report(run_root)
        self.assertIn("## Planner Raw Output Shapes", report)
        self.assertIn("| gemini | gemini-3.5-flash | missing | False | False | True | 1 |", report)

    def test_report_ignores_unknown_event_like_extras(self):
        with tempfile.TemporaryDirectory() as td:
            run_root = Path(td)
            row = {key: "" for key in SUMMARY_HEADER}
            row.update(
                {
                    "run_id": "unknown",
                    "suite": "s",
                    "scenario": "scenario",
                    "size": "small",
                    "category": "planner",
                    "mode": "step-plan",
                    "success": "true",
                    "rc": "0",
                    "extras_json": {"unknown_event_payload": [{"event": "new_event"}]},
                }
            )
            write_summary(run_root / "summary.eval.tsv", [row])
            report = generate_report(run_root)
        self.assertIn("# anvilminimal Eval Report", report)
        self.assertIn("## Planner Raw Output Shapes", report)

    def test_report_summarizes_target_metric_reasons_and_failure_layers(self):
        with tempfile.TemporaryDirectory() as td:
            run_root = Path(td)
            row = {key: "" for key in SUMMARY_HEADER}
            row.update(
                {
                    "run_id": "target",
                    "suite": "s",
                    "scenario": "scenario",
                    "size": "medium",
                    "category": "runtime",
                    "mode": "plan-run",
                    "success": "false",
                    "rc": "1",
                    "failure_layer": "bridge",
                    "capability_failure_included": "true",
                    "postcheck_stability_score": "25",
                    "postcheck_stability_reason": "build_or_test_command_failed;compile_or_type_failure",
                    "execution_contract_adherence_raw_score": "88",
                    "execution_contract_adherence_score": "55",
                    "execution_contract_cap_reason": "postcheck_stability_below_60",
                    "runtime_friction_reason": "verify_repair_stagnation",
                    "finalization_reason": "deferred_verify_requirement_pending",
                    "extras_json": {"failure_kind": "deferred_verify_requirement_pending"},
                }
            )
            write_summary(run_root / "summary.eval.tsv", [row])
            report = generate_report(run_root)
        self.assertIn("## Target Runtime Metrics", report)
        self.assertIn("## Target Metric Reasons", report)
        self.assertIn("build_or_test_command_failed", report)
        self.assertIn("postcheck_stability_below_60", report)
        self.assertIn("## Failure Layers", report)
        self.assertIn("| bridge | 1 | 1 | 0 |", report)

    def test_report_summarizes_plan_run_readiness(self):
        with tempfile.TemporaryDirectory() as td:
            run_root = Path(td)
            step = {key: "" for key in SUMMARY_HEADER}
            step.update(
                {
                    "run_id": "ready-step",
                    "suite": "s",
                    "scenario": "scenario",
                    "size": "medium",
                    "category": "planner",
                    "mode": "step-plan",
                    "main_provider": "openai",
                    "main_model": "gpt-5.4-mini",
                    "planner_provider": "openai",
                    "planner_model": "gpt-5.4-mini",
                    "local_llm_used": "false",
                    "success": "true",
                    "rc": "0",
                    "plan_run_readiness_score": "82",
                    "verify_policy_readiness_score": "100",
                    "contract_handoff_score": "85",
                    "declared_contract_completeness_score": "85",
                    "postcheck_contract_alignment_score": "80",
                    "dependency_ordering_score": "90",
                    "finalization_readiness_score": "75",
                    "readiness_cap_reason": "",
                }
            )
            run = dict(step)
            run.update(
                {
                    "run_id": "ready-run",
                    "mode": "plan-run",
                    "success": "false",
                    "rc": "1",
                    "plan_run_readiness_score": "82",
                    "missed_predictive_signal_reason": "postcheck_contract_not_reflected_in_readiness",
                    "extras_json": {"failure_kind": "postcheck_failure"},
                }
            )
            write_summary(run_root / "summary.eval.tsv", [step, run])
            report = generate_report(run_root)
        self.assertIn("## Plan Run Readiness", report)
        self.assertIn("postcheck_contract_not_reflected_in_readiness", report)
        self.assertIn("readiness", report)

    def test_report_summarizes_acceptance_false_positive(self):
        with tempfile.TemporaryDirectory() as td:
            run_root = Path(td)
            row = {key: "" for key in SUMMARY_HEADER}
            row.update(
                {
                    "run_id": "acceptance",
                    "suite": "s",
                    "scenario": "nextjs-game",
                    "size": "large",
                    "category": "new-code",
                    "mode": "ultra-plan-run",
                    "success": "true",
                    "legacy_success": "true",
                    "acceptance_success": "false",
                    "acceptance_false_positive": "true",
                    "acceptance_failure_kind": "static_title_only",
                    "oracle_gap_kind": "postcheck_too_weak_for_semantic_contract",
                    "source_semantic_score": "35",
                    "rc": "0",
                }
            )
            write_summary(run_root / "summary.eval.tsv", [row])
            report = generate_report(run_root)
        self.assertIn("## Acceptance Outcomes", report)
        self.assertIn("static_title_only", report)
        self.assertIn("postcheck_too_weak_for_semantic_contract", report)


if __name__ == "__main__":
    unittest.main()
