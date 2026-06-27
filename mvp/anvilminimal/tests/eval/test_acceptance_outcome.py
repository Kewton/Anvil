import json
import sys
import tempfile
import unittest
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(ROOT / "scripts"))

from eval_lib.acceptance_outcome import evaluate_acceptance_outcome


SCENARIO = {
    "profile": "nextjs",
    "prompt": "Create a Next.js Space Invaders game that can run on port 3011.",
    "expected_artifacts": ["package.json", "src/app/page.tsx", "src/app/layout.tsx", "src/app/global.d.ts"],
    "postcheck": {
        "commands": ["npm install --ignore-scripts", "npm run build"],
        "dev_server": {"port": 3011},
    },
}


class AcceptanceOutcomeTest(unittest.TestCase):
    def test_legacy_success_static_title_becomes_acceptance_false_positive(self):
        with tempfile.TemporaryDirectory() as td:
            root = Path(td)
            workdir = root / "workdir"
            run_dir = root / "run"
            (workdir / "src/app").mkdir(parents=True)
            (run_dir / "postcheck").mkdir(parents=True)
            (workdir / "package.json").write_text("{}", encoding="utf-8")
            (workdir / "src/app/page.tsx").write_text(
                "export default function Page(){return <h1>SPACE INVADERS</h1>}",
                encoding="utf-8",
            )
            (workdir / "src/app/layout.tsx").write_text("export default function Layout({children}){return children}", encoding="utf-8")
            (workdir / "src/app/global.d.ts").write_text("declare module '*.css';", encoding="utf-8")
            events = [
                {"event": "postcheck", "command": "npm install --ignore-scripts", "rc": 0},
                {"event": "postcheck", "command": "npm run build", "rc": 0},
                {"event": "dev_server", "ready": True},
            ]
            events_path = run_dir / "postcheck/events.jsonl"
            events_path.write_text("\n".join(json.dumps(event) for event in events), encoding="utf-8")
            outcome = evaluate_acceptance_outcome(
                scenario=SCENARIO,
                workdir=workdir,
                run_dir=run_dir,
                mode="ultra-plan-run",
                process_success=True,
                legacy_success=True,
                postcheck={"ok": True, "events_path": str(events_path)},
            )
        self.assertTrue(outcome["legacy_success"])
        self.assertFalse(outcome["acceptance_success"], outcome)
        self.assertTrue(outcome["acceptance_false_positive"], outcome)
        self.assertEqual(outcome["acceptance_failure_kind"], "static_title_only")
        self.assertEqual(outcome["oracle_gap_kind"], "postcheck_too_weak_for_semantic_contract")

    def test_plan_output_gap_becomes_acceptance_false_positive(self):
        with tempfile.TemporaryDirectory() as td:
            root = Path(td)
            workdir = root / "workdir"
            run_dir = root / "run"
            (workdir / "src/app").mkdir(parents=True)
            (run_dir / "postcheck").mkdir(parents=True)
            (run_dir / "plans").mkdir(parents=True)
            plan = run_dir / "plans/plan.yaml"
            plan.write_text(
                """
goal: Build a game.
steps:
  - id: game
    kind: implement
    instruction: Create a Canvas game loop with keyboard controls, enemies, bullets, collision, and score.
    expected_paths: [src/app/page.tsx]
    verify: [npm run build]
    expected_result: pass
""",
                encoding="utf-8",
            )
            (workdir / "package.json").write_text("{}", encoding="utf-8")
            (workdir / "src/app/page.tsx").write_text(
                '"use client"; export default function Page(){return <h1>SPACE INVADERS</h1>}',
                encoding="utf-8",
            )
            (workdir / "src/app/layout.tsx").write_text("export default function Layout({children}){return children}", encoding="utf-8")
            (workdir / "src/app/global.d.ts").write_text("declare module '*.css';", encoding="utf-8")
            events = [
                {"event": "postcheck", "command": "npm install --ignore-scripts", "rc": 0},
                {"event": "postcheck", "command": "npm run build", "rc": 0},
                {"event": "dev_server", "ready": True},
            ]
            events_path = run_dir / "postcheck/events.jsonl"
            events_path.write_text("\n".join(json.dumps(event) for event in events), encoding="utf-8")
            outcome = evaluate_acceptance_outcome(
                scenario=SCENARIO,
                workdir=workdir,
                run_dir=run_dir,
                mode="plan-run",
                process_success=True,
                legacy_success=True,
                postcheck={"ok": True, "events_path": str(events_path)},
                plan_paths=[plan],
            )
        self.assertFalse(outcome["acceptance_success"], outcome)
        self.assertTrue(outcome["acceptance_false_positive"], outcome)
        self.assertEqual(outcome["acceptance_failure_kind"], "plan_output_missing_required_capabilities")
        self.assertEqual(outcome["oracle_gap_kind"], "postcheck_too_weak_for_plan_contract")
        self.assertFalse(outcome["plan_output_adherence_success"], outcome)


if __name__ == "__main__":
    unittest.main()
