import json
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]


class EvalRunDryTest(unittest.TestCase):
    def test_dry_run_writes_matrix_and_commands(self):
        with tempfile.TemporaryDirectory() as td:
            run_root = Path(td) / "run"
            result = subprocess.run(
                [
                    sys.executable,
                    "scripts/eval-run.py",
                    "--suite",
                    "eval/suites/mvp-smoke.yaml",
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
            command_files = list((run_root / "runs").glob("*/command.txt"))
            self.assertEqual(len(command_files), 24)
            commands = "\n".join(path.read_text() for path in command_files)
            self.assertIn("--prompt", commands)
            self.assertIn("--plan-steps", commands)
            self.assertIn("Required final artifacts:", commands)

    def test_expected_artifacts_can_be_rendered_as_required_final_artifacts(self):
        sys.path.insert(0, str(ROOT / "scripts"))
        from eval_lib.suites import prompt_with_required_final_artifacts

        prompt = prompt_with_required_final_artifacts(
            {
                "prompt": "Create the app",
                "expected_artifacts": ["package.json", "src/app/page.tsx"],
            }
        )
        self.assertIn("Required final artifacts:", prompt)
        self.assertIn("- package.json", prompt)
        self.assertIn("- src/app/page.tsx", prompt)

    def test_completion_contract_for_minimal_loop_splits_verify_from_setup(self):
        sys.path.insert(0, str(ROOT / "scripts"))
        import importlib.util

        module_path = ROOT / "scripts/eval-run.py"
        spec = importlib.util.spec_from_file_location("eval_run", module_path)
        eval_run = importlib.util.module_from_spec(spec)
        self.assertIsNotNone(spec.loader)
        spec.loader.exec_module(eval_run)

        contract = eval_run.completion_contract_for_spec(
            {
                "mode": "minimal-loop",
                "scenario": {
                    "expected_artifacts": ["package.json", "src/app/page.tsx"],
                    "postcheck": {
                        "commands": [
                            "npm install --ignore-scripts",
                            "npm run build",
                            "python3 -m unittest test_app.py",
                        ]
                    },
                },
            }
        )
        self.assertEqual(contract["required_paths"], ["package.json", "src/app/page.tsx"])
        self.assertEqual(contract["verify_commands"], ["python3 -m unittest test_app.py"])

    def test_completion_contract_arg_is_inserted_before_prompt(self):
        sys.path.insert(0, str(ROOT / "scripts"))
        import importlib.util

        module_path = ROOT / "scripts/eval-run.py"
        spec = importlib.util.spec_from_file_location("eval_run", module_path)
        eval_run = importlib.util.module_from_spec(spec)
        self.assertIsNotNone(spec.loader)
        spec.loader.exec_module(eval_run)

        command = eval_run.inject_completion_contract_arg(
            ["anvilminimal", "--yes", "--prompt", "do it"],
            Path("/tmp/contract.json"),
        )
        self.assertLess(command.index("--completion-contract-json"), command.index("--prompt"))


if __name__ == "__main__":
    unittest.main()
