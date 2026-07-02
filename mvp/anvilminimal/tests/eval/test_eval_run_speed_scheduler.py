import json
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]


class EvalRunSpeedSchedulerTest(unittest.TestCase):
    def test_speed_cloud_5x_dry_run_records_five_way_cloud_contract(self):
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
                    "5",
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
        self.assertTrue(matrix)
        self.assertTrue(all(not row["local_llm_used"] for row in matrix))
        self.assertEqual({row["_model_profile"] for row in matrix}, {"speed-cloud-5x"})
        self.assertEqual({row["_parallel_limit"] for row in matrix}, {5})
        self.assertEqual({row["_provider_limit"] for row in matrix}, {5})


if __name__ == "__main__":
    unittest.main()
