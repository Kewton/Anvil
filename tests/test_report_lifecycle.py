"""Issue #1007 — repair-to-pass lifecycle metrics in scripts/report.py.

Unit-level checks that load report.py as a module (no subprocess) and drive the
aggregation helpers directly over crafted per-run rows. Each row mirrors the
analyze_run.py output shape (top-level fields + a ``failure_observation`` block).

Run from repo root:
    python3 -m unittest tests.test_report_lifecycle -v
"""

from __future__ import annotations

import importlib.util
import pathlib
import sys
import unittest

REPO_ROOT = pathlib.Path(__file__).resolve().parents[1]
SCRIPT_PATH = REPO_ROOT / "scripts" / "report.py"


def _load_report():
    spec = importlib.util.spec_from_file_location("report_under_test", SCRIPT_PATH)
    assert spec is not None and spec.loader is not None
    module = importlib.util.module_from_spec(spec)
    # Register before exec so report.py's @dataclass forward references resolve.
    sys.modules[spec.name] = module
    spec.loader.exec_module(module)
    return module


report = _load_report()


def _row(
    *,
    task_kind: str = "coding",
    terminal_state: str,
    failure_class: str = "none",
    postcheck_success: bool | None = None,
    missing_deliverable: bool = False,
    generated_file_count: int = 1,
    evidence_runner_executed: bool = False,
    repair_count: int = 0,
    same_diagnostic_repeated: bool = False,
    deterministic_operator_hit: bool = False,
    recovery_strategy_count: int = 0,
    extra: dict | None = None,
) -> dict:
    row: dict = {
        "_failed": False,
        "task_kind": task_kind,
        "recovery_strategy_count": recovery_strategy_count,
        "failure_observation": {
            "terminal_state": terminal_state,
            "failure_class": failure_class,
            "postcheck_success": postcheck_success,
            "missing_deliverable": missing_deliverable,
            "generated_file_count": generated_file_count,
            "evidence_runner_executed": evidence_runner_executed,
            "repair_count": repair_count,
            "same_diagnostic_repeated": same_diagnostic_repeated,
            "deterministic_operator_hit": deterministic_operator_hit,
        },
    }
    if extra:
        row.update(extra)
    return row


def _completed() -> dict:
    return _row(
        terminal_state="completed",
        postcheck_success=True,
        evidence_runner_executed=True,
    )


def _repair_to_pass() -> dict:
    # Repaired (count >= 1) and ultimately completed -> a conversion.
    return _row(
        terminal_state="completed",
        postcheck_success=True,
        evidence_runner_executed=True,
        repair_count=2,
        recovery_strategy_count=3,  # 2 switches
    )


def _repair_exhausted() -> dict:
    return _row(
        terminal_state="evidence_repair_exhausted",
        failure_class="recovery_exhausted",
        postcheck_success=False,
        repair_count=2,
        same_diagnostic_repeated=True,
        deterministic_operator_hit=False,
        recovery_strategy_count=4,  # 3 switches
    )


def _binding_failure() -> dict:
    return _row(
        task_kind="data",
        terminal_state="evidence_binding_failed",
        failure_class="evidence_failed",
        postcheck_success=False,
    )


def _missing_deliverable() -> dict:
    return _row(
        task_kind="docs",
        terminal_state="missing_deliverable",
        failure_class="missing_deliverable",
        postcheck_success=False,
        missing_deliverable=True,
        generated_file_count=0,
    )


def _operator_hit_exhausted() -> dict:
    # Recovery exhausted, but a deterministic operator DID fire -> not missing.
    return _row(
        terminal_state="evidence_repair_safe_stop",
        failure_class="recovery_exhausted",
        postcheck_success=False,
        repair_count=1,
        deterministic_operator_hit=True,
        recovery_strategy_count=2,  # 1 switch
    )


