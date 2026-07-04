import json
import sys
import unittest
import tempfile
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
REPO_ROOT = ROOT.parents[1]
sys.path.insert(0, str(ROOT / "scripts"))

from eval_lib.parity_gate import (
    REQUIRED_GATE_IDS,
    build_parity_gate_report,
    release_evidence_blockers,
    release_evidence_gaps,
    validate_parity_gate_report,
)
from eval_lib.run_summary import SUMMARY_HEADER, write_summary


class ParityGateReportTest(unittest.TestCase):
    def base_report(self):
        return {
            "schema_version": "1",
            "gate_level": "comparative",
            "required_gate_ids": sorted(REQUIRED_GATE_IDS),
            "passed_gate_ids": [],
            "partial_gate_ids": sorted(REQUIRED_GATE_IDS - {"G-S14"}),
            "failed_gate_ids": ["G-S14"],
            "intentionally_different_gate_ids": [],
            "failure_kind_blank_count": 1,
            "anvildev_comparison": {
                "status": "missing_current_same_condition_trace",
            },
            "uat_equivalent": {
                "status": "partial",
                "evidence_paths": [],
            },
            "errors": [
                "failure_kind_blank_count is non-zero",
                "anvildev comparison missing",
            ],
        }

    def test_local_report_allows_known_partial_and_failed_partition(self):
        report = self.base_report()
        report["gate_level"] = "local"
        self.assertEqual(validate_parity_gate_report(report), [])

    def test_comparative_report_rejects_unresolved_partial_gates(self):
        errors = validate_parity_gate_report(self.base_report())
        self.assertTrue(any("partial gate ids unresolved" in error for error in errors))

    def test_report_rejects_missing_gate_partition(self):
        report = self.base_report()
        report["partial_gate_ids"] = []
        errors = validate_parity_gate_report(report)
        self.assertTrue(any("missing from status partition" in error for error in errors))

    def test_report_rejects_blank_failure_count_without_error(self):
        report = self.base_report()
        report["errors"] = ["anvildev comparison missing"]
        errors = validate_parity_gate_report(report)
        self.assertTrue(any("blank failure kind" in error for error in errors))

    def test_workspace_022_report_is_schema_valid_even_when_gate_fails(self):
        report_path = REPO_ROOT / "workspace/mvp/eval/022/parity_gate_report.json"
        if not report_path.exists():
            self.skipTest("workspace parity gate report is not checked out")
        report = json.loads(report_path.read_text(encoding="utf-8"))
        errors = validate_parity_gate_report(report)
        self.assertEqual(errors, [])

    def test_comparison_threshold_fail_requires_report_error_or_intentional_evidence(self):
        with tempfile.TemporaryDirectory() as td:
            root = Path(td)
            mvp = root / "mvp.tsv"
            source = root / "source.tsv"
            trace = root / "trace.json"
            trace.write_text(json.dumps(pass_trace_diff()), encoding="utf-8")
            write_summary(mvp, [summary_row(success=False), summary_row(success=False)])
            write_summary(source, [summary_row(success=True), summary_row(success=True)])
            report = build_parity_gate_report(
                gate_level="comparative",
                mvp_summary_path=str(mvp),
                anvildev_summary_path=str(source),
                trace_diff_path=str(trace),
            )
        self.assertEqual(report["anvildev_comparison"]["threshold"]["status"], "fail")
        self.assertTrue(any("anvildev parity threshold failed" in item for item in report["errors"]))
        self.assertEqual(validate_parity_gate_report(report), [])

        invalid = dict(report)
        invalid["errors"] = []
        self.assertTrue(
            any("anvildev parity threshold failure" in item for item in validate_parity_gate_report(invalid))
        )

    def test_comparison_threshold_allows_intentional_difference_evidence(self):
        with tempfile.TemporaryDirectory() as td:
            mvp = Path(td) / "mvp.tsv"
            source = Path(td) / "source.tsv"
            evidence = Path(td) / "intentional.md"
            evidence.write_text("intentional false-positive tightening", encoding="utf-8")
            write_summary(mvp, [summary_row(success=False), summary_row(success=False)])
            write_summary(source, [summary_row(success=True), summary_row(success=True)])
            report = build_parity_gate_report(
                gate_level="comparative",
                mvp_summary_path=str(mvp),
                anvildev_summary_path=str(source),
                intentional_difference_evidence_paths=[str(evidence)],
            )
        self.assertEqual(
            report["anvildev_comparison"]["threshold"]["status"],
            "intentional_difference",
        )
        self.assertFalse(any("anvildev parity threshold failed" in item for item in report["errors"]))
        self.assertEqual(validate_parity_gate_report(report), [])

    def test_comparative_report_includes_completion_authority_release_and_recovery_fields(self):
        with tempfile.TemporaryDirectory() as td:
            root = Path(td)
            mvp = root / "mvp.tsv"
            source = root / "source.tsv"
            trace = root / "trace.json"
            trace.write_text(json.dumps(pass_trace_diff()), encoding="utf-8")
            write_summary(
                mvp,
                [
                    summary_row(
                        success=True,
                        command_completion_state="completed",
                        final_acceptance_status="partial",
                        release_gate_status="partial",
                        browser_readiness_status="unavailable:browser_readiness_evidence_missing",
                        recovery_prompt_path=".anvil/repairs/repair-1.md",
                        recovery_ultra_plan_path=".anvil/plans/recovery-ultra-plan-1.yaml",
                    )
                ],
            )
            write_summary(
                source,
                [
                    summary_row(
                        success=True,
                        command_completion_state="completed",
                        final_acceptance_status="pass",
                        release_gate_status="pass",
                        browser_readiness_status="ready",
                    )
                ],
            )
            report = build_parity_gate_report(
                gate_level="comparative",
                mvp_summary_path=str(mvp),
                anvildev_summary_path=str(source),
                trace_diff_path=str(trace),
            )
        comparison = report["anvildev_comparison"]
        self.assertEqual(comparison["release_quality_status"], "fail")
        self.assertEqual(comparison["mvp"]["command_completion_state_counts"]["completed"], 1)
        self.assertEqual(comparison["mvp"]["final_acceptance_status_counts"]["partial"], 1)
        self.assertEqual(comparison["mvp"]["release_gate_status_counts"]["partial"], 1)
        self.assertEqual(
            comparison["mvp"]["browser_readiness_status_counts"][
                "unavailable:browser_readiness_evidence_missing"
            ],
            1,
        )
        self.assertEqual(
            comparison["mvp"]["recovery_artifact_presence_counts"][
                "prompt_and_recovery_ultra_plan"
            ],
            1,
        )
        self.assertEqual(
            comparison["mvp"]["completion_authority_reason_counts"][
                "release_gate_partial"
            ],
            1,
        )
        self.assertIn(
            "release_gate_status_counts",
            comparison["completion_authority_comparison"]["fields"],
        )
        self.assertTrue(
            any(
                item["key"] == "partial" and item["delta"] == 1
                for item in comparison["completion_authority_comparison"]["deltas"][
                    "release_gate_status_counts"
                ]
            )
        )
        self.assertIn(
            "release-quality comparison is not pass",
            "\n".join(report["warnings"]),
        )
        self.assertEqual(report["recovery_item_status"]["status"], "implementation_pass")
        self.assertFalse(report["recovery_item_status"]["release_pass"])

    def test_release_report_requires_release_quality_for_recovery_release_pass(self):
        with tempfile.TemporaryDirectory() as td:
            root = Path(td)
            mvp = root / "mvp.tsv"
            source = root / "source.tsv"
            uat, browser, interaction, events = release_evidence_files(root)
            trace = root / "trace.json"
            trace.write_text(json.dumps(pass_trace_diff()), encoding="utf-8")
            write_summary(
                mvp,
                [
                    summary_row(
                        success=True,
                        final_acceptance_status="partial",
                        release_gate_status="partial",
                    )
                ],
            )
            write_summary(
                source,
                [
                    summary_row(
                        success=True,
                        final_acceptance_status="pass",
                        release_gate_status="pass",
                    )
                ],
            )
            report = build_parity_gate_report(
                gate_level="release",
                mvp_summary_path=str(mvp),
                anvildev_summary_path=str(source),
                trace_diff_path=str(trace),
                uat_evidence_paths=[str(uat)],
                browser_evidence_paths=[str(browser)],
                interaction_evidence_paths=[str(interaction)],
                tui_event_paths=[str(events)],
            )
        self.assertEqual(report["anvildev_comparison"]["release_quality_status"], "fail")
        self.assertTrue(
            any("release-quality comparison is not pass" in item for item in report["errors"])
        )
        self.assertEqual(report["recovery_item_status"]["status"], "implementation_pass")
        self.assertIn(
            "release_quality_status:fail",
            report["recovery_item_status"]["release_blockers"],
        )

    def test_comparative_report_includes_trace_diff_and_resolves_gate_partition(self):
        with tempfile.TemporaryDirectory() as td:
            root = Path(td)
            source_trace = root / "source-trace.json"
            mvp_trace = root / "mvp-trace.json"
            rows = [
                {
                    "suite": "s",
                    "scenario": "case",
                    "mode": "minimal-loop",
                    "provider_model_pair": "openai:gpt planner=gemini:flash",
                }
            ]
            source_trace.write_text(
                json.dumps(
                    {
                        "trace_id": "source",
                        "normalized_event_count": 2,
                        "manifest_rows": rows,
                        "stage_counts": {"plan_generated": 1, "verify_started": 1},
                        "gate_counts": {"G-S03": 1, "G-S08": 1},
                    }
                ),
                encoding="utf-8",
            )
            mvp_trace.write_text(
                json.dumps(
                    {
                        "trace_id": "mvp",
                        "normalized_event_count": 1,
                        "manifest_rows": rows,
                        "stage_counts": {"plan_generated": 1},
                        "gate_counts": {"G-S03": 1},
                    }
                ),
                encoding="utf-8",
            )
            report = build_parity_gate_report(
                gate_level="comparative",
                source_trace_report_path=str(source_trace),
                mvp_trace_report_path=str(mvp_trace),
            )
        self.assertEqual(report["normalized_trace_diff"]["status"], "compared")
        self.assertEqual(report["partial_gate_ids"], [])
        self.assertIn("G-S03", report["passed_gate_ids"])
        self.assertIn("G-S08", report["failed_gate_ids"])
        self.assertEqual(validate_parity_gate_report(report), [])

    def test_comparative_report_fails_all_gates_when_trace_diff_missing(self):
        report = build_parity_gate_report(gate_level="comparative")
        self.assertEqual(report["partial_gate_ids"], [])
        self.assertEqual(set(report["failed_gate_ids"]), REQUIRED_GATE_IDS)
        self.assertTrue(
            any("normalized trace diff" in item for item in report["errors"])
        )
        self.assertEqual(validate_parity_gate_report(report), [])

    def test_release_gate_pass_requires_browser_interaction_and_tui_evidence(self):
        report = self.base_report()
        report["gate_level"] = "release"
        report["uat_equivalent"] = {
            "status": "pass",
            "evidence_paths": ["uat.md"],
        }
        errors = validate_parity_gate_report(report)
        self.assertTrue(any("release gate cannot pass" in item for item in errors))

        with tempfile.TemporaryDirectory() as td:
            root = Path(td)
            uat = root / "uat.md"
            browser = root / "browser.json"
            interaction = root / "interaction.json"
            events = root / "events.jsonl"
            uat.write_text("manual UAT evidence", encoding="utf-8")
            browser.write_text(json.dumps({"ok": True, "http_status": 200, "route_rendered": True}), encoding="utf-8")
            interaction.write_text(
                json.dumps(
                    {
                        "ok": True,
                        "interaction_performed": True,
                        "start_transition": True,
                        "input_state_change": True,
                        "state_changed": True,
                    }
                ),
                encoding="utf-8",
            )
            events.write_text(
                "\n".join(
                    [
                        json.dumps({"event": "run_start", "action": "Repl"}),
                        json.dumps({"event": "tui_command_stop", "ok": True}),
                        json.dumps({"event": "run_stop", "ok": True, "stop_reason": "completed"}),
                    ]
                )
                + "\n",
                encoding="utf-8",
            )
            report = build_parity_gate_report(
                base_report=self.base_report(),
                gate_level="release",
                uat_evidence_paths=[str(uat)],
                browser_evidence_paths=[str(browser)],
                interaction_evidence_paths=[str(interaction)],
                tui_event_paths=[str(events)],
            )
        self.assertEqual(release_evidence_gaps(report["uat_equivalent"]), [])
        self.assertEqual(release_evidence_blockers(report["uat_equivalent"]), [])
        self.assertEqual(report["uat_equivalent"]["status"], "pass")
        self.assertEqual(validate_parity_gate_report(report), [])

    def test_release_gate_reads_browser_evidence_content(self):
        with tempfile.TemporaryDirectory() as td:
            root = Path(td)
            uat, browser, interaction, events = release_evidence_files(root)
            browser.write_text(json.dumps({"ok": False, "http_status": 500}), encoding="utf-8")
            report = build_parity_gate_report(
                base_report=self.base_report(),
                gate_level="release",
                uat_evidence_paths=[str(uat)],
                browser_evidence_paths=[str(browser)],
                interaction_evidence_paths=[str(interaction)],
                tui_event_paths=[str(events)],
            )
        uat_equivalent = report["uat_equivalent"]
        self.assertEqual(uat_equivalent["status"], "fail")
        self.assertIn("browser_readiness:browser_http_500", release_evidence_blockers(uat_equivalent))
        self.assertIn("browser_http_500", uat_equivalent["reason"])

    def test_release_gate_keeps_tailwind_dev_pipeline_failure_kind(self):
        with tempfile.TemporaryDirectory() as td:
            root = Path(td)
            uat, browser, interaction, events = release_evidence_files(root)
            browser.write_text(
                json.dumps(
                    {
                        "ok": False,
                        "http_status": 500,
                        "browser_failure_kind": "tailwind_dev_pipeline_failure",
                        "body_excerpt": "Module parse failed: Unexpected character '@' (1:0)\n@tailwind base;",
                    }
                ),
                encoding="utf-8",
            )
            report = build_parity_gate_report(
                base_report=self.base_report(),
                gate_level="release",
                uat_evidence_paths=[str(uat)],
                browser_evidence_paths=[str(browser)],
                interaction_evidence_paths=[str(interaction)],
                tui_event_paths=[str(events)],
            )
        uat_equivalent = report["uat_equivalent"]
        self.assertEqual(uat_equivalent["status"], "fail")
        self.assertIn(
            "browser_readiness:tailwind_dev_pipeline_failure",
            release_evidence_blockers(uat_equivalent),
        )
        self.assertIn("tailwind_dev_pipeline_failure", uat_equivalent["reason"])

    def test_release_gate_rejects_tui_command_stop_false(self):
        with tempfile.TemporaryDirectory() as td:
            root = Path(td)
            uat, browser, interaction, events = release_evidence_files(root)
            events.write_text(
                "\n".join(
                    [
                        json.dumps({"event": "run_start", "action": "Repl"}),
                        json.dumps(
                            {
                                "event": "tui_command_stop",
                                "ok": False,
                                "failure_kind": "tui_command_failed",
                                "primary_reason": "dependency_setup_missing",
                            }
                        ),
                        json.dumps({"event": "run_stop", "ok": True, "stop_reason": "completed"}),
                    ]
                )
                + "\n",
                encoding="utf-8",
            )
            report = build_parity_gate_report(
                base_report=self.base_report(),
                gate_level="release",
                uat_evidence_paths=[str(uat)],
                browser_evidence_paths=[str(browser)],
                interaction_evidence_paths=[str(interaction)],
                tui_event_paths=[str(events)],
            )
        uat_equivalent = report["uat_equivalent"]
        self.assertEqual(uat_equivalent["status"], "fail")
        self.assertIn("tui:tui_command_failed", release_evidence_blockers(uat_equivalent))

    def test_release_gate_rejects_silent_tui_exit(self):
        with tempfile.TemporaryDirectory() as td:
            root = Path(td)
            uat, browser, interaction, events = release_evidence_files(root)
            events.write_text(
                "\n".join(
                    [
                        json.dumps({"event": "run_start", "action": "Repl"}),
                        json.dumps({"event": "run_stop", "ok": True, "stop_reason": "completed"}),
                    ]
                )
                + "\n",
                encoding="utf-8",
            )
            report = build_parity_gate_report(
                base_report=self.base_report(),
                gate_level="release",
                uat_evidence_paths=[str(uat)],
                browser_evidence_paths=[str(browser)],
                interaction_evidence_paths=[str(interaction)],
                tui_event_paths=[str(events)],
            )
        uat_equivalent = report["uat_equivalent"]
        self.assertEqual(uat_equivalent["status"], "fail")
        self.assertIn("tui:silent_exit", release_evidence_blockers(uat_equivalent))
        self.assertEqual(
            uat_equivalent["evidence_results"]["tui"]["reason"],
            "tui_command_stop_missing",
        )

    def test_release_gate_rejects_malformed_evidence_json(self):
        with tempfile.TemporaryDirectory() as td:
            root = Path(td)
            uat, browser, interaction, events = release_evidence_files(root)
            browser.write_text("{not-json", encoding="utf-8")
            report = build_parity_gate_report(
                base_report=self.base_report(),
                gate_level="release",
                uat_evidence_paths=[str(uat)],
                browser_evidence_paths=[str(browser)],
                interaction_evidence_paths=[str(interaction)],
                tui_event_paths=[str(events)],
            )
        uat_equivalent = report["uat_equivalent"]
        self.assertEqual(uat_equivalent["status"], "fail")
        self.assertIn("browser_readiness:evidence_invalid", release_evidence_blockers(uat_equivalent))
        self.assertEqual(
            uat_equivalent["evidence_results"]["browser_readiness"]["reason"],
            "evidence_invalid",
        )

    def test_release_gate_treats_browser_ok_without_render_detail_as_partial(self):
        with tempfile.TemporaryDirectory() as td:
            root = Path(td)
            uat, browser, interaction, events = release_evidence_files(root)
            browser.write_text(json.dumps({"ok": True, "http_status": 200}), encoding="utf-8")
            report = build_parity_gate_report(
                base_report=self.base_report(),
                gate_level="release",
                uat_evidence_paths=[str(uat)],
                browser_evidence_paths=[str(browser)],
                interaction_evidence_paths=[str(interaction)],
                tui_event_paths=[str(events)],
            )
        uat_equivalent = report["uat_equivalent"]
        self.assertEqual(uat_equivalent["status"], "partial")
        self.assertIn(
            "browser_readiness:browser_render_evidence_missing",
            release_evidence_gaps(uat_equivalent),
        )

    def test_release_gate_rejects_canvas_unavailable_interaction_evidence(self):
        with tempfile.TemporaryDirectory() as td:
            root = Path(td)
            uat, browser, interaction, events = release_evidence_files(root)
            interaction.write_text(
                json.dumps(
                    {
                        "ok": True,
                        "interaction_performed": True,
                        "start_transition": True,
                        "input_state_change": True,
                        "state_changed": True,
                        "canvas_found": False,
                    }
                ),
                encoding="utf-8",
            )
            report = build_parity_gate_report(
                base_report=self.base_report(),
                gate_level="release",
                uat_evidence_paths=[str(uat)],
                browser_evidence_paths=[str(browser)],
                interaction_evidence_paths=[str(interaction)],
                tui_event_paths=[str(events)],
            )
        uat_equivalent = report["uat_equivalent"]
        self.assertEqual(uat_equivalent["status"], "fail")
        self.assertIn("interaction:canvas_unavailable", release_evidence_blockers(uat_equivalent))


