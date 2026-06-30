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
        source = {
            "trace_id": "source",
            "stage_counts": {"plan_generated": 1, "verify_started": 1},
            "gate_counts": {"G-S03": 1, "G-S08": 1},
        }
        mvp = {
            "trace_id": "mvp",
            "stage_counts": {"plan_generated": 1},
            "gate_counts": {"G-S03": 1},
        }
        diff = compare_trace_reports(source, mvp)
        self.assertEqual(diff["missing_stages_in_mvp"], ["verify_started"])
        self.assertEqual(diff["missing_gate_ids_in_mvp"], ["G-S08"])


if __name__ == "__main__":
    unittest.main()
