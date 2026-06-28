import sys
import tempfile
import unittest
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(ROOT / "scripts"))

from eval_lib.plan_capability_contract import score_plan_capability_contract


GAME_SCENARIO = {
    "profile": "nextjs",
    "prompt": "3011ポートで起動できる最高に面白いスペースインベーダーゲームをNext.jsで作ってください。",
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
    },
}


class PlanCapabilityContractTest(unittest.TestCase):
    def test_prompt_rich_plan_skeleton_scores_low(self):
        with tempfile.TemporaryDirectory() as td:
            plan = Path(td) / "plan.yaml"
            plan.write_text(
                """
goal: Create a Next.js app.
steps:
  - id: scaffold
    kind: implement
    instruction: Create package.json and src/app/page.tsx.
    expected_paths: [package.json, src/app/page.tsx]
    verify: [npm run build]
""",
                encoding="utf-8",
            )
            result = score_plan_capability_contract(scenario=GAME_SCENARIO, plan_paths=[plan])
        self.assertLess(result["prompt_plan_capability_coverage_score"], 50, result)
        self.assertEqual(result["prompt_plan_gap_kind"], "plan_too_generic_for_prompt")

    def test_game_plan_with_expected_paths_and_verify_scores_high(self):
        with tempfile.TemporaryDirectory() as td:
            plan = Path(td) / "plan.yaml"
            plan.write_text(
                """
goal: Create a playable Space Invaders game.
steps:
  - id: game
    kind: implement
    instruction: Implement Canvas game loop, keyboard player control, invaders, bullets, collision, score, lives, and restart flow in src/app/page.tsx.
    expected_paths: [src/app/page.tsx]
    verify:
      - node smoke-check.js
  - id: smoke
    kind: verify
    instruction: Create smoke-check.js assertions for canvas, keydown, enemies, bullets, collision, score, and lives.
    expected_paths: [smoke-check.js]
    verify:
      - node smoke-check.js
""",
                encoding="utf-8",
            )
            result = score_plan_capability_contract(scenario=GAME_SCENARIO, plan_paths=[plan])
        self.assertGreaterEqual(result["prompt_plan_capability_coverage_score"], 80, result)
        self.assertGreaterEqual(result["plan_capability_contract_score"], 75, result)

    def test_cli_entry_point_does_not_match_points_progression(self):
        scenario = {"prompt": "Create a CLI with one entry point and cargo test verification."}
        with tempfile.TemporaryDirectory() as td:
            plan = Path(td) / "plan.yaml"
            plan.write_text(
                """
goal: Create a CLI.
steps:
  - id: cli
    kind: implement
    instruction: Create src/main.rs with a CLI main entry point.
    expected_paths: [src/main.rs]
    verify: [cargo test]
""",
                encoding="utf-8",
            )
            result = score_plan_capability_contract(scenario=scenario, plan_paths=[plan])
        self.assertIn("cli_entrypoint", result["plan_required_capabilities"], result)
        self.assertNotIn("score_or_progression", result["plan_required_capabilities"], result)


if __name__ == "__main__":
    unittest.main()
