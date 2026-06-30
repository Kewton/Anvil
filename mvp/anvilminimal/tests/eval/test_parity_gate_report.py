import json
import sys
import unittest
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
REPO_ROOT = ROOT.parents[1]
sys.path.insert(0, str(ROOT / "scripts"))

from eval_lib.parity_gate import REQUIRED_GATE_IDS, validate_parity_gate_report


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


if __name__ == "__main__":
    unittest.main()