def summary_row(
    *,
    success: bool,
    failure_kind: str = "",
    failure_layer: str = "",
    acceptance_success: str = "",
    acceptance_false_positive: bool = False,
    command_completion_state: str = "",
    final_acceptance_status: str = "",
    release_gate_status: str = "",
    browser_readiness_status: str = "",
    recovery_prompt_path: str = "",
    recovery_ultra_plan_path: str = "",
):
    row = {key: "" for key in SUMMARY_HEADER}
    if not success and not failure_kind:
        failure_kind = "runtime_failure"
    if not success and not failure_layer:
        failure_layer = "runtime"
    row.update(
        {
            "run_id": f"r-{success}-{failure_kind}",
            "suite": "s",
            "scenario": "case",
            "mode": "minimal-loop",
            "success": str(success).lower(),
            "rc": "0" if success else "1",
            "failure_kind": failure_kind,
            "failure_layer": failure_layer,
            "acceptance_success": acceptance_success,
            "acceptance_false_positive": str(acceptance_false_positive).lower(),
            "command_completion_state": command_completion_state,
            "final_acceptance_status": final_acceptance_status,
            "release_gate_status": release_gate_status,
            "browser_readiness_status": browser_readiness_status,
            "recovery_prompt_path": recovery_prompt_path,
            "recovery_ultra_plan_path": recovery_ultra_plan_path,
        }
    )
    return row


