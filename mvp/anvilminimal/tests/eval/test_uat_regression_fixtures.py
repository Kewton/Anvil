import copy
import json
import sys
import unittest
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(ROOT / "scripts"))

from eval_lib.failure_classification import classify_events, failure_layer_for_kind
from eval_lib.uat_regression import summarize_uat_regression_fixture


class UatRegressionFixturesTest(unittest.TestCase):
    def load_fixture(self) -> dict:
        fixture = (
            ROOT
            / "tests/eval/fixtures/uat_001/test0630_001_regression.json"
        )
        return json.loads(fixture.read_text(encoding="utf-8"))

    def load_test0701_fixture(self) -> dict:
        fixture = (
            ROOT
            / "tests/eval/fixtures/uat_002/test0701_004_nextjs_dev_route_failure.json"
        )
        return json.loads(fixture.read_text(encoding="utf-8"))

    def test_test0630_phase_scaffold_failure_is_classified(self):
        fixture = self.load_fixture()
        classified = classify_events(fixture["events"])
        self.assertEqual(classified["failure_kind"], fixture["expected"]["failure_kind"])
        self.assertEqual(classified["planner_stage"], "scaffold")
        self.assertEqual(failure_layer_for_kind(classified["failure_kind"]), "planning")

    def test_test0630_regression_summary_detects_required_failure_shape(self):
        fixture = self.load_fixture()
        summary = summarize_uat_regression_fixture(fixture)
        expected = fixture["expected"]

        self.assertTrue(summary["phase_scaffold_error_detected"])
        self.assertEqual(summary["failed_phase_id"], expected["failed_phase_id"])
        self.assertTrue(summary["verify_command_policy_error_detected"])
        self.assertEqual(summary["verify_policy_error_attempts"], [1, 3])
        self.assertTrue(summary["recovery_prompt_saved"])
        self.assertTrue(summary["recovery_ultra_plan_missing"])
        self.assertTrue(summary["build_pass_browser_fail"])
        self.assertEqual(summary["browser_http_status"], 500)
        self.assertTrue(summary["path_only_early_stop_detected"])
        self.assertGreaterEqual(summary["path_only_stop_count"], 1)

    def test_test0630_fixture_keeps_recovery_yaml_missing_distinct_from_prompt(self):
        fixture = self.load_fixture()
        summary = summarize_uat_regression_fixture(fixture)
        self.assertTrue(summary["recovery_prompt_saved"])
        self.assertTrue(summary["recovery_ultra_plan_missing"])

        fixed = copy.deepcopy(fixture)
        fixed["artifacts"]["recovery_ultra_plans"] = [
            ".anvil/plans/recovery-ultra-plan-example.yaml"
        ]
        fixed_summary = summarize_uat_regression_fixture(fixed)
        self.assertTrue(fixed_summary["recovery_prompt_saved"])
        self.assertFalse(fixed_summary["recovery_ultra_plan_missing"])

        fixed_by_event = copy.deepcopy(fixture)
        for event in fixed_by_event["events"]:
            if event.get("event") == "recovery_prompt_saved":
                event["recovery_ultra_plan_path"] = (
                    ".anvil/plans/recovery-ultra-plan-example.yaml"
                )
                event["recovery_yaml_missing"] = False
        event_summary = summarize_uat_regression_fixture(fixed_by_event)
        self.assertTrue(event_summary["recovery_prompt_saved"])
        self.assertFalse(event_summary["recovery_ultra_plan_missing"])

    def test_test0630_fixture_detects_build_pass_browser_fail(self):
        fixture = self.load_fixture()
        summary = summarize_uat_regression_fixture(fixture)
        self.assertTrue(summary["build_pass_browser_fail"])
        self.assertEqual(summary["browser_failure_kind"], "browser_http_500")

        browser_fixed = copy.deepcopy(fixture)
        browser_fixed["browser"]["ok"] = True
        browser_fixed["browser"]["http_status"] = 200
        self.assertFalse(
            summarize_uat_regression_fixture(browser_fixed)["build_pass_browser_fail"]
        )

    def test_test0701_fixture_detects_build_pass_dev_route_failure(self):
        fixture = self.load_test0701_fixture()
        summary = summarize_uat_regression_fixture(fixture)
        expected = fixture["expected"]

        self.assertTrue(summary["build_pass_browser_fail"])
        self.assertEqual(summary["browser_http_status"], expected["browser_http_status"])
        self.assertEqual(summary["browser_failure_kind"], expected["browser_failure_kind"])
        stages = {
            event.get("stage")
            for event in fixture["events"]
            if event.get("event") == "dev_server_lifecycle"
        }
        self.assertEqual(stages, set(expected["required_dev_server_lifecycle_stages"]))
        run_stop = next(event for event in fixture["events"] if event.get("event") == "run_stop")
        self.assertEqual(run_stop["command_completion_status"], "complete")
        self.assertEqual(run_stop["release_gate_status"], expected["release_gate_status"])
        self.assertNotEqual(run_stop["release_quality_completion"], "release_ready")

    def test_test0630_fixture_detection_is_not_prompt_string_specific(self):
        fixture = self.load_fixture()
        renamed = copy.deepcopy(fixture)
        renamed["id"] = "generic_interactive_app_regression"
        renamed["description"] = "Generic interactive app UAT regression fixture."

        original = summarize_uat_regression_fixture(fixture)
        generic = summarize_uat_regression_fixture(renamed)
        for key in [
            "phase_scaffold_error_detected",
            "verify_command_policy_error_detected",
            "recovery_ultra_plan_missing",
            "build_pass_browser_fail",
            "path_only_early_stop_detected",
        ]:
            self.assertEqual(original[key], generic[key])


if __name__ == "__main__":
    unittest.main()
