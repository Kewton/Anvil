import sys
import tempfile
import unittest
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(ROOT / "scripts"))

from eval_lib.acceptance_contract import contract_from_scenario
from eval_lib.source_semantic_oracle import evaluate_source_semantics


class FalsePositiveRegressionTest(unittest.TestCase):
    def test_space_invaders_static_screen_is_stage_a_failure_without_browser(self):
        scenario = {
            "id": "any-id",
            "profile": "nextjs",
            "prompt": "Create a Next.js Space Invaders game that can run on port 3011.",
            "expected_artifacts": ["src/app/page.tsx"],
        }
        with tempfile.TemporaryDirectory() as td:
            workdir = Path(td)
            (workdir / "src/app").mkdir(parents=True)
            (workdir / "src/app/page.tsx").write_text(
                "export default function Page(){return <main><h1>SPACE INVADERS</h1><p>Press any key to start...</p></main>}",
                encoding="utf-8",
            )
            contract = contract_from_scenario(scenario)
            result = evaluate_source_semantics(scenario, workdir, contract)
        self.assertEqual(contract.category, "interactive-game")
        self.assertFalse(result["source_semantic_success"])
        self.assertEqual(result["source_semantic_failure_kind"], "static_title_only")


if __name__ == "__main__":
    unittest.main()
