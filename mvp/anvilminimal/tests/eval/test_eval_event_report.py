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


if __name__ == "__main__":
    unittest.main()
