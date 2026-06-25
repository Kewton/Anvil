import json
import sys
import unittest
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(ROOT / "scripts"))

from eval_lib.failure_classification import classify_events, classify_stderr, known_failure_kind


class FailureSnapshotClassificationTest(unittest.TestCase):
    def test_minimal_loop_20260625_failures_are_classified(self):
        fixture = ROOT / "eval/fixtures/provider_failures/minimal_loop_20260625.json"
        entries = json.loads(fixture.read_text(encoding="utf-8"))
        self.assertEqual(len(entries), 12)
        for entry in entries:
            with self.subTest(entry["run_id"]):
                classified = classify_stderr(entry["stderr"], rc=1, timeout=False)
                self.assertEqual(classified["failure_kind"], entry["expected_failure_kind"])
                self.assertTrue(known_failure_kind(classified["failure_kind"]))

    def test_unknown_process_failure_is_explicitly_unclassified(self):
        classified = classify_stderr("error: something opaque", rc=1, timeout=False)
        self.assertEqual(classified["failure_kind"], "unclassified_process_failure")

    def test_failure_kind_max_iterations_snapshot(self):
        classified = classify_stderr(
            "error: minimal loop reached max_iterations (12)", rc=1, timeout=False
        )
        self.assertEqual(classified["failure_kind"], "max_iterations")
        self.assertTrue(known_failure_kind(classified["failure_kind"]))

    def test_failure_kind_missing_tool_call_snapshot(self):
        classified = classify_stderr(
            "error: missing tool call for action prompt after feedback",
            rc=1,
            timeout=False,
        )
        self.assertEqual(classified["failure_kind"], "missing_tool_call")
        self.assertTrue(known_failure_kind(classified["failure_kind"]))

    def test_events_take_provider_and_tool_failure_shapes(self):
        provider = classify_events(
            [
                {
                    "event": "provider_error",
                    "provider": "gemini",
                    "status": 404,
                    "error_kind": "http_status",
                }
            ]
        )
        self.assertEqual(provider["failure_kind"], "provider_http_status")
        self.assertEqual(provider["provider_http_status"], 404)

        tool = classify_events(
            [
                {
                    "event": "tool_validation_error",
                    "name": "Grep",
                    "error_kind": "missing_arg",
                }
            ]
        )
        self.assertEqual(tool["failure_kind"], "tool_validation_error")
        self.assertEqual(tool["tool_error_kind"], "missing_arg")


if __name__ == "__main__":
    unittest.main()