class TestLifecycleMetrics(unittest.TestCase):
    def test_overall_funnel_counts(self) -> None:
        rows = [
            _completed(),
            _repair_to_pass(),
            _repair_exhausted(),
            _binding_failure(),
            _missing_deliverable(),
            _operator_hit_exhausted(),
        ]
        m = report._lifecycle_metrics(rows)
        self.assertEqual(m["runs"], 6)
        # scaffold complete for everyone except the missing-deliverable run.
        self.assertEqual(m["first_pass_scaffold_complete"]["ok"], 5)
        self.assertEqual(m["first_pass_scaffold_complete"]["total"], 6)
        # evidence runnable: completed, repair->pass, repair_exhausted,
        # operator_hit_exhausted (binding-failure + missing-deliverable excluded).
        self.assertEqual(m["first_evidence_runnable"]["ok"], 4)
        self.assertEqual(m["binding_failure_count"], 1)
        self.assertEqual(m["repair_loop_reached"], 3)
        self.assertEqual(m["repair_to_pass_conversion"]["reached"], 3)
        self.assertEqual(m["repair_to_pass_conversion"]["converted"], 1)
        self.assertAlmostEqual(m["repair_to_pass_conversion"]["rate"], 1 / 3)
        self.assertEqual(m["same_failure_repeated_count"], 1)
        self.assertEqual(m["strategy_switch_count"], 2 + 3 + 1)
        self.assertEqual(m["operator_missing_count"], 1)

    def test_repair_to_pass_conversion_rate_none_when_unreached(self) -> None:
        m = report._lifecycle_metrics([_completed()])
        self.assertEqual(m["repair_loop_reached"], 0)
        self.assertIsNone(m["repair_to_pass_conversion"]["rate"])

    def test_binding_failure_is_not_evidence_runnable(self) -> None:
        m = report._lifecycle_metrics([_binding_failure()])
        self.assertEqual(m["binding_failure_count"], 1)
        self.assertEqual(m["first_evidence_runnable"]["ok"], 0)
        # A binding failure still implies the deliverable existed.
        self.assertEqual(m["first_pass_scaffold_complete"]["ok"], 1)

    def test_operator_missing_requires_no_operator_hit(self) -> None:
        missing = report._lifecycle_metrics([_repair_exhausted()])
        self.assertEqual(missing["operator_missing_count"], 1)
        hit = report._lifecycle_metrics([_operator_hit_exhausted()])
        self.assertEqual(hit["operator_missing_count"], 0)

    def test_worker_lifecycle_bool_overrides_terminal_heuristic(self) -> None:
        # Explicit deliverable_created=False wins over a completed terminal.
        row = _completed()
        row["deliverable_created"] = False
        row["runner_bound"] = False
        m = report._lifecycle_metrics([row])
        self.assertEqual(m["first_pass_scaffold_complete"]["ok"], 0)
        self.assertEqual(m["first_evidence_runnable"]["ok"], 0)

    def test_strategy_switches_fall_back_to_label_list(self) -> None:
        row = _completed()
        del row["recovery_strategy_count"]
        row["recovery_strategies"] = ["tool_first_retry", "evidence_action"]
        m = report._lifecycle_metrics([row])
        self.assertEqual(m["strategy_switch_count"], 1)

    def test_failed_and_observationless_rows_are_excluded(self) -> None:
        rows = [
            _completed(),
            {"_failed": True, "_analyze_rc": 3},
            {"_failed": False},  # no failure_observation
        ]
        m = report._lifecycle_metrics(rows)
        self.assertEqual(m["runs"], 1)

    def test_empty_rows_yield_zero_runs(self) -> None:
        m = report._lifecycle_metrics([])
        self.assertEqual(m["runs"], 0)
        self.assertIsNone(m["first_pass_scaffold_complete"]["rate"])


class TestLifecycleMetricsByTaskKind(unittest.TestCase):
    def test_breakdown_groups_by_task_kind(self) -> None:
        rows = [
            _completed(),  # coding
            _repair_exhausted(),  # coding
            _binding_failure(),  # data
            _missing_deliverable(),  # docs
        ]
        by_kind = report._lifecycle_metrics_by_task_kind(rows)
        kinds = {item["task_kind"]: item for item in by_kind}
        self.assertEqual(sorted(kinds), ["coding", "data", "docs"])
        self.assertEqual(kinds["coding"]["runs"], 2)
        self.assertEqual(kinds["coding"]["operator_missing_count"], 1)
        self.assertEqual(kinds["data"]["binding_failure_count"], 1)
        self.assertEqual(kinds["data"]["first_evidence_runnable"]["ok"], 0)
        self.assertEqual(kinds["docs"]["first_pass_scaffold_complete"]["ok"], 0)

    def test_breakdown_excludes_failed_rows(self) -> None:
        rows = [_completed(), {"_failed": True, "_analyze_rc": 1}]
        by_kind = report._lifecycle_metrics_by_task_kind(rows)
        self.assertEqual([i["task_kind"] for i in by_kind], ["coding"])


class TestLifecycleRendering(unittest.TestCase):
    def test_markdown_section_lists_all_metrics(self) -> None:
        lines = report._render_lifecycle_metrics_summary([_repair_to_pass()])
        text = "\n".join(lines)
        self.assertIn("## Lifecycle Metrics", text)
        for metric in (
            "first_pass_scaffold_complete",
            "first_evidence_runnable",
            "binding_failure_count",
            "repair_loop_reached",
            "repair_to_pass_conversion",
            "same_failure_repeated_count",
            "strategy_switch_count",
            "operator_missing_count",
        ):
            self.assertIn(metric, text)
        self.assertIn("### Lifecycle Metrics By Task Kind", text)

    def test_markdown_section_handles_no_observations(self) -> None:
        lines = report._render_lifecycle_metrics_summary([])
        self.assertIn("(no completed analyses)", "\n".join(lines))


if __name__ == "__main__":
    unittest.main()
