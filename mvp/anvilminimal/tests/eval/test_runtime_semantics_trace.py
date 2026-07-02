import json
import tempfile
import unittest
from pathlib import Path
import sys

ROOT = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(ROOT / "scripts"))

from eval_lib.runtime_trace import compare_trace_reports, normalize_events, write_trace_artifacts
from eval_lib.run_summary import SUMMARY_HEADER, write_summary


class RuntimeSemanticsTraceTest(unittest.TestCase):
    def test_normalizes_event_sequence_to_lifecycle_stages_and_gates(self):
        events = [
            {"event": "tool_call_raw", "run_id": "r1", "argument_shape": {"path": "string"}},
            {"event": "ultra_phase_context_attached", "run_id": "r1", "phase_id": "implement"},
            {"event": "dependency_build_lifecycle", "run_id": "r1", "setup_attempted": True, "lifecycle_stages": ["dependency_check", "setup_passed", "build_rerun", "verification_passed"]},
            {"event": "acceptance_summary", "run_id": "r1", "acceptance_success": False, "acceptance_failure_kind": "missing_required_capabilities"},
            {"event": "recovery_prompt_saved", "run_id": "r1"},
        ]
        normalized = normalize_events(events, subject="mvp-anvilminimal")
        stages = [event["stage"] for event in normalized]
        self.assertEqual(
            stages,
            [
                "tool_requested",
                "phase_context_attached",
                "dependency_setup_attempted",
                "acceptance_failed",
                "recovery_handoff_saved",
            ],
        )
        gates = {gate for event in normalized for gate in event["gate_ids"]}
        self.assertTrue({"G-S05", "G-S07", "G-S09", "G-S12", "G-S13"}.issubset(gates))
        self.assertEqual(normalized[3]["failure_kind"], "missing_required_capabilities")

    def test_failed_run_without_events_is_gate_failure(self):
        with tempfile.TemporaryDirectory() as td:
            run_root = Path(td)
            run_id = "silent"
            run_dir = run_root / "runs" / run_id
            run_dir.mkdir(parents=True)
            row = {key: "" for key in SUMMARY_HEADER}
            row.update(
                {
                    "run_id": run_id,
                    "suite": "s",
                    "scenario": "scenario",
                    "mode": "ultra-plan-run",
                    "success": "false",
                    "rc": "1",
                    "failure_kind": "process_failure",
                }
            )
            write_summary(run_root / "summary.eval.tsv", [row])
            report = write_trace_artifacts(
                run_root,
                subject="mvp-anvilminimal",
                binary_kind="anvilminimal",
                binary_path="anvilminimal",
            )
            events = [
                json.loads(line)
                for line in Path(report["normalized_event_sequence_path"]).read_text(encoding="utf-8").splitlines()
                if line.strip()
            ]
        silent = [event for event in events if event.get("failure_kind") == "silent_exit_without_events"]
        self.assertEqual(len(silent), 1)
        self.assertEqual(silent[0]["stage"], "diagnostic_emitted")
        self.assertIn("G-S16", silent[0]["gate_ids"])

    def test_run_start_without_eval_override_is_manual_trace_evidence(self):
        normalized = normalize_events(
            [
                {
                    "event": "run_start",
                    "run_id": "manual",
                    "eval_events_override": False,
                    "action": "UltraPlanRun",
                }
            ],
            subject="mvp-anvilminimal",
        )
        stages = [event["stage"] for event in normalized]
        self.assertIn("request_understood", stages)
        self.assertIn("manual_tui_trace_recorded", stages)
        manual = next(event for event in normalized if event["stage"] == "manual_tui_trace_recorded")
        self.assertEqual(manual["gate_ids"], ["G-S16"])

    def test_source_and_mvp_diagnostics_trace_can_pass_gs14(self):
        rows = [
            {
                "suite": "s",
                "scenario": "diagnostic",
                "mode": "ultra-plan-run",
                "provider_model_pair": "openai:gpt planner=gemini:flash",
            }
        ]
        source_events = normalize_events(
            [
                {
                    "event": "agent.safe_stop.report",
                    "run_id": "diagnostic",
                    "stop_reason": "repair_exhausted",
                    "failure_type": "repair_exhausted",
                    "blocker_class": "repair_convergence",
                    "authority_status": "repair_authority_or_progress_not_established",
                    "next_user_action": "inspect diagnostics and retry",
                }
            ],
            subject="source-anvildev",
        )
        mvp_events = normalize_events(
            [
                {
                    "event": "loop_stop",
                    "run_id": "diagnostic",
                    "reason": "repair_exhausted",
                    "primary_reason": "repair_exhausted",
                    "failure_kind": "repair_exhausted",
                    "task_status": "failed",
                    "session_status": "repl_ready",
                    "recovery_next_action": "repair_final_acceptance_failure",
                },
                {
                    "event": "run_stop",
                    "run_id": "diagnostic",
                    "ok": False,
                    "stop_reason": "repair_exhausted",
                    "failure_kind": "repair_exhausted",
                    "task_status": "failed",
                    "session_status": "process_exited",
                    "recovery_next_action": "repair_final_acceptance_failure",
                }
            ],
            subject="mvp-anvilminimal",
        )
        source = self._trace_report("source", rows, source_events)
        mvp = self._trace_report("mvp", rows, mvp_events)
        diff = compare_trace_reports(source, mvp)
        by_gate = {item["gate_id"]: item for item in diff["gate_results"]}
        self.assertEqual(by_gate["G-S14"]["status"], "pass")
        self.assertEqual(by_gate["G-S14"]["reason"], "source_and_mvp_gate_observed")
        self.assertEqual(source_events[0]["failure_kind"], "repair_exhausted")
        diagnostic = next(event for event in mvp_events if event["stage"] == "diagnostic_emitted")
        self.assertEqual(diagnostic["task_status"], "failed")
        self.assertEqual(diagnostic["session_status"], "process_exited")

    def _trace_report(self, trace_id, rows, events):
        stage_counts = {}
        gate_counts = {}
        for event in events:
            stage_counts[event["stage"]] = stage_counts.get(event["stage"], 0) + 1
            for gate_id in event.get("gate_ids", []):
                gate_counts[gate_id] = gate_counts.get(gate_id, 0) + 1
        return {
            "trace_id": trace_id,
            "normalized_event_count": len(events),
            "manifest_rows": rows,
            "stage_counts": stage_counts,
            "gate_counts": gate_counts,
            "normalized_events": events,
        }

    def test_trace_manifest_redacts_prompt_and_secrets_from_command(self):
        with tempfile.TemporaryDirectory() as td:
            run_root = Path(td)
            run_id = "redact"
            run_dir = run_root / "runs" / run_id
            run_dir.mkdir(parents=True)
            command = [
                "anvilminimal",
                "--prompt",
                "build a secret app with sk-testsecret123456",
                "--model",
                "gpt-5.4-mini",
            ]
            (run_dir / "command.txt").write_text(
                " ".join(json.dumps(part) for part in command) + "\n",
                encoding="utf-8",
            )
            (run_dir / "anvil-events.jsonl").write_text(
                json.dumps({"event": "acceptance_summary", "run_id": run_id, "acceptance_success": True}) + "\n",
                encoding="utf-8",
            )
            row = {key: "" for key in SUMMARY_HEADER}
            row.update(
                {
                    "run_id": run_id,
                    "suite": "s",
                    "scenario": "scenario",
                    "mode": "minimal-loop",
                    "success": "true",
                    "rc": "0",
                }
            )
            write_summary(run_root / "summary.eval.tsv", [row])
            report = write_trace_artifacts(
                run_root,
                subject="mvp-anvilminimal",
                binary_kind="anvilminimal",
                binary_path="anvilminimal",
            )
            manifest = Path(report["manifest_path"]).read_text(encoding="utf-8")
        self.assertIn("<redacted-task-prompt>", manifest)
        self.assertNotIn("secret app", manifest)
        self.assertNotIn("sk-testsecret123456", manifest)

    def test_compare_reports_returns_stage_and_gate_diff(self):
        rows = [
            {
                "suite": "s",
                "scenario": "case",
                "mode": "minimal-loop",
                "provider_model_pair": "openai:gpt planner=gemini:flash",
            }
        ]
        source = {
            "trace_id": "source",
            "manifest_rows": rows,
            "stage_counts": {"plan_generated": 1, "verify_started": 1},
            "gate_counts": {"G-S03": 1, "G-S08": 1},
        }
        mvp = {
            "trace_id": "mvp",
            "manifest_rows": rows,
            "stage_counts": {"plan_generated": 1},
            "gate_counts": {"G-S03": 1},
        }
        diff = compare_trace_reports(source, mvp)
        self.assertEqual(diff["missing_stages_in_mvp"], ["verify_started"])
        self.assertEqual(diff["missing_gate_ids_in_mvp"], ["G-S08"])
        self.assertEqual(diff["partial_gate_ids"], [])
        by_gate = {item["gate_id"]: item for item in diff["gate_results"]}
        self.assertEqual(by_gate["G-S03"]["status"], "pass")
        self.assertEqual(by_gate["G-S08"]["reason"], "missing_gate_in_mvp_trace")

    def test_compare_reports_resolves_all_gates_without_partial(self):
        rows = [
            {
                "suite": "s",
                "scenario": "case",
                "mode": "minimal-loop",
                "provider_model_pair": "openai:gpt planner=gemini:flash",
            }
        ]
        source = {
            "trace_id": "source",
            "normalized_event_count": 2,
            "manifest_rows": rows,
            "stage_counts": {"plan_generated": 1, "verify_started": 1},
            "gate_counts": {"G-S03": 1, "G-S08": 1},
        }
        mvp = {
            "trace_id": "mvp",
            "normalized_event_count": 1,
            "manifest_rows": rows,
            "stage_counts": {"plan_generated": 1},
            "gate_counts": {"G-S03": 1},
        }
        diff = compare_trace_reports(source, mvp)
        self.assertEqual(diff["status"], "compared")
        self.assertEqual(diff["passed_gate_ids"], ["G-S03"])
        self.assertIn("G-S08", diff["failed_gate_ids"])
        self.assertEqual(diff["partial_gate_ids"], [])
        statuses = {item["status"] for item in diff["gate_results"]}
        self.assertLessEqual(statuses, {"pass", "fail", "intentionally_different"})

    def test_compare_reports_fails_same_condition_mismatch(self):
        source = {
            "trace_id": "source",
            "normalized_event_count": 1,
            "manifest_rows": [
                {
                    "suite": "s",
                    "scenario": "case-a",
                    "mode": "minimal-loop",
                    "provider_model_pair": "openai:gpt planner=gemini:flash",
                }
            ],
            "stage_counts": {"plan_generated": 1},
            "gate_counts": {"G-S03": 1},
        }
        mvp = {
            "trace_id": "mvp",
            "normalized_event_count": 1,
            "manifest_rows": [
                {
                    "suite": "s",
                    "scenario": "case-b",
                    "mode": "minimal-loop",
                    "provider_model_pair": "openai:gpt planner=gemini:flash",
                }
            ],
            "stage_counts": {"plan_generated": 1},
            "gate_counts": {"G-S03": 1},
        }
        diff = compare_trace_reports(source, mvp)
        self.assertEqual(diff["status"], "same_condition_mismatch")
        self.assertTrue(all(item["status"] == "fail" for item in diff["gate_results"]))

    def test_compare_reports_fails_when_same_condition_signature_missing(self):
        source = {
            "trace_id": "source",
            "normalized_event_count": 1,
            "stage_counts": {"plan_generated": 1},
            "gate_counts": {"G-S03": 1},
        }
        mvp = {
            "trace_id": "mvp",
            "normalized_event_count": 1,
            "stage_counts": {"plan_generated": 1},
            "gate_counts": {"G-S03": 1},
        }
        diff = compare_trace_reports(source, mvp)
        self.assertEqual(diff["status"], "same_condition_unknown")
        self.assertTrue(all(item["status"] == "fail" for item in diff["gate_results"]))

    def test_source_anvildev_llm_prompts_produce_phase_and_step_trace(self):
        with tempfile.TemporaryDirectory() as td:
            run_root = Path(td)
            run_id = "source-prompt"
            log_dir = (
                run_root
                / "runs"
                / run_id
                / "workdir"
                / ".anvil"
                / "state"
                / "sessions"
                / "session-1"
                / "logs"
            )
            log_dir.mkdir(parents=True)
            prompt_log = log_dir / "llm-io.jsonl"
            phase_prompt = """Create a step plan for this task:
Ultra goal:
Create README.md.

Current phase id:
draft-readme

Current phase goal:
Create README.md.

Existing workspace snapshot:
- none detected

Profile contract:
- Keep changes scoped.
"""
            step_prompt = """Overall goal:
Create README.md.

Current step id:
write-readme

Expected paths after this step:
- README.md

Verification commands for this step:
- cat README.md

Expected verification result:
pass
"""
            prompt_log.write_text(
                "\n".join(
                    [
                        json.dumps(
                            {
                                "event": "openai.responses.request",
                                "payload": {
                                    "messages": [
                                        {"role": "user", "content": phase_prompt},
                                        {"role": "user", "content": step_prompt},
                                    ]
                                },
                            }
                        )
                    ]
                )
                + "\n",
                encoding="utf-8",
            )
            row = {key: "" for key in SUMMARY_HEADER}
            row.update(
                {
                    "run_id": run_id,
                    "suite": "s",
                    "scenario": "scenario",
                    "mode": "ultra-plan-run",
                    "success": "true",
                    "rc": "0",
                    "main_provider": "openai",
                    "main_model": "gpt",
                    "planner_provider": "gemini",
                    "planner_model": "flash",
                }
            )
            write_summary(run_root / "summary.eval.tsv", [row])
            report = write_trace_artifacts(
                run_root,
                subject="source-anvildev",
                binary_kind="anvildev",
                binary_path="anvildev",
            )
            events = [
                json.loads(line)
                for line in Path(report["normalized_event_sequence_path"])
                .read_text(encoding="utf-8")
                .splitlines()
                if line.strip()
            ]
        stages = [event["stage"] for event in events]
        self.assertIn("phase_context_attached", stages)
        self.assertIn("step_prompt_built", stages)
        self.assertEqual(report["gate_counts"]["G-S05"], 1)
        self.assertEqual(report["gate_counts"]["G-S06"], 1)
        step = next(event for event in events if event["stage"] == "step_prompt_built")
        self.assertTrue(step["has_expected_result"])
        self.assertTrue(step["has_verify_commands"])

    def test_compare_reports_detects_missing_expected_result_and_verify(self):
        rows = [
            {
                "suite": "s",
                "scenario": "case",
                "mode": "plan-run",
                "provider_model_pair": "openai:gpt planner=gemini:flash",
            }
        ]
        source = {
            "trace_id": "source",
            "normalized_event_count": 1,
            "manifest_rows": rows,
            "stage_counts": {"step_prompt_built": 1},
            "gate_counts": {"G-S06": 1},
            "normalized_events": [
                {
                    "stage": "step_prompt_built",
                    "gate_ids": ["G-S06"],
                    "source_event": "source_step_prompt_observed",
                    "has_overall_goal": True,
                    "has_expected_paths": True,
                    "has_verify_commands": True,
                    "has_expected_result": True,
                }
            ],
        }
        mvp = {
            "trace_id": "mvp",
            "normalized_event_count": 1,
            "manifest_rows": rows,
            "stage_counts": {"step_prompt_built": 1},
            "gate_counts": {"G-S06": 1},
            "normalized_events": [
                {
                    "stage": "step_prompt_built",
                    "gate_ids": ["G-S06"],
                    "source_event": "step_prompt_contract",
                    "has_overall_goal": True,
                    "has_expected_paths": True,
                    "has_verify_commands": False,
                    "has_expected_result": False,
                }
            ],
        }
        diff = compare_trace_reports(source, mvp)
        by_gate = {item["gate_id"]: item for item in diff["gate_results"]}
        self.assertEqual(by_gate["G-S06"]["status"], "fail")
        self.assertEqual(by_gate["G-S06"]["reason"], "semantic_trace_contract_missing")
        kinds = {item["kind"] for item in diff["semantic_findings"]}
        self.assertIn("missing_expected_result", kinds)
        self.assertIn("missing_verify", kinds)

    def test_compare_reports_detects_missing_phase_context(self):
        rows = [
            {
                "suite": "s",
                "scenario": "case",
                "mode": "ultra-plan-run",
                "provider_model_pair": "openai:gpt planner=gemini:flash",
            }
        ]
        source_event = {
            "stage": "phase_context_attached",
            "gate_ids": ["G-S05"],
            "source_event": "source_phase_context_observed",
            "phase_index": 2,
            "has_ultra_goal": True,
            "has_current_phase": True,
            "has_workspace_snapshot": True,
            "has_profile_contract": True,
            "has_prior_conversation_context": True,
        }
        mvp_event = {
            "stage": "phase_context_attached",
            "gate_ids": ["G-S05"],
            "source_event": "ultra_phase_context_attached",
            "phase_index": 2,
            "has_previous_context": False,
        }
        diff = compare_trace_reports(
            {
                "trace_id": "source",
                "normalized_event_count": 1,
                "manifest_rows": rows,
                "stage_counts": {"phase_context_attached": 1},
                "gate_counts": {"G-S05": 1},
                "normalized_events": [source_event],
            },
            {
                "trace_id": "mvp",
                "normalized_event_count": 1,
                "manifest_rows": rows,
                "stage_counts": {"phase_context_attached": 1},
                "gate_counts": {"G-S05": 1},
                "normalized_events": [mvp_event],
            },
        )
        by_gate = {item["gate_id"]: item for item in diff["gate_results"]}
        self.assertEqual(by_gate["G-S05"]["status"], "fail")
        findings = [item for item in diff["semantic_findings"] if item["gate_id"] == "G-S05"]
        self.assertEqual(findings[0]["kind"], "missing_context")

    def test_compare_reports_does_not_pass_gate_counts_without_prompt_trace(self):
        rows = [
            {
                "suite": "s",
                "scenario": "case",
                "mode": "ultra-plan-run",
                "provider_model_pair": "openai:gpt planner=gemini:flash",
            }
        ]
        report = {
            "trace_id": "trace",
            "normalized_event_count": 1,
            "manifest_rows": rows,
            "stage_counts": {"plan_generated": 1},
            "gate_counts": {"G-S05": 1, "G-S06": 1},
        }
        diff = compare_trace_reports(report, report)
        by_gate = {item["gate_id"]: item for item in diff["gate_results"]}
        self.assertEqual(by_gate["G-S05"]["status"], "fail")
        self.assertEqual(by_gate["G-S06"]["status"], "fail")
        self.assertEqual(by_gate["G-S05"]["reason"], "semantic_trace_contract_missing")
        self.assertEqual(by_gate["G-S06"]["reason"], "semantic_trace_contract_missing")
        kinds = {item["kind"] for item in diff["semantic_findings"]}
        self.assertIn("missing_context", kinds)
        self.assertIn("missing_step_prompt", kinds)


if __name__ == "__main__":
    unittest.main()
