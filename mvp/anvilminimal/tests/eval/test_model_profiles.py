import sys
import unittest
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(ROOT / "scripts"))

from eval_lib.models import cli_model_args, load_model_profiles, normalize_model_ref


class ModelProfileTest(unittest.TestCase):
    def test_typo_normalization_warns(self):
        ref, warnings = normalize_model_ref("gollama:qwen3.6:27b-coding-nvfp4")
        self.assertEqual(ref.provider, "ollama")
        self.assertTrue(warnings)

        ref, warnings = normalize_model_ref("gemini:emini-3.5-flash")
        self.assertEqual(ref.raw, "gemini:gemini-3.5-flash")
        self.assertTrue(warnings)

    def test_load_speed_cloud(self):
        profiles, warnings = load_model_profiles(ROOT / "eval/model_profiles.yaml")
        self.assertIn("speed-cloud", profiles)
        self.assertFalse(profiles["speed-cloud"]["runs"][0]["local_llm_used"])
        self.assertEqual(warnings, [])

    def test_cli_args(self):
        profiles, _ = load_model_profiles(ROOT / "eval/model_profiles.yaml")
        run = profiles["speed-cloud"]["runs"][0]
        args = cli_model_args(run["main"], run["planner"])
        self.assertIn("--provider", args)
        self.assertIn("--planner-provider", args)


if __name__ == "__main__":
    unittest.main()

