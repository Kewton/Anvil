import json
import sys
import unittest
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(ROOT / "scripts"))

from eval_lib.plan_scoring import score_plan_file
from eval_lib.simple_yaml import load_yaml
from eval_lib.suites import load_suite


def normalize_generated_plan(data, original_goal):
    kind_aliases = {"create": "implement", "edit": "implement", "work": "implement", "repair": "implement"}
    return {
        "goal": original_goal,
        "steps": [
            {
                "id": step["id"],
                "kind": kind_aliases.get(str(step.get("kind", "implement")).lower(), step.get("kind", "implement")),
                "expected_result": step.get("expected_result", "pass") or "pass",
                "instruction": step["instruction"],
                "expected_paths": step.get("expected_paths", []) or [],
                "verify": step.get("verify", []) or [],
            }
            for step in data["steps"]
        ],
    }


def normalize_loaded_yaml_plan(data):
    normalized = {"goal": data["goal"], "steps": []}
    for step in data["steps"]:
        copied = dict(step)
        copied["expected_paths"] = copied.get("expected_paths") or []
        copied["verify"] = copied.get("verify") or []
        normalized["steps"].append(copied)
    return normalized


class StepPlanSourceParityFixturesTest(unittest.TestCase):
    def test_source_json_normalizes_to_expected_yaml_shape(self):
        source = json.loads((ROOT / "eval/fixtures/plans/source-step-plan.json").read_text(encoding="utf-8"))
        expected = normalize_loaded_yaml_plan(load_yaml(ROOT / "eval/fixtures/plans/source-step-plan.expected.yaml"))
        self.assertEqual(normalize_generated_plan(source, source["goal"]), expected)
        self.assertTrue(any(step["kind"] == "implement" for step in expected["steps"]))
        self.assertTrue(any(step["kind"] == "verify" for step in expected["steps"]))

    def test_existing_mvp_yaml_fixture_remains_readable_and_scorable(self):
        plan = load_yaml(ROOT / "eval/fixtures/plans/existing-mvp-step-plan.yaml")
        self.assertEqual(plan["goal"], "Create a small markdown heading linter.")
        self.assertEqual(len(plan["steps"]), 4)
        self.assertEqual(plan["steps"][1]["kind"], "implement")

    def test_source_expected_yaml_scores_as_non_degenerate_plan(self):
        suite = load_suite(ROOT / "eval/suites/mvp-smoke.yaml")
        scenario = next(s for s in suite["scenarios"] if s["id"] == "nextjs-space-invaders-large")
        score = score_plan_file(ROOT / "eval/fixtures/plans/source-step-plan.expected.yaml", scenario)
        self.assertGreaterEqual(score["score"], 70, score)

    def test_invalid_fixtures_document_rejection_shapes(self):
        missing_goal_yaml = load_yaml(ROOT / "eval/fixtures/plans/invalid-step-plan-missing-goal.yaml")
        self.assertNotIn("goal", missing_goal_yaml)

        unsafe_yaml = load_yaml(ROOT / "eval/fixtures/plans/invalid-step-plan-unsafe-path.yaml")
        paths = unsafe_yaml["steps"][0]["expected_paths"]
        self.assertIn("../secret.txt", paths)

        missing_goal_json = json.loads((ROOT / "eval/fixtures/plans/invalid-step-plan-missing-goal.json").read_text())
        self.assertNotIn("goal", missing_goal_json)

        empty_steps_json = json.loads((ROOT / "eval/fixtures/plans/invalid-step-plan-empty-steps.json").read_text())
        self.assertEqual(empty_steps_json["steps"], [])

    def test_step_plan_fixtures_do_not_contain_secrets_or_home_paths(self):
        fixture_dir = ROOT / "eval/fixtures/plans"
        text = "\n".join(path.read_text(encoding="utf-8") for path in fixture_dir.glob("*step-plan*"))
        self.assertNotIn("OPENAI_API_KEY", text)
        self.assertNotIn("GEMINI_API_KEY", text)
        self.assertNotIn("sk-", text)
        self.assertNotIn("AIza", text)
        self.assertNotIn("/Users/", text)


if __name__ == "__main__":
    unittest.main()
