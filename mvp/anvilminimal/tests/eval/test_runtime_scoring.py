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
        self.assertEqual(score["phase_completion_score"], "")
        self.assertEqual(score["ultra_runtime_health_score"], "")
        self.assertEqual(score["execution_contract_adherence_score"], "")
        self.assertEqual(score["execution_contract_adherence_raw_score"], "")
        self.assertEqual(score["postcheck_stability_reason"], "")

    def test_final_acceptance_repair_events_are_runtime_events(self):
        events = [
            {"event": "ultra_phase_start", "phase_id": "finish", "total_phases": 1},
            {"event": "ultra_phase_scaffold_complete", "phase_id": "finish", "total_phases": 1},
            {"event": "ultra_phase_execute_complete", "phase_id": "finish", "total_phases": 1},
            {"event": "ultra_phase_profile_check", "phase_id": "finish", "total_phases": 1, "ok": True},
            {"event": "ultra_phase_complete", "phase_id": "finish", "total_phases": 1},
            {
                "event": "ultra_final_acceptance_failed",
                "lifecycle_stage": "final_acceptance",
                "repair_target": "missing_path",
            },
            {
                "event": "final_acceptance_repair_start",
                "lifecycle_stage": "final_acceptance_repair",
                "attempt": 1,
                "max_attempts": 1,
            },
            {
                "event": "final_acceptance_repair_exhausted",
                "lifecycle_stage": "final_acceptance_repair",
                "attempt": 1,
                "max_attempts": 1,
            },
        ]
        with tempfile.TemporaryDirectory() as td:
            score = score_runtime_health(
                events,
                mode="ultra-plan-run",
                success=False,
                scenario={"expected_artifacts": ["src/app/page.tsx"]},
                workdir=Path(td),
            )
        self.assertNotEqual(score["runtime_friction_score"], "")
        self.assertNotEqual(score["ultra_runtime_health_score"], "")

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

    def test_ultra_runtime_scores_phase_progress_and_failed_build_repair(self):
        events = [
            {"event": "ultra_phase_start", "phase_id": "scaffold", "total_phases": 2},
            {"event": "ultra_phase_scaffold_complete", "phase_id": "scaffold", "total_phases": 2},
            {"event": "ultra_phase_execute_complete", "phase_id": "scaffold", "total_phases": 2},
            {"event": "ultra_phase_profile_check", "phase_id": "scaffold", "total_phases": 2, "ok": True},
            {"event": "ultra_phase_complete", "phase_id": "scaffold", "total_phases": 2},
            {"event": "ultra_phase_start", "phase_id": "game", "total_phases": 2},
            {"event": "ultra_phase_scaffold_complete", "phase_id": "game", "total_phases": 2},
            {"event": "ultra_phase_execute_complete", "phase_id": "game", "total_phases": 2},
            {
                "event": "completion_verify",
                "ok": False,
                "command_failures": 1,
                "primary_reason": "command failed: npm run build src/app/page.tsx duplicate variable",
                "failure_signature": "command:npm run build:duplicate-variable",
            },
            {
                "event": "verify_repair_progress",
                "verdict": "unchanged",
                "had_edit": False,
            },
            {
                "event": "verify_repair_turn",
                "has_edit": False,
                "inspect_only": True,
            },
            {"event": "ultra_phase_failed", "phase_id": "game", "total_phases": 2, "stage": "execute"},
            {"event": "loop_stop", "reason": "verify_repair_progress_unchanged"},
        ]
        with tempfile.TemporaryDirectory() as td:
            score = score_runtime_health(
                events,
                mode="ultra-plan-run",
                success=False,
                scenario={"expected_artifacts": ["package.json", "src/app/page.tsx"]},
                workdir=Path(td),
            )
        self.assertEqual(score["phase_completion_score"], 82.5)
        self.assertEqual(score["phase_plan_validity_score"], 100.0)
        self.assertEqual(score["phase_scaffold_success_score"], 100.0)
        self.assertEqual(score["phase_step_execution_score"], 100.0)
        self.assertEqual(score["phase_verify_success_score"], 0.0)
        self.assertEqual(score["phase_finalization_score"], 50.0)
        self.assertEqual(score["phase_failure_stage"], "execute")
        self.assertEqual(score["build_verify_pass_score"], 0.0)
        self.assertEqual(score["compile_diagnostic_progress_score"], 35.0)
        self.assertEqual(score["verify_repair_edit_score"], 0.0)
        self.assertEqual(score["build_repair_effectiveness_score"], 21.0)
        self.assertLess(score["ultra_runtime_health_score"], 50.0)

    def test_ultra_phase_plan_validity_uses_explicit_validation_event_when_present(self):
        events = [
            {"event": "ultra_phase_start", "phase_id": "one", "total_phases": 2},
            {"event": "ultra_phase_scaffold_complete", "phase_id": "one", "total_phases": 2},
            {"event": "ultra_phase_plan_validated", "phase_id": "one", "total_phases": 2},
            {"event": "ultra_phase_start", "phase_id": "two", "total_phases": 2},
            {"event": "ultra_phase_scaffold_complete", "phase_id": "two", "total_phases": 2},
            {"event": "ultra_phase_failed", "phase_id": "two", "total_phases": 2, "stage": "lint"},
        ]
        with tempfile.TemporaryDirectory() as td:
            score = score_runtime_health(
                events,
                mode="ultra-plan-run",
                success=False,
                scenario={"expected_artifacts": []},
                workdir=Path(td),
            )
        self.assertEqual(score["phase_plan_validity_score"], 50.0)
        self.assertEqual(score["phase_scaffold_success_score"], 100.0)
        self.assertEqual(score["phase_failure_stage"], "lint")

    def test_ultra_context_continuity_scores_shared_session_events(self):
        events = [
            {
                "event": "ultra_context_initialized",
                "total_phases": 2,
                "shared_execution_session": True,
                "session_message_count": 0,
                "pending_final_artifacts_count": 2,
            },
            {
                "event": "ultra_phase_context_attached",
                "phase_id": "scaffold",
                "phase_index": 1,
                "total_phases": 2,
                "shared_execution_session": True,
                "session_message_count": 0,
                "has_previous_context": False,
                "changed_path_count": 0,
                "unresolved_repair_target_count": 0,
            },
            {
                "event": "ultra_phase_context_updated",
                "phase_id": "scaffold",
                "phase_index": 1,
                "total_phases": 2,
                "shared_execution_session": True,
                "session_message_count": 4,
                "changed_path_count": 2,
                "recent_verify_failure_count": 0,
                "unresolved_repair_target_count": 0,
                "partial_outcome_recorded": False,
            },
            {
                "event": "ultra_phase_context_attached",
                "phase_id": "finish",
                "phase_index": 2,
                "total_phases": 2,
                "shared_execution_session": True,
                "session_message_count": 4,
                "has_previous_context": True,
                "changed_path_count": 2,
                "unresolved_repair_target_count": 0,
            },
            {
                "event": "ultra_phase_context_updated",
                "phase_id": "finish",
                "phase_index": 2,
                "total_phases": 2,
                "shared_execution_session": True,
                "session_message_count": 8,
                "changed_path_count": 3,
                "recent_verify_failure_count": 1,
                "unresolved_repair_target_count": 1,
                "partial_outcome_recorded": True,
            },
        ]
        with tempfile.TemporaryDirectory() as td:
            score = score_runtime_health(
                events,
                mode="ultra-plan-run",
                success=False,
                scenario={"expected_artifacts": []},
                workdir=Path(td),
            )
        self.assertEqual(score["ultra_context_continuity_score"], 100.0)
        self.assertEqual(score["ultra_shared_session_observed"], 100.0)
        self.assertEqual(score["ultra_context_attached_after_first_phase"], 100.0)
        self.assertEqual(score["ultra_context_bounded"], 100.0)
        self.assertEqual(score["ultra_session_message_growth_observed"], 100.0)
        self.assertEqual(score["ultra_partial_outcome_recorded"], 100.0)

    def test_ultra_runtime_scores_successful_build_repair(self):
        events = [
            {"event": "ultra_phase_start", "phase_id": "final", "total_phases": 1},
            {"event": "ultra_phase_scaffold_complete", "phase_id": "final", "total_phases": 1},
            {"event": "ultra_phase_execute_complete", "phase_id": "final", "total_phases": 1},
            {"event": "ultra_phase_profile_check", "phase_id": "final", "total_phases": 1, "ok": True},
            {"event": "ultra_phase_complete", "phase_id": "final", "total_phases": 1},
            {
                "event": "completion_verify",
                "ok": False,
                "command_failures": 1,
                "primary_reason": "command failed: npm run build",
                "failure_signature": "command:npm run build:syntax",
            },
            {"event": "verify_repair_progress", "verdict": "improved", "had_edit": True},
            {
                "event": "completion_verify",
                "ok": True,
                "command_failures": 0,
                "primary_reason": "ok",
                "deferred_verify_requirements": [{"command": "npm run build", "status": "passed"}],
            },
            {"event": "ultra_plan_complete", "total_phases": 1, "ok": True},
        ]
        with tempfile.TemporaryDirectory() as td:
            score = score_runtime_health(
                events,
                mode="ultra-plan-run",
                success=True,
                scenario={"expected_artifacts": []},
                workdir=Path(td),
            )
        self.assertEqual(score["phase_completion_score"], 100.0)
        self.assertEqual(score["build_verify_pass_score"], 100.0)
        self.assertEqual(score["build_repair_effectiveness_score"], 100.0)
        self.assertEqual(score["compile_diagnostic_progress_score"], 100.0)
        self.assertEqual(score["verify_repair_edit_score"], 100.0)
        self.assertEqual(score["ultra_runtime_health_score"], 100.0)

    def test_execution_contract_adherence_penalizes_plan_artifact_drift(self):
        plan = """goal: nextjs app
steps:
  - id: setup
    kind: setup
    instruction: Create package.json with compatible aligned Next.js, React, React DOM, @types/react, @types/react-dom, and TypeScript 5.x dependencies. Use matching React runtime and type package major versions. Create tsconfig.json with moduleResolution=bundler and target=ES2017 or newer.
    expected_paths:
      - package.json
      - tsconfig.json
  - id: verify-build
    kind: verify
    instruction: Verify build.
    verify:
      - npm run build
"""
        events = [
            {"event": "provider_response", "tool_calls": 1},
            {"event": "tool_call_raw", "name": "Write"},
            {"event": "tool_execute", "name": "Write", "status": "ok"},
            {"event": "loop_stop", "reason": "required_artifacts_satisfied_after_tool"},
        ]
        with tempfile.TemporaryDirectory() as td:
            root = Path(td)
            plan_path = root / "plan.yaml"
            plan_path.write_text(plan, encoding="utf-8")
            workdir = root / "workdir"
            workdir.mkdir()
            (workdir / "package.json").write_text(
                """{
  "scripts": {"build": "next build"},
  "dependencies": {"next": "14.2.14", "react": "18.3.1", "react-dom": "18.3.1"},
  "devDependencies": {"typescript": "6.0.3", "@types/react": "19.2.17", "@types/react-dom": "18.3.1"}
}
""",
                encoding="utf-8",
            )
            (workdir / "tsconfig.json").write_text(
                """{"compilerOptions":{"moduleResolution":"bundler","target":"ES2017"}}""",
                encoding="utf-8",
            )
            postcheck = root / "postcheck"
            postcheck.mkdir()
            (postcheck / "events.jsonl").write_text(
                '{"event":"postcheck","command":"npm run build","rc":0}\\n',
                encoding="utf-8",
            )
            (postcheck / "command-0.stderr.log").write_text(
                "Installing devDependencies (yarn):\\n- typescript\\nwarning package-lock.json found\\n",
                encoding="utf-8",
            )
            score = score_runtime_health(
                events,
                mode="plan-run",
                success=True,
                scenario={
                    "profile": "nextjs",
                    "expected_artifacts": ["package.json"],
                    "postcheck": {"commands": ["npm run build"]},
                },
                workdir=workdir,
                plan_paths=[plan_path],
                run_dir=root,
            )
        self.assertLess(score["dependency_contract_score"], 80.0)
        self.assertLess(score["postcheck_stability_score"], 80.0)
        self.assertLess(score["execution_contract_adherence_score"], 90.0)

    def test_dependency_contract_does_not_penalize_unpinned_future_major_without_plan_contract(self):
        plan = """goal: nextjs app
steps:
  - id: setup
    kind: setup
    instruction: Create package.json with compatible aligned Next.js, React, React DOM, @types/react, @types/react-dom, and TypeScript dependencies. Use matching React runtime and type package major versions.
    expected_paths:
      - package.json
  - id: verify-build
    kind: verify
    instruction: Verify build.
    verify:
      - npm run build
"""
        events = [
            {"event": "provider_response", "tool_calls": 1},
            {"event": "tool_call_raw", "name": "Write"},
            {"event": "tool_execute", "name": "Write", "status": "ok"},
            {"event": "loop_stop", "reason": "required_artifacts_satisfied_after_tool"},
        ]
        with tempfile.TemporaryDirectory() as td:
            root = Path(td)
            plan_path = root / "plan.yaml"
            plan_path.write_text(plan, encoding="utf-8")
            workdir = root / "workdir"
            workdir.mkdir()
            (workdir / "package.json").write_text(
                """{
  "scripts": {"build": "next build"},
  "dependencies": {"next": "15.0.0", "react": "19.0.0", "react-dom": "19.0.0"},
  "devDependencies": {"typescript": "6.0.3", "@types/react": "19.2.17", "@types/react-dom": "19.2.17"}
}
""",
                encoding="utf-8",
            )
            score = score_runtime_health(
                events,
                mode="plan-run",
                success=True,
                scenario={
                    "profile": "nextjs",
                    "expected_artifacts": ["package.json"],
                    "postcheck": {"commands": ["npm run build"]},
                },
                workdir=workdir,
                plan_paths=[plan_path],
                run_dir=root,
            )
        self.assertEqual(score["dependency_contract_score"], 100.0)

    def test_execution_contract_adherence_rewards_stable_contract_match(self):
        plan = """goal: nextjs app
steps:
  - id: setup
    kind: setup
    instruction: Create package.json with compatible aligned Next.js, React, React DOM, @types/react, @types/react-dom, and TypeScript 5.x dependencies. Use matching React runtime and type package major versions. Create tsconfig.json with moduleResolution=bundler and target=ES2017 or newer.
    expected_paths:
      - package.json
      - tsconfig.json
  - id: verify-build
    kind: verify
    instruction: Verify build.
    verify:
      - npm run build
"""
        events = [
            {"event": "provider_response", "tool_calls": 1},
            {"event": "tool_call_raw", "name": "Write"},
            {"event": "tool_execute", "name": "Write", "status": "ok"},
            {"event": "loop_stop", "reason": "required_artifacts_satisfied_after_tool"},
        ]
        with tempfile.TemporaryDirectory() as td:
            root = Path(td)
            plan_path = root / "plan.yaml"
            plan_path.write_text(plan, encoding="utf-8")
            workdir = root / "workdir"
            workdir.mkdir()
            (workdir / "package.json").write_text(
                """{
  "scripts": {"build": "next build"},
  "dependencies": {"next": "14.2.14", "react": "18.3.1", "react-dom": "18.3.1"},
  "devDependencies": {"typescript": "5.5.4", "@types/react": "18.3.1", "@types/react-dom": "18.3.1"}
}
""",
                encoding="utf-8",
            )
            (workdir / "tsconfig.json").write_text(
                """{"compilerOptions":{"moduleResolution":"bundler","target":"ES2017"}}""",
                encoding="utf-8",
            )
            postcheck = root / "postcheck"
            postcheck.mkdir()
            (postcheck / "events.jsonl").write_text(
                '{"event":"postcheck","command":"npm run build","rc":0}\\n',
                encoding="utf-8",
            )
            (postcheck / "command-0.stderr.log").write_text("", encoding="utf-8")
            score = score_runtime_health(
                events,
                mode="plan-run",
                success=True,
                scenario={
                    "profile": "nextjs",
                    "expected_artifacts": ["package.json"],
                    "postcheck": {"commands": ["npm run build"]},
                },
                workdir=workdir,
                plan_paths=[plan_path],
                run_dir=root,
            )
        self.assertEqual(score["dependency_contract_score"], 100.0)
        self.assertEqual(score["config_contract_score"], 100.0)
        self.assertEqual(score["verify_contract_score"], 100.0)
        self.assertEqual(score["postcheck_stability_score"], 100.0)
        self.assertEqual(score["postcheck_stability_reason"], "")
        self.assertEqual(score["execution_contract_adherence_raw_score"], 100.0)
        self.assertEqual(score["execution_contract_min_subscore"], 100.0)
        self.assertEqual(score["execution_contract_cap_reason"], "")
        self.assertEqual(score["execution_contract_adherence_score"], 100.0)

    def test_execution_contract_adherence_caps_low_postcheck_subscore(self):
        plan = """goal: nextjs app
steps:
  - id: setup
    kind: setup
    instruction: Create package.json, tsconfig.json, and verify with npm run build.
    expected_paths:
      - package.json
      - tsconfig.json
    verify:
      - npm run build
"""
        events = [
            {"event": "provider_response", "tool_calls": 1},
            {"event": "tool_call_raw", "name": "Write"},
            {"event": "tool_execute", "name": "Write", "status": "ok"},
            {"event": "loop_stop", "reason": "required_artifacts_satisfied_after_tool"},
        ]
        with tempfile.TemporaryDirectory() as td:
            root = Path(td)
            plan_path = root / "plan.yaml"
            plan_path.write_text(plan, encoding="utf-8")
            workdir = root / "workdir"
            workdir.mkdir()
            (workdir / "package.json").write_text(
                """{
  "scripts": {"build": "next build"},
  "dependencies": {"next": "14.2.14", "react": "18.3.1", "react-dom": "18.3.1"},
  "devDependencies": {"typescript": "5.5.4", "@types/react": "18.3.1", "@types/react-dom": "18.3.1"}
}
""",
                encoding="utf-8",
            )
            (workdir / "tsconfig.json").write_text(
                """{"compilerOptions":{"moduleResolution":"bundler","target":"ES2017"}}""",
                encoding="utf-8",
            )
            postcheck = root / "postcheck"
            postcheck.mkdir()
            (postcheck / "events.jsonl").write_text(
                '{"event":"postcheck","command":"npm run build","rc":1}\n',
                encoding="utf-8",
            )
            (postcheck / "command-0.stderr.log").write_text(
                "Failed to compile. Type error in src/app/page.tsx",
                encoding="utf-8",
            )
            score = score_runtime_health(
                events,
                mode="plan-run",
                success=False,
                scenario={
                    "profile": "nextjs",
                    "expected_artifacts": ["package.json", "tsconfig.json"],
                    "postcheck": {"commands": ["npm run build"]},
                },
                workdir=workdir,
                plan_paths=[plan_path],
                run_dir=root,
            )
        self.assertEqual(score["postcheck_stability_reason"], "build_or_test_command_failed;compile_or_type_failure")
        self.assertGreater(score["execution_contract_adherence_raw_score"], 70.0)
        self.assertLessEqual(score["execution_contract_adherence_score"], 55.0)
        self.assertIn("postcheck_stability_below_60", score["execution_contract_cap_reason"])

    def test_build_lifecycle_scores_dependency_boundary_and_repair_target(self):
        events = [
            {
                "event": "completion_verify",
                "ok": False,
                "build_verifier_required": True,
                "build_verifier_attempted": False,
                "dependency_setup_status": "missing",
                "repair_target": "dependency_setup",
                "build_verifier_observations": [
                    {
                        "command": "npm run build",
                        "status": "dependency_missing",
                        "required_for_completion": True,
                        "requires_dependency_setup": True,
                        "attempted": False,
                    }
                ],
            },
            {
                "event": "loop_stop",
                "reason": "dependency_setup_missing",
                "repair_target": "dependency_setup",
            },
        ]
        with tempfile.TemporaryDirectory() as td:
            score = score_runtime_health(
                events,
                mode="plan-run",
                success=False,
                scenario={"expected_artifacts": ["package.json"]},
                workdir=Path(td),
            )
        self.assertEqual(score["build_verifier_completion_score"], 30.0)
        self.assertEqual(score["dependency_setup_boundary_score"], 85.0)
        self.assertEqual(score["dependency_setup_bridge_score"], 55.0)
        self.assertEqual(score["build_verifier_lifecycle_score"], 35.0)
        self.assertEqual(score["repair_target_resolution_score"], 70.0)
        self.assertEqual(score["repair_stagnation_score"], 70.0)
        self.assertEqual(score["profile_static_vs_build_gap_score"], 70.0)
        self.assertLess(score["plan_run_success_predictor"], 80.0)

    def test_build_lifecycle_scores_setup_passed_then_build_failed(self):
        events = [
            {
                "event": "completion_verify",
                "ok": False,
                "build_verifier_required": True,
                "dependency_setup_status": "ready",
                "build_verifier_lifecycle": [
                    {
                        "requirement": {
                            "command": "npm run build",
                            "required_for_completion": True,
                        },
                        "setup": {"status": "passed"},
                        "final_status": "failed",
                    }
                ],
                "build_verifier_observations": [
                    {
                        "command": "npm run build",
                        "status": "failed",
                        "required_for_completion": True,
                    }
                ],
            },
            {"event": "loop_stop", "reason": "build_verify_failed"},
        ]
        with tempfile.TemporaryDirectory() as td:
            score = score_runtime_health(
                events,
                mode="plan-run",
                success=False,
                scenario={"expected_artifacts": []},
                workdir=Path(td),
            )
        self.assertEqual(score["dependency_setup_bridge_score"], 100.0)
        self.assertEqual(score["build_verifier_lifecycle_score"], 55.0)

    def test_step_runtime_bridge_scores_repair_followthrough(self):
        events = [
            {
                "event": "step_verify_failure",
                "dependency_missing": ["node_modules/.bin/next missing"],
                "dependency_setup_authority": "plan_setup_step",
            },
            {
                "event": "step_verify_repair",
                "ok": False,
                "repair_target_followed": False,
                "dependency_setup_authority": "plan_setup_step",
            },
            {"event": "loop_stop", "reason": "step_verify_failure"},
        ]
        with tempfile.TemporaryDirectory() as td:
            score = score_runtime_health(
                events,
                mode="plan-run",
                success=False,
                scenario={"expected_artifacts": []},
                workdir=Path(td),
            )
        self.assertLess(score["step_runtime_bridge_score"], 80.0)
        self.assertEqual(score["repair_target_followthrough_score"], 0.0)

if __name__ == "__main__":
    unittest.main()
