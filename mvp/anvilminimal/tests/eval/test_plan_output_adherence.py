import sys
import tempfile
import unittest
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(ROOT / "scripts"))

from eval_lib.plan_output_adherence import evaluate_plan_output_adherence


SCENARIO = {
    "profile": "nextjs",
    "prompt": "Create a Next.js Space Invaders game that can run on port 3011.",
    "expected_artifacts": ["src/app/page.tsx"],
}


PLAN = """
goal: Build a Space Invaders game.
steps:
  - id: game
    kind: implement
    instruction: |
      Create an HTML5 Canvas 60fps game loop with keyboard player controls,
      enemies, bullets, collision handling, score, lives, Web Audio, and particle effects.
    expected_paths:
      - src/app/page.tsx
    verify:
      - npm run build
    expected_result: pass
"""


class PlanOutputAdherenceTest(unittest.TestCase):
    def test_cli_entry_point_does_not_create_game_score_contract(self):
        with tempfile.TemporaryDirectory() as td:
            root = Path(td)
            workdir = root / "workdir"
            run_dir = root / "run"
            (workdir / "src").mkdir(parents=True)
            run_dir.mkdir()
            plan = run_dir / "plan.yaml"
            plan.write_text(
                """
goal: Create a small Rust CLI.
steps:
  - id: implement-main
    kind: implement
    instruction: Create src/main.rs containing a CLI main entry point and one unit test.
    expected_paths: [src/main.rs]
    verify: [cargo test]
    expected_result: pass
""",
                encoding="utf-8",
            )
            (workdir / "src/main.rs").write_text(
                "fn greeting() -> &'static str { \"hello\" } fn main(){ println!(\"{}\", greeting()); }",
                encoding="utf-8",
            )
            result = evaluate_plan_output_adherence(
                plan_paths=[plan],
                workdir=workdir,
                scenario={"expected_artifacts": ["src/main.rs"]},
            )
        self.assertEqual(result["plan_output_adherence_success"], "", result)
        self.assertEqual(result["plan_output_details"]["reason"], "no_plan_output_capabilities")

    def test_static_title_fails_when_plan_requires_game_mechanics(self):
        with tempfile.TemporaryDirectory() as td:
            root = Path(td)
            workdir = root / "workdir"
            run_dir = root / "run"
            (workdir / "src/app").mkdir(parents=True)
            run_dir.mkdir()
            plan = run_dir / "plan.yaml"
            plan.write_text(PLAN, encoding="utf-8")
            (workdir / "src/app/page.tsx").write_text(
                "export default function Page(){return <h1>SPACE INVADERS</h1>}",
                encoding="utf-8",
            )
            result = evaluate_plan_output_adherence(
                plan_paths=[plan],
                workdir=workdir,
                scenario=SCENARIO,
            )
        self.assertFalse(result["plan_output_adherence_success"], result)
        self.assertEqual(result["plan_output_failure_kind"], "plan_output_missing_required_capabilities")
        missing = result["plan_output_details"]["missing_capabilities"]
        self.assertIn("render_loop_or_canvas", missing)
        self.assertIn("keyboard_or_player_control", missing)
        self.assertLess(result["plan_output_adherence_score"], 50)

    def test_game_mechanics_pass_when_plan_claims_are_implemented(self):
        with tempfile.TemporaryDirectory() as td:
            root = Path(td)
            workdir = root / "workdir"
            run_dir = root / "run"
            (workdir / "src/app").mkdir(parents=True)
            run_dir.mkdir()
            plan = run_dir / "plan.yaml"
            plan.write_text(PLAN, encoding="utf-8")
            (workdir / "src/app/page.tsx").write_text(
                """
"use client";
import { useEffect, useRef, useState } from "react";
export default function Page() {
  const canvas = useRef<HTMLCanvasElement>(null);
  const [player, setPlayer] = useState({ x: 10, y: 10 });
  const [enemies, setEnemies] = useState([{ x: 20, y: 20 }]);
  const [bullets, setBullets] = useState([{ x: 10, y: 9 }]);
  const [score, setScore] = useState(0);
  const [lives, setLives] = useState(3);
  const [particles, setParticles] = useState([{ x: 1, y: 1 }]);
  useEffect(() => {
    const audio = new Audio();
    const onKeyDown = (event: KeyboardEvent) => {
      if (event.key === "ArrowLeft") setPlayer({ ...player, x: player.x - 1 });
      if (event.key === " ") setBullets([...bullets, { x: player.x, y: player.y }]);
    };
    window.addEventListener("keydown", onKeyDown);
    const frame = requestAnimationFrame(() => {
      const hit = bullets.some((bullet) =>
        enemies.some((enemy) => bullet.x > enemy.x && bullet.y > enemy.y)
      );
      if (hit) { setScore(score + 100); setLives(lives - 1); }
      setParticles([...particles, { x: score, y: lives }]);
      audio.volume = 0.1;
    });
    return () => { window.removeEventListener("keydown", onKeyDown); cancelAnimationFrame(frame); };
  }, [player, enemies, bullets, score, lives, particles]);
  return <canvas ref={canvas} />;
}
""",
                encoding="utf-8",
            )
            result = evaluate_plan_output_adherence(
                plan_paths=[plan],
                workdir=workdir,
                scenario=SCENARIO,
            )
        self.assertTrue(result["plan_output_adherence_success"], result)
        self.assertEqual(result["plan_output_adherence_score"], 100.0)


if __name__ == "__main__":
    unittest.main()
