import sys
import unittest
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(ROOT / "scripts"))

from eval_lib.browser_oracle import evaluate_browser_oracle


class BrowserInteractionOracleTest(unittest.TestCase):
    def test_browser_oracle_is_explicit_adapter_not_smoke_dependency(self):
        result = evaluate_browser_oracle(
            {
                "profile": "nextjs",
                "prompt": "Create a keyboard controlled game.",
            },
            ROOT,
            enabled=False,
        )
        self.assertEqual(result["browser_success"], "")
        self.assertEqual(result["browser_details"]["status"], "not_enabled")


if __name__ == "__main__":
    unittest.main()
