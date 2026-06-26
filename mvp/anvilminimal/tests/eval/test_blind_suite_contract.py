import json
import subprocess
import sys
import tempfile
import unittest
from collections import Counter
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(ROOT / "scripts"))

from eval_lib.suites import load_suite


class BlindSuiteContractTest(unittest.TestCase):
    def test_blind_suite_is_independent_holdout_shape(self):
        blind = load_suite(ROOT / "eval/suites/mvp-blind.yaml")
        smoke = load_suite(ROOT / "eval/suites/mvp-smoke.yaml")
        balanced = load_suite(ROOT / "eval/suites/mvp-balanced.yaml")

        self.assertEqual(blind["name"], "mvp-blind")
        self.assertEqual(len(blind["scenarios"]), 6)
        blind_ids = {scenario["id"] for scenario in blind["scenarios"]}
        self.assertEqual(len(blind_ids), 6)
        self.assertTrue(blind_ids.isdisjoint({s["id"] for s in smoke["scenarios"]}))
        self.assertTrue(blind_ids.isdisjoint({s["id"] for s in balanced["scenarios"]}))

        sizes = Counter(scenario["size"] for scenario in blind["scenarios"])
        self.assertEqual(sizes, {"small": 2, "medium": 2, "large": 2})
        for scenario in blind["scenarios"]:
            self.assertTrue(scenario.get("blind"), scenario["id"])
            self.assertGreaterEqual(len(scenario["expected_artifacts"]), 1, scenario["id"])
            constraints = scenario.get("plan_constraints", {})
            self.assertIn("min_steps", constraints, scenario["id"])
            self.assertIn("max_steps", constraints, scenario["id"])
            self.assertIn("required_verify_keywords", constraints, scenario["id"])

    def test_blind_suite_dry_run_matrix_and_commands(self):
        with tempfile.TemporaryDirectory() as td:
            run_root = Path(td) / "run"
            result = subprocess.run(
                [
                    sys.executable,
                    "scripts/eval-run.py",
                    "--suite",
                    "eval/suites/mvp-blind.yaml",
                    "--model-profile",
                    "speed-cloud",
                    "--modes",
                    "minimal-loop,step-plan",
                    "--runs",
                    "1",
                    "--run-root",
                    str(run_root),
                    "--dry-run",
                ],
                cwd=ROOT,
                text=True,
                capture_output=True,
            )
            self.assertEqual(result.returncode, 0, result.stderr)
            matrix = json.loads((run_root / "matrix.json").read_text())
            self.assertEqual(len(matrix), 24)
            self.assertEqual({row["suite"] for row in matrix}, {"mvp-blind"})
            self.assertIn(
                "Required final artifacts:",
                "\n".join(path.read_text() for path in (run_root / "runs").glob("*/command.txt")),
            )

    def test_blind_nextjs_contract_keeps_deferred_build_requirement(self):
        import importlib.util

        module_path = ROOT / "scripts/eval-run.py"
        spec = importlib.util.spec_from_file_location("eval_run_blind_contract", module_path)
        eval_run = importlib.util.module_from_spec(spec)
        self.assertIsNotNone(spec.loader)
        spec.loader.exec_module(eval_run)

        suite = load_suite(ROOT / "eval/suites/mvp-blind.yaml")
        scenario = next(s for s in suite["scenarios"] if s["id"] == "nextjs-memory-grid-large")
        contract = eval_run.completion_contract_for_spec(
            {"binary_kind": "anvilminimal", "mode": "minimal-loop", "scenario": scenario}
        )
        self.assertEqual(contract["profile"], "nextjs")
        self.assertEqual(contract["verify_commands"], [])
        self.assertEqual(
            contract["deferred_verify_requirements"][0]["command"], "npm run build"
        )

    def test_blind_docs_contract_remains_artifact_only(self):
        import importlib.util

        module_path = ROOT / "scripts/eval-run.py"
        spec = importlib.util.spec_from_file_location("eval_run_blind_docs_contract", module_path)
        eval_run = importlib.util.module_from_spec(spec)
        self.assertIsNotNone(spec.loader)
        spec.loader.exec_module(eval_run)

        suite = load_suite(ROOT / "eval/suites/mvp-blind.yaml")
        scenario = next(s for s in suite["scenarios"] if s["id"] == "cli-usage-reference-small")
        contract = eval_run.completion_contract_for_spec(
            {"binary_kind": "anvilminimal", "mode": "minimal-loop", "scenario": scenario}
        )
        self.assertEqual(contract["required_paths"], ["docs/CLI_USAGE.md"])
        self.assertEqual(contract["verify_commands"], [])
        self.assertNotIn("profile", contract)


if __name__ == "__main__":
    unittest.main()
