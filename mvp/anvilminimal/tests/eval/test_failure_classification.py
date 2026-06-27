import sys
import unittest
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(ROOT / "scripts"))

from eval_lib.failure_classification import (
    capability_failure_included,
    classify_events,
    classify_stderr,
    failure_layer_for_kind,
)


class FailureClassificationTest(unittest.TestCase):
    def test_failure_classification_recognizes_plan_final_contract_failure(self):
        result = classify_events(
            [
                {
                    "event": "plan_final_contract",
                    "ok": False,
                    "missing_final_artifacts": ["README.md"],
                }
            ]
        )
        self.assertEqual(result["failure_kind"], "plan_final_contract_failure")
        self.assertEqual(result["missing_artifacts"], "README.md")

    def test_failure_classification_recognizes_step_obligation_scope_violation(self):
        result = classify_events(
            [
                {
                    "event": "step_obligation_scope",
                    "session_scope": "plan-run-step",
                    "explicit_required_paths": [],
                    "effective_required_paths": ["README.md"],
                    "prompt_extracted_paths_enabled": True,
                    "completion_contract_path_merge_enabled": False,
                    "completion_contract_verification_enabled": False,
                }
            ]
        )
        self.assertEqual(result["failure_kind"], "step_obligation_scope_violation")

    def test_stderr_classifies_plan_final_contract_failure(self):
        result = classify_stderr("error: plan final contract failed: README.md", rc=1)
        self.assertEqual(result["failure_kind"], "plan_final_contract_failure")

    def test_stderr_classifies_step_verify_failure(self):
        result = classify_stderr(
            "error: step create-main-rs failed verification after bounded repair: command failed",
            rc=1,
        )
        self.assertEqual(result["failure_kind"], "step_verify_failure")

    def test_failure_layers_separate_provider_from_capability_failures(self):
        self.assertEqual(failure_layer_for_kind("provider_http_status"), "provider")
        self.assertEqual(capability_failure_included("provider_http_status"), False)
        self.assertEqual(failure_layer_for_kind("planner_schema_error"), "planning")
        self.assertEqual(capability_failure_included("planner_schema_error"), True)
        self.assertEqual(failure_layer_for_kind("tool_validation_error"), "runtime")
        self.assertEqual(capability_failure_included("tool_validation_error"), True)


if __name__ == "__main__":
    unittest.main()
