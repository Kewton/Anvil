import json
import sys
import tempfile
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

    def test_postcheck_http_500_is_browser_failure(self):
        with tempfile.TemporaryDirectory() as td:
            root = Path(td)
            postcheck_dir = root / "run/postcheck"
            postcheck_dir.mkdir(parents=True)
            events_path = postcheck_dir / "events.jsonl"
            events_path.write_text(
                json.dumps({"event": "dev_server", "ready": False, "status": 500}),
                encoding="utf-8",
            )
            result = evaluate_browser_oracle(
                {
                    "profile": "nextjs",
                    "prompt": "Create a keyboard controlled game.",
                },
                root / "workdir",
                postcheck={"events_path": str(events_path)},
            )
        self.assertFalse(result["browser_success"])
        self.assertEqual(result["browser_failure_kind"], "browser_http_500")
        self.assertEqual(result["browser_details"]["status"], "failed")

    def test_ready_route_without_browser_render_is_unavailable_not_pass(self):
        with tempfile.TemporaryDirectory() as td:
            root = Path(td)
            postcheck_dir = root / "run/postcheck"
            postcheck_dir.mkdir(parents=True)
            events_path = postcheck_dir / "events.jsonl"
            events_path.write_text(
                json.dumps({"event": "dev_server", "ready": True, "status": 200}),
                encoding="utf-8",
            )
            result = evaluate_browser_oracle(
                {
                    "profile": "nextjs",
                    "prompt": "Create a keyboard controlled game.",
                },
                root / "workdir",
                postcheck={"events_path": str(events_path)},
            )
        self.assertEqual(result["browser_success"], "")
        self.assertEqual(
            result["browser_details"]["status"],
            "browser_render_or_interaction_evidence_missing",
        )

    def test_saved_browser_evidence_can_pass(self):
        with tempfile.TemporaryDirectory() as td:
            root = Path(td)
            run_dir = root / "run"
            run_dir.mkdir(parents=True)
            (run_dir / "browser-readiness.json").write_text(
                json.dumps({"ok": True, "http_status": 200}),
                encoding="utf-8",
            )
            result = evaluate_browser_oracle(
                {
                    "profile": "nextjs",
                    "prompt": "Create a keyboard controlled game.",
                },
                root / "workdir",
                run_dir=run_dir,
            )
        self.assertTrue(result["browser_success"])
        self.assertEqual(result["browser_details"]["status"], "passed")


if __name__ == "__main__":
    unittest.main()
