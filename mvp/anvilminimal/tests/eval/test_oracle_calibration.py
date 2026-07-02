import sys
import unittest
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(ROOT / "scripts"))

from eval_lib.oracle_calibration import evaluate_calibration_root, summarize_calibration


class OracleCalibrationTest(unittest.TestCase):
    def test_acceptance_oracle_fixture_replay_has_positive_and_negative_cases(self):
        cases = evaluate_calibration_root(ROOT / "eval/fixtures/acceptance_oracle")
        summary = summarize_calibration(cases)
        self.assertGreaterEqual(summary["positive"], 1, summary)
        self.assertGreaterEqual(summary["negative"], 1, summary)
        self.assertGreaterEqual(summary["out_of_scope"], 1, summary)

    def test_static_title_is_negative_and_good_game_is_positive(self):
        cases = {
            case["fixture"]: case
            for case in evaluate_calibration_root(ROOT / "eval/fixtures/acceptance_oracle")
        }
        self.assertFalse(cases["nextjs_static_title_only"]["gate_success"], cases["nextjs_static_title_only"])
        self.assertEqual(
            cases["nextjs_static_title_only"]["source_semantic_failure_kind"],
            "static_title_only",
        )
        self.assertTrue(cases["nextjs_good_minimal_game"]["gate_success"], cases["nextjs_good_minimal_game"])


if __name__ == "__main__":
    unittest.main()
