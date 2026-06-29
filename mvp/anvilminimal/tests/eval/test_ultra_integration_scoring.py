import json
import sys
import tempfile
import unittest
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(ROOT / "scripts"))

from eval_lib.acceptance_outcome import evaluate_acceptance_outcome
from eval_lib.runtime_scoring import score_runtime_health


class UltraIntegrationScoringTest(unittest.TestCase):
    def test_phase_completion_does_not_imply_final_acceptance(self):
        events = [
            {"event": "ultra_phase_start", "phase_id": "game", "total_phases": 1},
            {"event": "ultra_phase_scaffold_complete", "phase_id": "game", "total_phases": 1},
            {"event": "ultra_phase_execute_complete", "phase_id": "game", "total_phases": 1},
            {"event": "ultra_phase_profile_check", "phase_id": "game", "total_phases": 1, "ok": True},
            {"event": "ultra_phase_complete", "phase_id": "game", "total_phases": 1},
            {"event": "ultra_plan_complete", "total_phases": 1, "ok": True},
        ]
        scenario = {
            "profile": "nextjs",
            "prompt": "Create a playable Space Invaders game that can run on port 3011.",
            "expected_artifacts": ["package.json", "src/app/page.tsx", "src/app/layout.tsx", "src/app/global.d.ts"],
            "postcheck": {
                "commands": ["npm install --ignore-scripts", "npm run build"],
                "dev_server": {"port": 3011},
            },
        }
        with tempfile.TemporaryDirectory() as td:
            root = Path(td)
            workdir = root / "workdir"
            run_dir = root / "run"
            postcheck_dir = run_dir / "postcheck"
            (workdir / "src/app").mkdir(parents=True)
            postcheck_dir.mkdir(parents=True)
            (workdir / "package.json").write_text("{}", encoding="utf-8")
            (workdir / "src/app/page.tsx").write_text(
                "export default function Page(){return <h1>SPACE INVADERS</h1>}",
                encoding="utf-8",
            )
            (workdir / "src/app/layout.tsx").write_text(
                "export default function Layout({children}){return children}",
                encoding="utf-8",
            )
            (workdir / "src/app/global.d.ts").write_text("declare module '*.css';", encoding="utf-8")
            events_path = postcheck_dir / "events.jsonl"
            postcheck_events = [
                {"event": "postcheck", "command": "npm install --ignore-scripts", "rc": 0},
                {"event": "postcheck", "command": "npm run build", "rc": 0},
                {"event": "dev_server", "ready": True},
            ]
            events_path.write_text("\n".join(json.dumps(event) for event in postcheck_events), encoding="utf-8")
            runtime = score_runtime_health(
                events,
                mode="ultra-plan-run",
                success=True,
                scenario=scenario,
                workdir=workdir,
            )
            acceptance = evaluate_acceptance_outcome(
                scenario=scenario,
                workdir=workdir,
                run_dir=run_dir,
                mode="ultra-plan-run",
                process_success=True,
                legacy_success=True,
                postcheck={"ok": True, "events_path": str(events_path)},
            )
        self.assertEqual(runtime["phase_completion_score"], 100.0)
        self.assertFalse(acceptance["acceptance_success"], acceptance)
        self.assertTrue(acceptance["acceptance_false_positive"], acceptance)


if __name__ == "__main__":
    unittest.main()
