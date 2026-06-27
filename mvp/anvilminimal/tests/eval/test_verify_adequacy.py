import sys
import json
import tempfile
import unittest
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(ROOT / "scripts"))

from eval_lib.plan_scoring import score_plan_file
from eval_lib.simple_yaml import load_yaml
from eval_lib.verify_adequacy import score_verify_adequacy


SCENARIO = {
    "size": "large",
    "profile": "nextjs",
    "prompt": "Create a Next.js Space Invaders game that can run on port 3011.",
    "expected_artifacts": ["package.json", "src/app/page.tsx"],
    "functional_contract": {
        "category": "interactive-game",
        "required_capabilities": [
            "stateful_interaction",
            "start_or_restart_flow",
            "player_control",
            "adversary_or_challenge",
            "progression_or_score",
            "failure_or_collision_rule",
        ],
        "forbidden_minimal_outputs": ["static_title_only"],
    },
    "plan_constraints": {"min_steps": 2, "max_steps": 6, "required_verify_keywords": ["npm run build"]},
}


class VerifyAdequacyTest(unittest.TestCase):
    def test_build_and_file_existence_are_not_semantic_acceptance(self):
        score = score_verify_adequacy(
            [
                {"verify": ["test -f src/app/page.tsx"]},
                {"verify": ["npm run build"]},
            ],
            SCENARIO,
        )
        self.assertLess(score["verify_adequacy_score"], 60, score)
        self.assertGreaterEqual(score["contentless_verify_penalty"], 45)
        self.assertLess(score["semantic_verify_coverage_score"], 50)

    def test_behavior_oracle_and_semantic_terms_score_higher(self):
        score = score_verify_adequacy(
            [
                {
                    "verify": [
                        "npm test -- --runInBand",
                        "playwright test tests/game.spec.ts",
                    ]
                },
                {
                    "verify": [
                        "grep -q 'player enemy score collision keyboard start' src/app/page.tsx"
                    ]
                },
            ],
            SCENARIO,
        )
        self.assertGreaterEqual(score["verify_adequacy_score"], 80, score)

    def test_plan_scoring_exposes_verify_adequacy_fields(self):
        text = """goal: nextjs game
steps:
  - id: setup
    kind: setup
    instruction: Create package.json.
    expected_paths:
      - package.json
    verify:
      - test -f package.json
  - id: implement
    kind: implement
    instruction: Create src/app/page.tsx.
    expected_paths:
      - src/app/page.tsx
    verify:
      - npm run build
"""
        with tempfile.TemporaryDirectory() as td:
            path = Path(td) / "plan.yaml"
            path.write_text(text, encoding="utf-8")
            score = score_plan_file(path, SCENARIO)
        self.assertIn("verify_adequacy_score", score)
        self.assertLess(score["verify_adequacy_score"], 60, score)

    def test_contentless_verify_fixture_scores_low(self):
        directory = ROOT / "eval/fixtures/acceptance_oracle/contentless_verify_plan"
        scenario = load_yaml(directory / "scenario.yaml")
        expected = json.loads((directory / "expected.json").read_text(encoding="utf-8"))
        score = score_plan_file(directory / "plan.yaml", scenario)
        self.assertLessEqual(score["verify_adequacy_score"], expected["verify_adequacy_max"], score)
        self.assertGreaterEqual(
            score["contentless_verify_penalty"],
            expected["contentless_verify_penalty_min"],
            score,
        )


if __name__ == "__main__":
    unittest.main()
