import json
import sys
import unittest
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(ROOT / "scripts"))

from eval_lib.failure_classification import classify_events, classify_stderr, known_failure_kind
from eval_lib.redaction import redact_text


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

    def test_minimal_loop_eval_failure_fixture_documents_countermeasures(self):
        fixture = ROOT / "eval/fixtures/minimal_loop_failures/20260625_speed_cloud.json"
        entries = json.loads(fixture.read_text(encoding="utf-8"))
        self.assertEqual(len(entries), 2)
        by_run = {entry["run_id"]: entry for entry in entries}

        postcheck = by_run[
            "mvp-smoke__python-markdown-linter-medium__minimal-loop__openai-gpt-5.4-mini__gemini-gemini-3.5-flash__r1"
        ]
        classified = classify_events(postcheck["events"])
        self.assertEqual(classified["failure_kind"], "postcheck_failure")
        self.assertEqual(postcheck["expected_failure_kind_after_fix"], "verify_repair_exhausted")

        provider = by_run[
            "mvp-smoke__nextjs-space-invaders-large__minimal-loop__openai-gpt-5.4-mini__gemini-gemini-3.5-flash__r1"
        ]
        classified = classify_events(provider["events"])
        self.assertEqual(classified["failure_kind"], "provider_transient_exhausted")
        self.assertEqual(provider["expected_failure_kind_after_fix"], "provider_transient_exhausted")

    def test_planner_20260625_unclassified_fixture_is_classified(self):
        fixture = ROOT / "eval/fixtures/planner_failures/20260625_speed_cloud_unclassified.json"
        entries = json.loads(fixture.read_text(encoding="utf-8"))
        self.assertEqual(len(entries), 28)
        for entry in entries:
            with self.subTest(entry["id"]):
                classified = classify_stderr(entry["stderr"], rc=1, timeout=False)
                self.assertEqual(classified["failure_kind"], entry["expected_failure_kind"])
                self.assertTrue(known_failure_kind(classified["failure_kind"]))

    def test_minimal_loop_004_fixture_is_classified(self):
        fixture = ROOT / "tests/eval/fixtures/minimal_loop_004/failure_events.json"
        entries = json.loads(fixture.read_text(encoding="utf-8"))
        self.assertEqual(len(entries), 5)
        for entry in entries:
            with self.subTest(entry["id"]):
                classified = classify_events(entry["events"])
                self.assertEqual(classified["failure_kind"], entry["expected_failure_kind"])
                self.assertTrue(known_failure_kind(classified["failure_kind"]))

    def test_planner_error_event_wins_over_stderr(self):
        classified = classify_events(
            [
                {
                    "event": "planner_error",
                    "planner_stage": "lint",
                    "planner_error_kind": "planner_lint_error",
                    "planner_error_message": "implement step must declare concrete expected paths",
                    "planner_provider": "openai",
                    "planner_model": "gpt-5.4-mini",
                    "repair_attempt": 2,
                }
            ]
        )
        self.assertEqual(classified["failure_kind"], "planner_lint_error")
        self.assertEqual(classified["planner_stage"], "lint")
        self.assertEqual(classified["planner_repair_attempts"], 2)

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
        self.assertEqual(provider["failure_kind"], "provider_model_unavailable")
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

        tool_exec = classify_events(
            [
                {
                    "event": "tool_execute",
                    "name": "Read",
                    "status": "error",
                    "error_kind": "tool_execution_error",
                }
            ]
        )
        self.assertEqual(tool_exec["failure_kind"], "tool_execution_error")
        self.assertEqual(tool_exec["tool_name"], "Read")

    def test_classifies_verify_repair_exhausted_from_events(self):
        classified = classify_events(
            [
                {
                    "event": "loop_stop",
                    "reason": "verify_repair_exhausted",
                    "verify_attempts": 2,
                }
            ]
        )
        self.assertEqual(classified["failure_kind"], "verify_repair_exhausted")
        self.assertTrue(known_failure_kind(classified["failure_kind"]))

    def test_classifies_artifact_recovery_exhausted_from_events(self):
        classified = classify_events(
            [
                {
                    "event": "loop_stop",
                    "reason": "artifact_recovery_exhausted",
                    "missing_paths": ["date-helper.js"],
                }
            ]
        )
        self.assertEqual(classified["failure_kind"], "artifact_recovery_exhausted")
        self.assertTrue(known_failure_kind(classified["failure_kind"]))

    def test_classifies_verify_repair_no_change_from_events(self):
        classified = classify_events(
            [
                {
                    "event": "loop_stop",
                    "reason": "verify_repair_no_change",
                    "failure_signature": "commands=assertion_failure",
                }
            ]
        )
        self.assertEqual(classified["failure_kind"], "verify_repair_no_change")
        self.assertTrue(known_failure_kind(classified["failure_kind"]))

    def test_classifies_repair_progress_from_events(self):
        classified = classify_events(
            [
                {
                    "event": "loop_stop",
                    "reason": "verify_repair_progress_unchanged",
                    "repair_progress": "unchanged",
                }
            ]
        )
        self.assertEqual(classified["failure_kind"], "verify_repair_progress_unchanged")
        self.assertEqual(classified["repair_progress"], "unchanged")
        self.assertTrue(known_failure_kind(classified["failure_kind"]))

    def test_classifies_test_discovery_failure(self):
        classified = classify_events(
            [
                {
                    "event": "loop_stop",
                    "reason": "test_discovery_failure",
                    "primary_reason": "test_discovery_failure:no_tests_ran",
                }
            ]
        )
        self.assertEqual(classified["failure_kind"], "test_discovery_failure")
        self.assertTrue(known_failure_kind(classified["failure_kind"]))

        stderr = classify_stderr("error: completion contract verify failed: NO TESTS RAN", rc=1)
        self.assertEqual(stderr["failure_kind"], "test_discovery_failure")

    def test_classifies_provider_transient_exhausted(self):
        classified = classify_events(
            [
                {
                    "event": "provider_error",
                    "provider": "openai",
                    "status": 500,
                    "error_kind": "http_status",
                    "attempt": 2,
                }
            ]
        )
        self.assertEqual(classified["failure_kind"], "provider_transient_exhausted")
        self.assertEqual(classified["provider_http_status"], 500)
        self.assertEqual(classified["provider_attempts"], 2)
        self.assertTrue(known_failure_kind(classified["failure_kind"]))

    def test_stderr_http_5xx_fallback_classifies_transient(self):
        classified = classify_stderr("error: OpenAI Responses API failed: 500 Internal Server Error", rc=1)
        self.assertEqual(classified["failure_kind"], "provider_transient_exhausted")

    def test_fixture_redaction_removes_provider_keys_and_home_paths(self):
        redacted = redact_text("sk-1234567890abcdef /Users/example request_id: req_abc")
        self.assertIn("sk-<REDACTED>", redacted)
        self.assertIn("request_id=<REDACTED>", redacted)
        self.assertNotIn("sk-1234567890abcdef", redacted)


if __name__ == "__main__":
    unittest.main()
