import sys
import unittest
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(ROOT / "scripts"))

from eval_lib.plan_readiness import (
    aggregate_ultra_phase_readiness,
    classify_readiness_outcome,
    diagnose_verify_command,
    score_plan_readiness,
)


def sample_plan():
    return {
        "goal": "Create a small project and verify it deterministically.",
        "steps": [
            {
                "id": "setup",
                "kind": "setup",
                "expected_result": "pass",
                "instruction": "Create dependency manifest and scripts.",
                "expected_paths": ["package.json"],
                "verify": ["test -f package.json"],
            },
            {
                "id": "implement",
                "kind": "implement",
                "expected_result": "pass",
                "instruction": "Implement the requested app.",
                "expected_paths": ["src/app/page.tsx"],
                "verify": ["npm run build"],
            },
        ],
    }


class PlanReadinessTest(unittest.TestCase):
    def test_verify_policy_mirrors_rust_diagnosis_cases(self):
        cases = {
            "npm test && npm run build": "shell_control_syntax",
            "next dev -p 3011": "setup_or_dev_server",
            "cargo test --manifest-path ../Cargo.toml": "workspace_escape",
            "   ": "empty",
            "  cargo   test   --locked  ": "",
        }
        for command, violation in cases.items():
            with self.subTest(command=command):
                self.assertEqual(diagnose_verify_command(command)["violation"], violation)
        self.assertEqual(
            diagnose_verify_command("  cargo   test   --locked  ")["normalized"],
            "cargo test --locked",
        )

    def test_pre_run_readiness_does_not_use_post_run_outcome(self):
        plan = sample_plan()
        before = score_plan_readiness(plan, profile="web", prompt="Build the app")
        high_failure = classify_readiness_outcome(
            before["plan_run_readiness_score"],
            success=False,
            failure_kind="postcheck_failure",
        )
        low_success = classify_readiness_outcome(
            55.0,
            success=True,
            failure_kind="",
        )
        after = score_plan_readiness(plan, profile="web", prompt="Build the app")
        self.assertEqual(before["plan_run_readiness_score"], after["plan_run_readiness_score"])
        self.assertEqual(high_failure["missed_predictive_signal_reason"], "postcheck_contract_not_reflected_in_readiness")
        self.assertEqual(low_success["readiness_false_negative_kind"], "low_readiness_but_success")

    def test_same_step_plan_ignores_hidden_suite_oracle_changes(self):
        plan = sample_plan()
        first = score_plan_readiness(plan, profile="web", prompt="Build the app")
        second = score_plan_readiness(plan, profile="web", prompt="Build the app")
        self.assertEqual(first["plan_run_readiness_score"], second["plan_run_readiness_score"])
        self.assertEqual(first["readiness_source"], "eval_derived")

    def test_runner_handoff_integrity_uses_boolean_contract_event(self):
        plan = sample_plan()
        events = [
            {
                "event": "step_prompt_contract",
                "has_overall_goal": True,
                "has_required_final_artifacts": True,
                "has_expected_paths": True,
                "has_verify_commands": True,
                "has_expected_result": True,
                "has_bounded_repair_policy": True,
                "prior_artifact_context_applicable": True,
                "has_prior_artifact_context": True,
            }
        ]
        scored = score_plan_readiness(
            plan,
            profile="web",
            prompt="Build the app",
            handoff_events=events,
        )
        self.assertEqual(scored["runner_handoff_integrity_score"], 100.0)
        self.assertEqual(scored["readiness_source"], "eval_derived+runtime_event")

    def test_ultra_phase_aggregate_uses_weakest_phase(self):
        aggregate = aggregate_ultra_phase_readiness(
            [
                {"plan_path": "/tmp/plan-a.yaml", "plan_run_readiness_score": 91.0, "readiness_cap_reason": ""},
                {"plan_path": "/tmp/plan-b.yaml", "plan_run_readiness_score": 54.0, "readiness_cap_reason": "verify_before_manifest_owner"},
            ]
        )
        self.assertEqual(aggregate["ultra_phase_readiness_min_score"], 54.0)
        self.assertEqual(aggregate["ultra_phase_readiness_avg_score"], 72.5)
        self.assertEqual(aggregate["ultra_phase_readiness_failing_phase"], "plan-b.yaml")
        self.assertEqual(aggregate["ultra_phase_readiness_cap_reason"], "verify_before_manifest_owner")


if __name__ == "__main__":
    unittest.main()
