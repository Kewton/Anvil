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
            self.assertIn('"--chat-retries" "2"', commands)

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
                "binary_kind": "anvilminimal",
                "mode": "minimal-loop",
                "scenario": {
                    "profile": "nextjs",
                    "prompt": "Create a Next.js app",
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
        self.assertEqual(contract["profile"], "nextjs")
        self.assertEqual(
            contract["deferred_verify_requirements"],
            [
                {
                    "command": "npm run build",
                    "reason": "requires dependency setup",
                    "authority": "postcheck",
                    "profile": "nextjs",
                    "status": "blocked_by_dependency_setup",
                }
            ],
        )

    def test_completion_contract_for_docs_only_does_not_force_profile_or_verify(self):
        sys.path.insert(0, str(ROOT / "scripts"))
        import importlib.util

        module_path = ROOT / "scripts/eval-run.py"
        spec = importlib.util.spec_from_file_location("eval_run_docs", module_path)
        eval_run = importlib.util.module_from_spec(spec)
        self.assertIsNotNone(spec.loader)
        spec.loader.exec_module(eval_run)

        contract = eval_run.completion_contract_for_spec(
            {
                "binary_kind": "anvilminimal",
                "mode": "minimal-loop",
                "scenario": {
                    "profile": "generic",
                    "prompt": "Update docs",
                    "expected_artifacts": ["README.md"],
                    "postcheck": {"commands": []},
                },
            }
        )
        self.assertEqual(contract["required_paths"], ["README.md"])
        self.assertEqual(contract["verify_commands"], [])
        self.assertNotIn("profile", contract)
        self.assertNotIn("deferred_verify_requirements", contract)

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

    def test_speed_cloud_profile_sets_chat_retries_without_cli_default_change(self):
        sys.path.insert(0, str(ROOT / "scripts"))
        from eval_lib.models import load_model_profiles
        from eval_lib.matrix import render_command

        profiles, _ = load_model_profiles(ROOT / "eval/model_profiles.yaml")
        self.assertEqual(profiles["speed-cloud"]["chat_retries"], 2)
        command = render_command(
            binary="anvilminimal",
            binary_kind="anvilminimal",
            mode="minimal-loop",
            scenario={"prompt": "do it"},
            main=profiles["speed-cloud"]["runs"][0]["main"],
            planner=profiles["speed-cloud"]["runs"][0]["planner"],
            context_budget=65536,
            workdir=Path("workdir"),
            chat_retries=profiles["speed-cloud"]["chat_retries"],
        )
        self.assertIn("--chat-retries", command)
        self.assertEqual(command[command.index("--chat-retries") + 1], "2")

    def test_anvilminimal_ultra_plan_run_does_not_duplicate_profile(self):
        sys.path.insert(0, str(ROOT / "scripts"))
        from eval_lib.models import load_model_profiles
        from eval_lib.matrix import render_command

        profiles, _ = load_model_profiles(ROOT / "eval/model_profiles.yaml")
        command = render_command(
            binary="anvilminimal",
            binary_kind="anvilminimal",
            mode="ultra-plan-run",
            scenario={"prompt": "do it", "profile": "nextjs"},
            main=profiles["speed-cloud"]["runs"][0]["main"],
            planner=profiles["speed-cloud"]["runs"][0]["planner"],
            context_budget=65536,
            workdir=Path("workdir"),
            chat_retries=profiles["speed-cloud"]["chat_retries"],
        )
        self.assertEqual(command.count("--profile"), 1, command)
        self.assertLess(command.index("--profile"), command.index("--ultra-plan-run"))

    def test_anvildev_dry_run_uses_source_cli_dialect(self):
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
                    "minimal-loop,step-plan,plan-run,ultra-plan-run,ultra-step-run",
                    "--scenario",
                    "nextjs-space-invaders-large",
                    "--runs",
                    "1",
                    "--binary",
                    "anvildev",
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
            self.assertEqual({row["binary_kind"] for row in matrix}, {"anvildev"})
            command_files = list((run_root / "runs").glob("*/command.txt"))
            commands = "\n".join(path.read_text() for path in command_files)
            self.assertIn('"--engine" "minimal"', commands)
            self.assertIn('"--plan-run"', commands)
            self.assertIn('"--ultra-plan-run"', commands)
            self.assertIn('"--run-plan" "<phase-step-plan.yaml>"', commands)
            self.assertNotIn("--completion-contract-json", commands)

            ultra = next(row["command"] for row in matrix if row["mode"] == "ultra-plan-run")
            self.assertTrue(
                ultra[ultra.index("--ultra-plan-run") + 1].startswith("Create a Next.js"),
                ultra,
            )
            self.assertGreater(ultra.index("--profile"), ultra.index("--ultra-plan-run"))

    def test_scenario_filter_is_single_id(self):
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
                    "step-plan",
                    "--scenario",
                    "docs-heading-update-small,python-markdown-linter-medium",
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
            self.assertNotEqual(result.returncode, 0)
            self.assertIn("scenario not found", result.stderr + result.stdout)

    def test_completion_contract_is_anvilminimal_only(self):
        sys.path.insert(0, str(ROOT / "scripts"))
        import importlib.util

        module_path = ROOT / "scripts/eval-run.py"
        spec = importlib.util.spec_from_file_location("eval_run", module_path)
        eval_run = importlib.util.module_from_spec(spec)
        self.assertIsNotNone(spec.loader)
        spec.loader.exec_module(eval_run)

        contract = eval_run.completion_contract_for_spec(
            {
                "binary_kind": "anvildev",
                "mode": "minimal-loop",
                "scenario": {
                    "expected_artifacts": ["app.py"],
                    "postcheck": {"commands": ["python3 -m unittest test_app.py"]},
                },
            }
        )
        self.assertIsNone(contract)

    def test_postcheck_oracle_defaults_to_fixed(self):
        sys.path.insert(0, str(ROOT / "scripts"))
        from eval_lib.postcheck import run_postcheck

        with tempfile.TemporaryDirectory() as td:
            workdir = Path(td) / "work"
            out = Path(td) / "out"
            workdir.mkdir()
            result = run_postcheck(
                {"expected_artifacts": [], "postcheck": {"commands": []}},
                workdir,
                out,
            )
            self.assertTrue(result["ok"])
            self.assertEqual(result["oracle_kind"], "fixed")


if __name__ == "__main__":
    unittest.main()
