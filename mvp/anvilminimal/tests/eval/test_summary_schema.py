import sys
import tempfile
import unittest
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(ROOT / "scripts"))

from eval_lib.report import compare_summaries
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


if __name__ == "__main__":
    unittest.main()

