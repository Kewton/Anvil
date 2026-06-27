import sys
import json
import tempfile
import unittest
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(ROOT / "scripts"))

from eval_lib.source_semantic_oracle import evaluate_source_semantics
from eval_lib.simple_yaml import load_yaml


SCENARIO = {
    "profile": "nextjs",
    "prompt": "Create a Next.js Space Invaders game that can run on port 3011.",
    "expected_artifacts": ["src/app/page.tsx"],
}


class SourceSemanticOracleTest(unittest.TestCase):
    def test_static_title_only_game_fails(self):
        with tempfile.TemporaryDirectory() as td:
            workdir = Path(td)
            (workdir / "src/app").mkdir(parents=True)
            (workdir / "src/app/page.tsx").write_text(
                """
export default function Page() {
  return <main><h1>SPACE INVADERS</h1><p>Press any key to start...</p></main>
}
""",
                encoding="utf-8",
            )
            result = evaluate_source_semantics(SCENARIO, workdir)
        self.assertFalse(result["source_semantic_success"], result)
        self.assertEqual(result["source_semantic_failure_kind"], "static_title_only")
        self.assertLess(result["source_semantic_score"], 60)

    def test_playable_core_loop_signals_pass(self):
        with tempfile.TemporaryDirectory() as td:
            workdir = Path(td)
            (workdir / "src/app").mkdir(parents=True)
            (workdir / "src/app/page.tsx").write_text(
                """
"use client";
import { useEffect, useState } from "react";
export default function Page() {
  const [gameState, setGameState] = useState("start");
  const [player, setPlayer] = useState({ x: 10 });
  const [enemies, setEnemies] = useState([{ x: 1, y: 1 }]);
  const [score, setScore] = useState(0);
  const [lives, setLives] = useState(3);
  useEffect(() => {
    const onKeyDown = (event: KeyboardEvent) => {
      if (event.key === "Enter") setGameState("playing");
      if (event.key === "ArrowLeft") setPlayer({ x: player.x - 1 });
    };
    window.addEventListener("keydown", onKeyDown);
    const loop = requestAnimationFrame(() => {
      const collision = enemies.some((enemy) => enemy.x === player.x);
      if (collision) setLives(lives - 1);
      setScore(score + 1);
    });
    return () => { window.removeEventListener("keydown", onKeyDown); cancelAnimationFrame(loop); };
  }, [player, enemies, score, lives]);
  return <canvas aria-label="Space Invaders game" />;
}
""",
                encoding="utf-8",
            )
            result = evaluate_source_semantics(SCENARIO, workdir)
        self.assertTrue(result["source_semantic_success"], result)
        self.assertEqual(result["source_semantic_score"], 100.0)

    def test_acceptance_oracle_fixtures_match_expected_outcomes(self):
        fixture_root = ROOT / "eval/fixtures/acceptance_oracle"
        for fixture in [
            "nextjs_static_title_only",
            "nextjs_good_minimal_game",
            "nextjs_missing_input",
            "nextjs_missing_score",
            "nextjs_placeholder_tokens",
            "docs_missing_content",
            "cli_wrong_behavior",
        ]:
            with self.subTest(fixture=fixture):
                directory = fixture_root / fixture
                scenario = load_yaml(directory / "scenario.yaml")
                expected = json.loads((directory / "expected.json").read_text(encoding="utf-8"))
                result = evaluate_source_semantics(scenario, directory / "workdir")
                if "acceptance_success" in expected:
                    self.assertEqual(result["source_semantic_success"], expected["acceptance_success"], result)
                if "acceptance_failure_kind" in expected:
                    self.assertEqual(result["source_semantic_failure_kind"], expected["acceptance_failure_kind"], result)
                for capability in expected.get("missing_capabilities", []):
                    self.assertIn(
                        capability,
                        result["source_semantic_details"]["missing_capabilities"],
                        result,
                    )


if __name__ == "__main__":
    unittest.main()
