import sys
import tempfile
import unittest
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(ROOT / "scripts"))

from eval_lib.plan_capability_contract import score_plan_capability_contract
from eval_lib.plan_verify_coverage import score_plan_verify_coverage


SCENARIO = {
    "profile": "nextjs",
    "prompt": "Create a Next.js Space Invaders game that can run on port 3011.",
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


PLAN = """
goal: Build a game.
steps:
  - id: game
    kind: implement
    instruction: Create a Canvas game loop with keyboard controls, player ship, enemies, bullets, collision, score, and lives.
    expected_paths: [src/app/page.tsx]
    verify:
      - {verify}
"""


class PlanVerifyCoverageTest(unittest.TestCase):
    def test_build_only_game_verify_has_low_coverage(self):
        with tempfile.TemporaryDirectory() as td:
            plan = Path(td) / "plan.yaml"
            plan.write_text(PLAN.format(verify="npm run build"), encoding="utf-8")
            result = score_plan_verify_coverage(scenario=SCENARIO, mode="step-plan", plan_paths=[plan])
        self.assertLess(result["plan_verify_coverage_score"], 40, result)
        self.assertEqual(result["plan_verify_gap_kind"], "build_only_verify_for_behavior_contract")

    def test_step_plan_does_not_read_unmaterialized_verify_artifact(self):
        with tempfile.TemporaryDirectory() as td:
            plan = Path(td) / "plan.yaml"
            plan.write_text(PLAN.format(verify="node smoke-check.js"), encoding="utf-8")
            result = score_plan_verify_coverage(
                scenario=SCENARIO,
                mode="step-plan",
                plan_paths=[plan],
                workdir=Path(td) / "workdir",
            )
        self.assertEqual(result["executed_verify_coverage_score"], "", result)
        self.assertLess(result["plan_verify_declared_coverage_score"], 70, result)

    def test_plan_run_reads_safe_smoke_artifact_and_raises_coverage(self):
        with tempfile.TemporaryDirectory() as td:
            root = Path(td)
            plan = root / "plan.yaml"
            workdir = root / "workdir"
            workdir.mkdir()
            plan.write_text(PLAN.format(verify="node smoke-check.js"), encoding="utf-8")
            (workdir / "smoke-check.js").write_text(
                """
const source = 'canvas requestAnimationFrame keydown player enemy bullet collision score lives';
for (const token of ['canvas','requestAnimationFrame','keydown','player','enemy','bullet','collision','score','lives']) {
  if (!source.includes(token)) throw new Error(token);
}
""",
                encoding="utf-8",
            )
            result = score_plan_verify_coverage(
                scenario=SCENARIO,
                mode="plan-run",
                plan_paths=[plan],
                workdir=workdir,
            )
        self.assertGreaterEqual(result["executed_verify_coverage_score"], 80, result)
        self.assertEqual(result["plan_verify_gap_kind"], "")

    def test_verify_artifact_path_is_confined_to_workdir(self):
        with tempfile.TemporaryDirectory() as td:
            root = Path(td)
            plan = root / "plan.yaml"
            workdir = root / "workdir"
            workdir.mkdir()
            plan.write_text(PLAN.format(verify="node ../outside-smoke-check.js"), encoding="utf-8")
            result = score_plan_verify_coverage(
                scenario=SCENARIO,
                mode="plan-run",
                plan_paths=[plan],
                workdir=workdir,
            )
        artifact_reads = result["plan_verify_details"]["artifact_reads"]
        self.assertEqual(artifact_reads[0]["status"], "outside_workspace", result)

    def test_plan_capability_result_can_be_reused(self):
        with tempfile.TemporaryDirectory() as td:
            plan = Path(td) / "plan.yaml"
            plan.write_text(PLAN.format(verify="npm run build"), encoding="utf-8")
            capability = score_plan_capability_contract(scenario=SCENARIO, plan_paths=[plan])
            result = score_plan_verify_coverage(
                scenario=SCENARIO,
                mode="step-plan",
                plan_paths=[plan],
                plan_capability_result=capability,
            )
        self.assertEqual(
            result["plan_unverified_capability_count"],
            capability["plan_required_capability_count"],
            result,
        )


if __name__ == "__main__":
    unittest.main()
