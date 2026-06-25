import sys
import tempfile
import unittest
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(ROOT / "scripts"))

from eval_lib.report import compare_summaries, generate_report
from eval_lib.run_summary import SUMMARY_HEADER, read_summary, write_summary


class SummarySchemaTest(unittest.TestCase):
    def test_fixture_header(self):
        rows = read_summary(ROOT / "eval/fixtures/summaries/baseline.summary.eval.tsv")
        self.assertEqual(set(rows[0].keys()), set(SUMMARY_HEADER))

    def test_write_read_round_trip(self):
        with tempfile.TemporaryDirectory() as td:
            path = Path(td) / "summary.eval.tsv"
            write_summary(path, [{"run_id": "x", "suite": "s"}])
            self.assertEqual(read_summary(path)[0]["run_id"], "x")

    def test_compare_generates_markdown(self):
        text = compare_summaries(
            ROOT / "eval/fixtures/summaries/baseline.summary.eval.tsv",
            ROOT / "eval/fixtures/summaries/experiment.summary.eval.tsv",
        )
        self.assertIn("# Eval Compare", text)
        self.assertIn("success_rate", text)

    def test_report_uses_failure_kind_from_extras(self):
        with tempfile.TemporaryDirectory() as td:
            run_root = Path(td)
            row = {key: "" for key in SUMMARY_HEADER}
            row.update(
                {
                    "run_id": "x",
                    "suite": "s",
                    "scenario": "scenario",
                    "size": "small",
                    "category": "provider-smoke",
                    "mode": "minimal-loop",
                    "main_provider": "openai",
                    "success": "false",
                    "rc": "1",
                    "stop_reason": "verify_repair_exhausted",
                    "last_blocking_reason": "command failed",
                    "extras_json": {"failure_kind": "tool_validation_error"},
                }
            )
            write_summary(run_root / "summary.eval.tsv", [row])
            report = generate_report(run_root)
            self.assertIn("blocking: all required runs failed", report)
            self.assertIn("| tool_validation_error | 1 |", report)
            self.assertIn("verify_repair_exhausted", report)
            self.assertIn("command failed", report)


if __name__ == "__main__":
    unittest.main()
