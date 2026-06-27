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


if __name__ == "__main__":
    unittest.main()
