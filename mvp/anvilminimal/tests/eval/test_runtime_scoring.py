import sys
import tempfile
import unittest
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(ROOT / "scripts"))

from eval_lib.runtime_scoring import score_runtime_health


class RuntimeScoringTest(unittest.TestCase):
    def test_runtime_health_separates_stalled_inspection_from_artifact_progress(self):
        scenario = {"expected_artifacts": ["date-helper.js"]}
        stalled = [
            {"event": "provider_response", "tool_calls": 1},
            {"event": "tool_call_raw", "name": "Glob"},
            {"event": "tool_call_raw", "name": "Read"},
            {"event": "tool_call_raw", "name": "Read"},
            {"event": "tool_call_raw", "name": "Read"},
            {"event": "tool_call_raw", "name": "Read"},
            {"event": "tool_call_raw", "name": "Read"},
            {"event": "provider_response", "tool_calls": 0},
            {"event": "artifact_stagnation_feedback"},
            {"event": "loop_stop", "reason": "max_iterations"},
        ]
        successful = [
            {"event": "provider_response", "tool_calls": 1},
            {"event": "tool_call_raw", "name": "Write"},
            {"event": "tool_execute", "name": "Write", "status": "ok"},
            {"event": "loop_stop", "reason": "required_artifacts_satisfied_after_tool"},
        ]
        with tempfile.TemporaryDirectory() as td:
            workdir = Path(td)
            stalled_score = score_runtime_health(
                stalled,
                mode="plan-run",
                success=False,
                scenario=scenario,
                workdir=workdir,
            )
            (workdir / "date-helper.js").write_text("module.exports = {}", encoding="utf-8")
            success_score = score_runtime_health(
                successful,
                mode="plan-run",
                success=True,
                scenario=scenario,
                workdir=workdir,
            )
        self.assertLess(stalled_score["runtime_friction_score"], success_score["runtime_friction_score"])
        self.assertLess(stalled_score["artifact_progress_score"], success_score["artifact_progress_score"])
        self.assertLess(stalled_score["plan_run_runtime_health_score"], success_score["plan_run_runtime_health_score"])
        self.assertEqual(stalled_score["prompt_contract_score"], "")

    def test_runtime_health_is_blank_without_runtime_events(self):
        with tempfile.TemporaryDirectory() as td:
            score = score_runtime_health(
                [{"event": "plan_score", "score": 80}],
                mode="plan-run",
                success=True,
                scenario={"expected_artifacts": ["README.md"]},
                workdir=Path(td),
            )
        self.assertEqual(score["runtime_friction_score"], "")
        self.assertEqual(score["plan_run_runtime_health_score"], "")
        self.assertEqual(score["prompt_contract_score"], "")
        self.assertEqual(score["step_obligation_scope_score"], "")

    def test_prompt_contract_score_uses_boolean_event_without_prompt_body(self):
        events = [
            {
                "event": "provider_response",
                "tool_calls": 1,
            },
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
                "prompt_body_saved": False,
            },
            {"event": "loop_stop", "reason": "required_artifacts_satisfied_after_tool"},
        ]
        with tempfile.TemporaryDirectory() as td:
            score = score_runtime_health(
                events,
                mode="plan-run",
                success=True,
                scenario={"expected_artifacts": []},
                workdir=Path(td),
            )
        self.assertEqual(score["prompt_contract_score"], 100.0)

    def test_step_obligation_scope_score_detects_disabled_extraction(self):
        events = [
            {"event": "provider_response", "tool_calls": 1},
            {
                "event": "step_obligation_scope",
                "session_scope": "plan-run-step",
                "explicit_required_paths": ["src/app/page.tsx"],
                "effective_required_paths": ["src/app/page.tsx"],
                "prompt_extracted_paths_enabled": False,
                "prompt_extracted_paths": [],
                "completion_contract_path_merge_enabled": False,
                "completion_contract_verification_enabled": False,
                "completion_contract_paths": [],
            },
            {"event": "loop_stop", "reason": "required_artifacts_satisfied_after_tool"},
        ]
        with tempfile.TemporaryDirectory() as td:
            score = score_runtime_health(
                events,
                mode="plan-run",
                success=True,
                scenario={"expected_artifacts": []},
                workdir=Path(td),
            )
        self.assertEqual(score["step_obligation_scope_score"], 100.0)
        self.assertEqual(score["step_obligation_scope_violation_count"], 0)

    def test_step_obligation_scope_score_penalizes_context_artifact_merge(self):
        events = [
            {"event": "provider_response", "tool_calls": 1},
            {
                "event": "step_obligation_scope",
                "session_scope": "plan-run-step",
                "explicit_required_paths": [],
                "effective_required_paths": ["README.md"],
                "prompt_extracted_paths_enabled": True,
                "prompt_extracted_paths": ["README.md"],
                "completion_contract_path_merge_enabled": False,
                "completion_contract_verification_enabled": False,
                "completion_contract_paths": [],
            },
            {"event": "loop_stop", "reason": "required_artifacts_missing"},
        ]
        with tempfile.TemporaryDirectory() as td:
            score = score_runtime_health(
                events,
                mode="plan-run",
                success=False,
                scenario={"expected_artifacts": ["README.md"]},
                workdir=Path(td),
            )
        self.assertLess(score["step_obligation_scope_score"], 100.0)
        self.assertEqual(score["step_obligation_scope_violation_count"], 1)


if __name__ == "__main__":
    unittest.main()
