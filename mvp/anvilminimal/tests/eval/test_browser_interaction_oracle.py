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
                json.dumps(
                    {
                        "ok": True,
                        "http_status": 200,
                        "route_rendered": True,
                        "start_transition": True,
                        "interaction_performed": True,
                        "input_event_observed": True,
                        "input_state_change": True,
                        "state_changed": True,
                    }
                ),
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

    def test_saved_browser_ok_without_render_or_interaction_detail_is_unavailable(self):
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
        self.assertEqual(result["browser_success"], "")
        self.assertEqual(
            result["browser_details"]["status"],
            "browser_render_or_interaction_evidence_missing",
        )

    def test_saved_browser_unavailable_is_not_browser_failure(self):
        with tempfile.TemporaryDirectory() as td:
            root = Path(td)
            run_dir = root / "run"
            run_dir.mkdir(parents=True)
            (run_dir / "browser-readiness.json").write_text(
                json.dumps(
                    {
                        "status": "unavailable",
                        "ok": False,
                        "browser_failure_kind": "port_in_use",
                    }
                ),
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
        self.assertEqual(result["browser_success"], "")
        self.assertEqual(result["browser_failure_kind"], "")
        self.assertEqual(result["browser_details"]["status"], "port_in_use")

    def test_saved_tailwind_dev_route_500_keeps_pipeline_failure_kind(self):
        with tempfile.TemporaryDirectory() as td:
            root = Path(td)
            run_dir = root / "run"
            run_dir.mkdir(parents=True)
            (run_dir / "browser-readiness.json").write_text(
                json.dumps(
                    {
                        "ok": False,
                        "http_status": 500,
                        "browser_failure_kind": "tailwind_dev_pipeline_failure",
                        "body_excerpt": "Module parse failed: Unexpected character '@' (1:0)\n@tailwind base;",
                    }
                ),
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
        self.assertFalse(result["browser_success"])
        self.assertEqual(result["browser_failure_kind"], "tailwind_dev_pipeline_failure")
        self.assertEqual(result["browser_details"]["http_status"], 500)

    def test_saved_browser_canvas_unavailable_is_failure(self):
        with tempfile.TemporaryDirectory() as td:
            root = Path(td)
            run_dir = root / "run"
            run_dir.mkdir(parents=True)
            (run_dir / "browser-readiness.json").write_text(
                json.dumps(
                    {
                        "ok": True,
                        "http_status": 200,
                        "route_rendered": True,
                        "interaction_performed": True,
                        "canvas_found": False,
                    }
                ),
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
        self.assertFalse(result["browser_success"])
        self.assertEqual(result["browser_failure_kind"], "canvas_unavailable")


if __name__ == "__main__":
    unittest.main()
