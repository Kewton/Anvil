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

    def test_valid_report_allows_known_partial_and_failed_partition(self):
        self.assertEqual(validate_parity_gate_report(self.base_report()), [])

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
            mvp = Path(td) / "mvp.tsv"
            source = Path(td) / "source.tsv"
            write_summary(mvp, [summary_row(success=False), summary_row(success=False)])
            write_summary(source, [summary_row(success=True), summary_row(success=True)])
            report = build_parity_gate_report(
                gate_level="comparative",
                mvp_summary_path=str(mvp),
                anvildev_summary_path=str(source),
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
                json.dumps({"ok": True, "interaction_performed": True, "state_changed": True}),
                encoding="utf-8",
            )
            events.write_text(
                json.dumps({"event": "tui_command_stop", "ok": True}) + "\n",
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

    def test_release_gate_rejects_tui_command_stop_false(self):
        with tempfile.TemporaryDirectory() as td:
            root = Path(td)
            uat, browser, interaction, events = release_evidence_files(root)
            events.write_text(
                json.dumps(
                    {
                        "event": "tui_command_stop",
                        "ok": False,
                        "failure_kind": "tui_command_failed",
                        "primary_reason": "dependency_setup_missing",
                    }
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
        json.dumps({"ok": True, "interaction_performed": True, "state_changed": True}),
        encoding="utf-8",
    )
    events.write_text(
        json.dumps({"event": "tui_command_stop", "ok": True}) + "\n",
        encoding="utf-8",
    )
    return uat, browser, interaction, events


if __name__ == "__main__":
    unittest.main()
