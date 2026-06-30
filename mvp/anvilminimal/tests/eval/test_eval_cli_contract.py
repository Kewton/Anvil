import json
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(ROOT / "scripts"))

from eval_lib.run_summary import SUMMARY_HEADER, write_summary


class EvalCliContractTest(unittest.TestCase):
    def test_speed_cloud_5x_and_provider_limit_override_are_recorded_in_matrix(self):
        with tempfile.TemporaryDirectory() as td:
            run_root = Path(td) / "run"
            result = subprocess.run(
                [
                    sys.executable,
                    "scripts/eval-run.py",
                    "--suite",
                    "eval/suites/mvp-smoke.yaml",
                    "--model-profile",
                    "speed-cloud-5x",
                    "--modes",
                    "minimal-loop",
                    "--runs",
                    "1",
                    "--parallel",
                    "5",
                    "--provider-limit",
                    "4",
                    "--run-root",
                    str(run_root),
                    "--dry-run",
                ],
                cwd=ROOT,
                text=True,
                capture_output=True,
            )
            self.assertEqual(result.returncode, 0, result.stderr)
            matrix = json.loads((run_root / "matrix.json").read_text(encoding="utf-8"))
        self.assertTrue(matrix, "matrix should not be empty")
        self.assertEqual({item["_model_profile"] for item in matrix}, {"speed-cloud-5x"})
        self.assertEqual({item["_parallel_limit"] for item in matrix}, {5})
        self.assertEqual({item["_provider_limit"] for item in matrix}, {4})

    def test_anvildev_engine_minimal_is_rendered_in_child_command_not_eval_run_cli(self):
        with tempfile.TemporaryDirectory() as td:
            run_root = Path(td) / "run"
            result = subprocess.run(
                [
                    sys.executable,
                    "scripts/eval-run.py",
                    "--suite",
                    "eval/suites/mvp-smoke.yaml",
                    "--model-profile",
                    "speed-cloud-5x",
                    "--modes",
                    "minimal-loop",
                    "--scenario",
                    "nextjs-space-invaders-large",
                    "--runs",
                    "1",
                    "--binary",
                    "anvildev",
                    "--binary-kind",
                    "anvildev",
                    "--run-root",
                    str(run_root),
                    "--dry-run",
                ],
                cwd=ROOT,
                text=True,
                capture_output=True,
            )
            self.assertEqual(result.returncode, 0, result.stderr)
            commands = "\n".join(path.read_text(encoding="utf-8") for path in (run_root / "runs").glob("*/command.txt"))
        self.assertIn('"--engine" "minimal"', commands)

    def test_provider_smoke_suite_records_provider_probe_metadata(self):
        with tempfile.TemporaryDirectory() as td:
            run_root = Path(td) / "run"
            result = subprocess.run(
                [
                    sys.executable,
                    "scripts/eval-run.py",
                    "--suite",
                    "eval/suites/mvp-provider-smoke.yaml",
                    "--model-profile",
                    "openai-only",
                    "--modes",
                    "minimal-loop",
                    "--runs",
                    "1",
                    "--scenario",
                    "write-one-file-small",
                    "--run-root",
                    str(run_root),
                    "--dry-run",
                ],
                cwd=ROOT,
                text=True,
                capture_output=True,
            )
            self.assertEqual(result.returncode, 0, result.stderr)
            matrix = json.loads((run_root / "matrix.json").read_text(encoding="utf-8"))
        probe = matrix[0]["provider_probe"]
        self.assertTrue(probe["required_for_prompt_sensitive_fix"])
        self.assertIn("provider_probe", probe["command"])
        self.assertEqual(
            {item["provider"] for item in probe["probes"]},
            {"openai", "gemini", "ollama"},
        )

    def test_provider_probe_results_are_recorded_in_dry_run_summary(self):
        with tempfile.TemporaryDirectory() as td:
            td_path = Path(td)
            run_root = td_path / "run"
            probe = td_path / "provider-probe.jsonl"
            probe.write_text(
                "\n".join(
                    [
                        json.dumps(
                            {
                                "event": "provider_probe",
                                "provider": "openai",
                                "probe": "tool_args_shape",
                                "status": "passed",
                            }
                        ),
                        json.dumps(
                            {
                                "event": "provider_probe",
                                "provider": "gemini",
                                "probe": "function_calling_schema",
                                "status": "skipped",
                                "reason": "missing_gemini_api_key",
                            }
                        ),
                    ]
                )
                + "\n",
                encoding="utf-8",
            )
            result = subprocess.run(
                [
                    sys.executable,
                    "scripts/eval-run.py",
                    "--suite",
                    "eval/suites/mvp-provider-smoke.yaml",
                    "--model-profile",
                    "openai-only",
                    "--modes",
                    "minimal-loop",
                    "--runs",
                    "1",
                    "--scenario",
                    "write-one-file-small",
                    "--run-root",
                    str(run_root),
                    "--provider-probe-results",
                    str(probe),
                    "--dry-run",
                ],
                cwd=ROOT,
                text=True,
                capture_output=True,
            )
            self.assertEqual(result.returncode, 0, result.stderr)
            summary = (run_root / "summary.eval.tsv").read_text(encoding="utf-8")
            events = (run_root / "events.jsonl").read_text(encoding="utf-8")
            provider_summary = json.loads(
                (run_root / "provider_probe_summary.json").read_text(encoding="utf-8")
            )
        self.assertIn("provider_probe_status", summary)
        self.assertIn("\tpassed\t1\t0\t1\t", summary)
        self.assertIn('"event": "provider_probe"', events)
        self.assertIn('"provider": "openai"', events)
        self.assertEqual(provider_summary["status"], "passed")
        self.assertEqual(provider_summary["passed"], 1)
        self.assertEqual(provider_summary["skipped"], 1)

    def test_eval_preflight_writes_comparative_parity_gate_report(self):
        with tempfile.TemporaryDirectory() as td:
            td_path = Path(td)
            run_root = td_path / "preflight"
            mvp = td_path / "mvp.summary.eval.tsv"
            source = td_path / "source.summary.eval.tsv"
            report = td_path / "parity_gate_report.json"
            write_summary(mvp, [summary_row(True), summary_row(True)])
            write_summary(source, [summary_row(True), summary_row(True)])
            result = subprocess.run(
                [
                    sys.executable,
                    "scripts/eval-preflight.py",
                    "--suite",
                    "eval/suites/mvp-smoke.yaml",
                    "--model-profile",
                    "openai-only",
                    "--modes",
                    "minimal-loop",
                    "--runs",
                    "1",
                    "--binary",
                    "python3",
                    "--offline-ok",
                    "--gate-level",
                    "comparative",
                    "--mvp-summary",
                    str(mvp),
                    "--anvildev-summary",
                    str(source),
                    "--write-parity-gate-report",
                    str(report),
                    "--run-root",
                    str(run_root),
                ],
                cwd=ROOT,
                text=True,
                capture_output=True,
            )
            self.assertEqual(result.returncode, 0, result.stderr)
            generated = json.loads(report.read_text(encoding="utf-8"))
            preflight = json.loads((run_root / "preflight.json").read_text(encoding="utf-8"))
        self.assertEqual(generated["anvildev_comparison"]["status"], "compared")
        self.assertEqual(generated["anvildev_comparison"]["threshold"]["status"], "pass")
        self.assertTrue(preflight["parity_gate"]["ok"])

    def test_eval_preflight_fails_comparative_gate_threshold(self):
        with tempfile.TemporaryDirectory() as td:
            td_path = Path(td)
            mvp = td_path / "mvp.summary.eval.tsv"
            source = td_path / "source.summary.eval.tsv"
            write_summary(mvp, [summary_row(False), summary_row(False)])
            write_summary(source, [summary_row(True), summary_row(True)])
            result = subprocess.run(
                [
                    sys.executable,
                    "scripts/eval-preflight.py",
                    "--suite",
                    "eval/suites/mvp-smoke.yaml",
                    "--model-profile",
                    "openai-only",
                    "--modes",
                    "minimal-loop",
                    "--runs",
                    "1",
                    "--binary",
                    "python3",
                    "--offline-ok",
                    "--gate-level",
                    "comparative",
                    "--mvp-summary",
                    str(mvp),
                    "--anvildev-summary",
                    str(source),
                    "--run-root",
                    str(td_path / "preflight"),
                ],
                cwd=ROOT,
                text=True,
                capture_output=True,
            )
        self.assertEqual(result.returncode, 5, result.stdout)


def summary_row(success: bool):
    row = {key: "" for key in SUMMARY_HEADER}
    row.update(
        {
            "run_id": f"r-{success}",
            "suite": "s",
            "scenario": "case",
            "mode": "minimal-loop",
            "success": str(success).lower(),
            "rc": "0" if success else "1",
            "failure_kind": "" if success else "runtime_failure",
            "failure_layer": "" if success else "runtime",
            "acceptance_success": str(success).lower(),
            "acceptance_false_positive": "false",
        }
    )
    return row


if __name__ == "__main__":
    unittest.main()
