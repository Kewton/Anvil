import sys
import unittest
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(ROOT / "scripts"))

from eval_lib.failure_classification import (
    blank_failure_kind_gate_violations,
    capability_failure_included,
    classify_events,
    classify_stderr,
    failure_layer_for_kind,
    failure_kind_required_for_row,
    normalize_failure_kind,
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

    def test_failure_classification_recognizes_final_acceptance_lifecycle(self):
        result = classify_events(
            [
                {
                    "event": "ultra_final_acceptance_failed",
                    "lifecycle_stage": "final_acceptance",
                    "repair_target": "missing_path",
                    "missing_paths": ["src/app/page.tsx"],
                }
            ]
        )
        self.assertEqual(result["failure_kind"], "final_acceptance_failure")
        self.assertEqual(result["lifecycle_stage"], "final_acceptance")
        self.assertEqual(result["repair_target"], "missing_path")
        self.assertEqual(result["missing_artifacts"], "src/app/page.tsx")

    def test_failure_classification_recognizes_final_acceptance_repair_exhausted(self):
        result = classify_events(
            [
                {
                    "event": "ultra_final_acceptance_failed",
                    "lifecycle_stage": "final_acceptance",
                    "repair_target": "missing_path",
                },
                {
                    "event": "final_acceptance_repair_exhausted",
                    "lifecycle_stage": "final_acceptance_repair",
                    "repair_target": "missing_path",
                    "missing_paths": ["src/app/page.tsx"],
                },
            ]
        )
        self.assertEqual(result["failure_kind"], "final_acceptance_repair_exhausted")
        self.assertEqual(result["lifecycle_stage"], "final_acceptance_repair")
        self.assertEqual(result["missing_artifacts"], "src/app/page.tsx")

    def test_stderr_classifies_final_acceptance_repair_failures(self):
        result = classify_stderr(
            "error: ultra final acceptance failed after bounded repair: src/app/page.tsx",
            rc=1,
        )
        self.assertEqual(result["failure_kind"], "final_acceptance_repair_exhausted")
        result = classify_stderr(
            "error: ultra final acceptance repair failed: fake client exhausted",
            rc=1,
        )
        self.assertEqual(result["failure_kind"], "final_acceptance_repair_failed")

    def test_stderr_classifies_step_verify_failure(self):
        result = classify_stderr(
            "error: step create-main-rs failed verification after bounded repair: command failed",
            rc=1,
        )
        self.assertEqual(result["failure_kind"], "step_verify_failure")

    def test_stderr_classifies_ultra_plan_generation_schema_failure(self):
        result = classify_stderr(
            "error: invalid generated UltraPlan after corrective retries: UltraPlan missing goal",
            rc=1,
        )
        self.assertEqual(result["failure_kind"], "planner_schema_error")

    def test_stderr_classifies_ultra_plan_generation_lint_failure(self):
        result = classify_stderr(
            "error: invalid generated UltraPlan after corrective retries: ultra phase prompt must be a plain natural-language goal, not a shell command",
            rc=1,
        )
        self.assertEqual(result["failure_kind"], "phase_scaffold_error")

    def test_stderr_classifies_verify_dependency_order(self):
        result = classify_stderr(
            "error: invalid StepPlan after corrective retries: verify command requires dependency setup or package manifest first",
            rc=1,
        )
        self.assertEqual(result["failure_kind"], "verify_dependency_order_error")
        self.assertEqual(result["planner_stage"], "dependency_order")

    def test_stderr_classifies_verify_setup_or_dev_server_policy(self):
        result = classify_stderr(
            "error: invalid StepPlan after corrective retries: verify command may not perform setup or start a dev server",
            rc=1,
        )
        self.assertEqual(result["failure_kind"], "verify_command_policy_error")
        self.assertEqual(result["planner_stage"], "verify_policy")

    def test_stderr_classifies_missing_expected_result_as_schema(self):
        result = classify_stderr(
            "error: invalid StepPlan after corrective retries: StepPlan missing expected_result in step 1",
            rc=1,
        )
        self.assertEqual(result["failure_kind"], "planner_schema_error")
        self.assertEqual(result["planner_stage"], "schema")

    def test_failure_layers_separate_provider_from_capability_failures(self):
        self.assertEqual(failure_layer_for_kind("provider_http_status"), "provider")
        self.assertEqual(capability_failure_included("provider_http_status"), False)
        self.assertEqual(failure_layer_for_kind("planner_schema_error"), "planning")
        self.assertEqual(capability_failure_included("planner_schema_error"), True)
        self.assertEqual(failure_layer_for_kind("tool_validation_error"), "runtime")
        self.assertEqual(capability_failure_included("tool_validation_error"), True)

    def test_dependency_setup_missing_loop_stop_can_classify_blocked_setup(self):
        result = classify_events(
            [
                {
                    "event": "loop_stop",
                    "reason": "dependency_setup_missing",
                    "dependency_setup_status": "blocked",
                    "verifier_bootstrap_state": "dependency_setup_blocked",
                }
            ]
        )
        self.assertEqual(result["failure_kind"], "dependency_setup_blocked")
        self.assertEqual(failure_layer_for_kind(result["failure_kind"]), "bridge")

    def test_dependency_build_lifecycle_classifies_plan_run_setup_blocked(self):
        result = classify_events(
            [
                {
                    "event": "dependency_build_lifecycle",
                    "mode": "plan-run",
                    "lifecycle_stage": "dependency_setup_build",
                    "lifecycle_stages": [
                        "dependency_check",
                        "setup_authority_missing",
                        "setup_blocked",
                        "verification_dependency_missing",
                    ],
                    "setup_status": "blocked",
                    "final_status": "dependency_missing",
                },
                {
                    "event": "step_verify_failure",
                    "dependency_missing": [
                        "dependency_setup_missing: Next.js build dependency setup missing"
                    ],
                    "repair_target": "dependency_setup",
                },
            ]
        )
        self.assertEqual(result["failure_kind"], "dependency_setup_blocked")
        self.assertEqual(result["dependency_setup_status"], "blocked")

    def test_tailwind_profile_failure_is_precise_contract_kind(self):
        result = classify_events(
            [
                {
                    "event": "step_verify_failure",
                    "profile_failures": [
                        "tailwind_contract_failure: Tailwind config file missing"
                    ],
                    "repair_target": "framework_config",
                }
            ]
        )
        self.assertEqual(result["failure_kind"], "tailwind_contract_failure")
        self.assertEqual(failure_layer_for_kind(result["failure_kind"]), "bridge")

    def test_later_step_verify_event_wins_over_older_planner_error(self):
        result = classify_events(
            [
                {
                    "event": "planner_error",
                    "planner_error_kind": "planner_lint_error",
                    "planner_stage": "lint",
                },
                {
                    "event": "step_verify_failure",
                    "repair_target": "implementation",
                },
            ]
        )
        self.assertEqual(result["failure_kind"], "step_verify_failure")
        self.assertEqual(failure_layer_for_kind(result["failure_kind"]), "bridge")

    def test_ultra_phase_execute_failure_is_runtime_not_stale_planning(self):
        result = classify_events(
            [
                {
                    "event": "planner_error",
                    "planner_error_kind": "planner_lint_error",
                    "planner_stage": "lint",
                },
                {
                    "event": "ultra_phase_failed",
                    "stage": "execute",
                    "phase_id": "implement",
                    "reason": "step verify failed verification after bounded repair: command failed",
                },
            ]
        )
        self.assertEqual(result["failure_kind"], "step_verify_failure")
        self.assertEqual(result["phase_failure_stage"], "execute")

    def test_build_failed_after_setup_is_distinct_from_plain_build_failure(self):
        result = classify_events(
            [
                {
                    "event": "loop_stop",
                    "reason": "build_verify_failed",
                    "build_verifier_lifecycle": [
                        {
                            "setup": {"status": "passed"},
                            "final_status": "failed",
                        }
                    ],
                }
            ]
        )
        self.assertEqual(result["failure_kind"], "build_after_setup_failed")

    def test_browser_http_500_is_distinct_runtime_acceptance_failure(self):
        result = classify_events(
            [
                {
                    "event": "browser_oracle_summary",
                    "browser_success": False,
                    "browser_failure_kind": "browser_http_500",
                    "browser_details": {"status": "failed", "http_status": 500},
                }
            ]
        )
        self.assertEqual(result["failure_kind"], "browser_http_500")
        self.assertEqual(result["browser_http_status"], 500)
        self.assertEqual(failure_layer_for_kind("browser_http_500"), "acceptance")

    def test_acceptance_summary_failure_is_classified(self):
        result = classify_events(
            [
                {
                    "event": "acceptance_summary",
                    "acceptance_success": False,
                    "acceptance_failure_kind": "static_title_only",
                    "acceptance_failure_reasons": ["source_semantic_failure"],
                }
            ]
        )
        self.assertEqual(result["failure_kind"], "static_title_only")
        self.assertEqual(failure_layer_for_kind(result["failure_kind"]), "acceptance")

    def test_summary_row_failure_kind_normalizes_from_acceptance_failure(self):
        row = {
            "run_id": "r1",
            "success": "true",
            "rc": "0",
            "acceptance_success": "false",
            "acceptance_failure_kind": "plan_output_missing_required_capabilities",
            "extras_json": "{}",
        }
        self.assertTrue(failure_kind_required_for_row(row))
        self.assertEqual(
            normalize_failure_kind(row),
            "plan_output_missing_required_capabilities",
        )
        self.assertEqual(blank_failure_kind_gate_violations([row]), [])

    def test_summary_row_blank_failure_kind_gate_detects_process_failure(self):
        row = {
            "run_id": "r1",
            "scenario": "s",
            "mode": "plan-run",
            "success": "false",
            "rc": "1",
            "process_success": "",
            "acceptance_success": "",
            "extras_json": "{}",
        }
        violations = blank_failure_kind_gate_violations([row])
        self.assertEqual(len(violations), 1)
        self.assertEqual(violations[0]["run_id"], "r1")

    def test_dry_run_and_diagnostic_skipped_do_not_require_failure_kind(self):
        rows = [
            {"success": "diagnostic_skipped", "rc": "", "extras_json": "{}"},
            {"success": "", "rc": "", "extras_json": {"dry_run": True}},
        ]
        self.assertEqual(blank_failure_kind_gate_violations(rows), [])

if __name__ == "__main__":
    unittest.main()
