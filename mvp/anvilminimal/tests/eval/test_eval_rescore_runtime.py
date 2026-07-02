import json
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(ROOT / "scripts"))

from eval_lib.run_summary import SUMMARY_HEADER, write_summary, read_summary


class EvalRescoreRuntimeTest(unittest.TestCase):
    def test_rescore_adds_capability_and_verify_details(self):
        with tempfile.TemporaryDirectory() as td:
            root = Path(td)
            run_root = root / "run-root"
            run_dir = run_root / "runs/r1"
            workdir = run_dir / "workdir"
            plans = run_dir / "plans"
            postcheck = run_dir / "postcheck"
            workdir.mkdir(parents=True)
            plans.mkdir(parents=True)
            postcheck.mkdir(parents=True)
            (run_dir / "anvil-events.jsonl").write_text(
                json.dumps({"event": "loop_stop", "reason": "done"}) + "\n",
                encoding="utf-8",
            )
            (postcheck / "events.jsonl").write_text(
                json.dumps({"event": "postcheck", "command": "npm run build", "rc": 0}) + "\n",
                encoding="utf-8",
            )
            (plans / "plan.yaml").write_text(
                """
goal: Build a game.
steps:
  - id: game
    kind: implement
    instruction: Create Canvas game loop with keyboard player control, enemies, bullets, collision, and score.
    expected_paths: [src/app/page.tsx]
    verify: [node smoke-check.js]
""",
                encoding="utf-8",
            )
            (workdir / "src/app").mkdir(parents=True)
            (workdir / "src/app/page.tsx").write_text(
                "canvas requestAnimationFrame keydown player enemy bullet collision score",
                encoding="utf-8",
            )
            (workdir / "smoke-check.js").write_text(
                "canvas requestAnimationFrame keydown player enemy bullet collision score",
                encoding="utf-8",
            )
            row = {key: "" for key in SUMMARY_HEADER}
            row.update(
                {
                    "run_id": "r1",
                    "suite": "mvp-smoke",
                    "scenario": "nextjs-space-invaders-large",
                    "size": "large",
                    "category": "new-code",
                    "mode": "plan-run",
                    "rc": "0",
                    "success": "true",
                    "workdir": str(workdir),
                    "plan_artifacts": "plans/plan.yaml",
                    "extras_json": "{}",
                }
            )
            write_summary(run_root / "summary.eval.tsv", [row])
            out = run_root / "summary.rescored.eval.tsv"
            subprocess.run(
                [
                    sys.executable,
                    str(ROOT / "scripts/eval-rescore-runtime.py"),
                    "--run-root",
                    str(run_root),
                    "--suite",
                    str(ROOT / "eval/suites/mvp-smoke.yaml"),
                    "--out-summary",
                    str(out),
                ],
                check=True,
                cwd=ROOT,
            )
            rescored = read_summary(out)[0]
        self.assertNotEqual(rescored["plan_capability_contract_score"], "")
        self.assertNotEqual(rescored["plan_verify_coverage_score"], "")
        extras = json.loads(rescored["extras_json"])
        self.assertIn("plan_capability_details", extras)
        self.assertIn("plan_verify_details", extras)


if __name__ == "__main__":
    unittest.main()
