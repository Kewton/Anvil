import sys
import unittest
from pathlib import Path
import tempfile

ROOT = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(ROOT / "scripts"))

from eval_lib.plan_scoring import score_plan_file
from eval_lib.suites import load_suite


class PlanScoringTest(unittest.TestCase):
    def setUp(self):
        suite = load_suite(ROOT / "eval/suites/mvp-smoke.yaml")
        self.scenario = next(s for s in suite["scenarios"] if s["id"] == "nextjs-space-invaders-large")

    def test_good_plan_scores_higher_than_bad(self):
        good = score_plan_file(ROOT / "eval/fixtures/plans/good-step-plan.yaml", self.scenario)
        bad = score_plan_file(ROOT / "eval/fixtures/plans/bad-overlong-step-plan.yaml", self.scenario)
        self.assertGreater(good["score"], bad["score"])

    def test_path_escape_is_penalized(self):
        score = score_plan_file(ROOT / "eval/fixtures/plans/bad-path-escape-step-plan.yaml", self.scenario)
        penalties = score["details"]["penalties"]
        self.assertTrue(any(p["kind"] == "path_escape" for p in penalties))
        self.assertLess(score["score"], 60)

    def test_ultra_plan_scores(self):
        score = score_plan_file(ROOT / "eval/fixtures/plans/good-ultra-plan.yaml", self.scenario)
        self.assertEqual(score["kind"], "ultra")
        self.assertGreaterEqual(score["score"], 60)

    def test_plan_quality_penalizes_lint_categories(self):
        text = """goal: bad
steps:
  - id: s1
    kind: implement
    instruction: Create app
    expected_paths:
      - src/app/page.tsx
    verify:
      - npm test && npm run build
  - id: s2
    kind: implement
    instruction: Create duplicate
    expected_paths:
      - src/app/page.tsx
"""
        with tempfile.TemporaryDirectory() as td:
            path = Path(td) / "plan.yaml"
            path.write_text(text, encoding="utf-8")
            score = score_plan_file(path, self.scenario)
        penalties = {penalty["kind"] for penalty in score["details"]["penalties"]}
        self.assertIn("verify_command_policy_error", penalties)
        self.assertIn("duplicate_expected_path_ownership", penalties)


if __name__ == "__main__":
    unittest.main()
