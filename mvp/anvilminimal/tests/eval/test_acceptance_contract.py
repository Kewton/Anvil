import sys
import unittest
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(ROOT / "scripts"))

from eval_lib.acceptance_contract import contract_from_scenario


class AcceptanceContractTest(unittest.TestCase):
    def test_infers_interactive_game_from_english_prompt(self):
        contract = contract_from_scenario(
            {
                "id": "renamed-case",
                "profile": "nextjs",
                "prompt": "Create a Next.js Space Invaders game that can run on port 3011.",
            }
        )
        self.assertEqual(contract.category, "interactive-game")
        self.assertIn("player_control", contract.required_capabilities)
        self.assertIn("implementation", contract.required_obligations)
        self.assertIn("acceptance_evidence", contract.required_obligations)
        self.assertIn("static_title_only", contract.forbidden_minimal_outputs)
        self.assertIn("scaffold_only", contract.forbidden_minimal_outputs)
        self.assertIn("style_only", contract.forbidden_minimal_outputs)
        self.assertIn("docs_only", contract.forbidden_minimal_outputs)
        self.assertIn("manifest_only", contract.forbidden_minimal_outputs)
        self.assertEqual(contract.runtime["port"], 3011)

    def test_infers_interactive_game_from_japanese_prompt_without_scenario_id(self):
        contract = contract_from_scenario(
            {
                "id": "opaque-id",
                "profile": "nextjs",
                "prompt": "最高に面白いゲームを3011ポートで起動可能なNext.jsアプリとして作ってください。",
            }
        )
        self.assertEqual(contract.category, "interactive-game")
        self.assertEqual(contract.runtime["port"], 3011)

    def test_explicit_contract_overrides_inference(self):
        contract = contract_from_scenario(
            {
                "prompt": "Create a dashboard",
                "functional_contract": {
                    "category": "interactive-web-app",
                    "required_capabilities": ["user_input_or_action"],
                },
            }
        )
        self.assertEqual(contract.category, "interactive-web-app")
        self.assertEqual(contract.required_capabilities, ["user_input_or_action"])
        self.assertIn("implementation", contract.required_obligations)
        self.assertTrue(contract.explicit)

    def test_docs_contract_uses_acceptance_evidence_obligation_only(self):
        contract = contract_from_scenario({"prompt": "Write README usage docs"})
        self.assertEqual(contract.category, "docs-content")
        self.assertEqual(contract.required_obligations, ["acceptance_evidence"])


if __name__ == "__main__":
    unittest.main()