def release_evidence_files(root: Path):
    uat = root / "uat.md"
    browser = root / "browser.json"
    interaction = root / "interaction.json"
    events = root / "events.jsonl"
    uat.write_text("manual UAT evidence", encoding="utf-8")
    browser.write_text(json.dumps({"ok": True, "http_status": 200, "route_rendered": True}), encoding="utf-8")
    interaction.write_text(
        json.dumps(
            {
                "ok": True,
                "interaction_performed": True,
                "start_transition": True,
                "input_state_change": True,
                "state_changed": True,
            }
        ),
        encoding="utf-8",
    )
    events.write_text(
        "\n".join(
            [
                json.dumps({"event": "run_start", "action": "Repl"}),
                json.dumps({"event": "tui_command_stop", "ok": True}),
                json.dumps({"event": "run_stop", "ok": True, "stop_reason": "completed"}),
            ]
        )
        + "\n",
        encoding="utf-8",
    )
    return uat, browser, interaction, events


def pass_trace_diff():
    return {
        "schema_version": "1",
        "status": "compared",
        "gate_results": [
            {
                "gate_id": gate_id,
                "status": "pass",
                "reason": "fixture_pass",
                "source_event_count": 1,
                "mvp_event_count": 1,
            }
            for gate_id in sorted(REQUIRED_GATE_IDS)
        ],
    }


if __name__ == "__main__":
    unittest.main()
