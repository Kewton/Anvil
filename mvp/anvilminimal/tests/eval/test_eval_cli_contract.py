import json
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]


class EvalCliContractTest(unittest.TestCase):
    def test_speed_cloud_5x_and_provider_limit_override_are_recorded_in_matrix(self):
        with tempfile.TemporaryDirectory() as td:
            run_root = Path(td) / "run"
            result = subprocess.run(
                [
                    sys.executable,
                    "scripts/eval-run.py",
                    "--suite",
                    "eval/suites/mvp-smoke.yaml",
                    "--model-profile",
                    "speed-cloud-5x",
                    "--modes",
                    "minimal-loop",
                    "--runs",
                    "1",
                    "--parallel",
                    "5",
                    "--provider-limit",
                    "4",
                    "--run-root",
                    str(run_root),
                    "--dry-run",
                ],
                cwd=ROOT,
                text=True,
                capture_output=True,
            )
            self.assertEqual(result.returncode, 0, result.stderr)
            matrix = json.loads((run_root / "matrix.json").read_text(encoding="utf-8"))
        self.assertTrue(matrix, "matrix should not be empty")
        self.assertEqual({item["_model_profile"] for item in matrix}, {"speed-cloud-5x"})
        self.assertEqual({item["_parallel_limit"] for item in matrix}, {5})
        self.assertEqual({item["_provider_limit"] for item in matrix}, {4})

    def test_anvildev_engine_minimal_is_rendered_in_child_command_not_eval_run_cli(self):
        with tempfile.TemporaryDirectory() as td:
            run_root = Path(td) / "run"
            result = subprocess.run(
                [
                    sys.executable,
                    "scripts/eval-run.py",
                    "--suite",
                    "eval/suites/mvp-smoke.yaml",
                    "--model-profile",
                    "speed-cloud-5x",
                    "--modes",
                    "minimal-loop",
                    "--scenario",
                    "nextjs-space-invaders-large",
                    "--runs",
                    "1",
                    "--binary",
                    "anvildev",
                    "--binary-kind",
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
            commands = "\n".join(path.read_text(encoding="utf-8") for path in (run_root / "runs").glob("*/command.txt"))
        self.assertIn('"--engine" "minimal"', commands)

    def test_provider_smoke_suite_records_provider_probe_metadata(self):
        with tempfile.TemporaryDirectory() as td:
            run_root = Path(td) / "run"
            result = subprocess.run(
                [
                    sys.executable,
                    "scripts/eval-run.py",
                    "--suite",
                    "eval/suites/mvp-provider-smoke.yaml",
                    "--model-profile",
                    "openai-only",
                    "--modes",
                    "minimal-loop",
                    "--runs",
                    "1",
                    "--scenario",
                    "write-one-file-small",
                    "--run-root",
                    str(run_root),
                    "--dry-run",
                ],
                cwd=ROOT,
                text=True,
                capture_output=True,
            )
            self.assertEqual(result.returncode, 0, result.stderr)
            matrix = json.loads((run_root / "matrix.json").read_text(encoding="utf-8"))
        probe = matrix[0]["provider_probe"]
        self.assertTrue(probe["required_for_prompt_sensitive_fix"])
        self.assertIn("provider_probe", probe["command"])
        self.assertEqual(
            {item["provider"] for item in probe["probes"]},
            {"openai", "gemini", "ollama"},
        )


if __name__ == "__main__":
    unittest.main()
