"""Snapshot tests for scripts/report.py.

Run from repo root:
    python3 -m unittest tests.test_report -v
"""

from __future__ import annotations

import pathlib
import re
import subprocess
import sys
import unittest

REPO_ROOT = pathlib.Path(__file__).resolve().parents[1]
SCRIPT = REPO_ROOT / "scripts" / "report.py"
FIXTURES = REPO_ROOT / "tests" / "golden" / "report"

GENERATED_LINE_RE = re.compile(r"^Generated: .*$", re.MULTILINE)


def _strip_generated(text: str) -> str:
    """Replace the ``Generated: <timestamp>`` line with a stable placeholder."""
    return GENERATED_LINE_RE.sub("Generated: <ts>", text)


def _read(path: pathlib.Path) -> str:
    return path.read_text(encoding="utf-8")


class TestReportSingleRoot(unittest.TestCase):
    def _run_report(self, case: str) -> str:
        bench_root = FIXTURES / case / "bench-root"
        result = subprocess.run(
            [sys.executable, str(SCRIPT), str(bench_root)],
            capture_output=True,
            text=True,
            cwd=str(REPO_ROOT),
            check=False,
        )
        self.assertEqual(result.returncode, 0, msg=result.stderr)
        return result.stdout

    def _assert_matches_golden(self, case: str, golden_name: str = "golden-report.md") -> None:
        actual = _strip_generated(self._run_report(case))
        expected = _strip_generated(_read(FIXTURES / case / golden_name))
        self.assertEqual(actual, expected)

    def test_single_model(self) -> None:
        self._assert_matches_golden("single-model")

    def test_multi_model(self) -> None:
        self._assert_matches_golden("multi-model")

    def test_empty_bench_root(self) -> None:
        self._assert_matches_golden("empty-bench-root")

    def test_partial_failure(self) -> None:
        self._assert_matches_golden("partial-failure")


class TestReportCompare(unittest.TestCase):
    def _run_compare(self, case: str) -> str:
        a = FIXTURES / case / "bench-root-a"
        b = FIXTURES / case / "bench-root-b"
        result = subprocess.run(
            [sys.executable, str(SCRIPT), "--compare", str(a), str(b)],
            capture_output=True,
            text=True,
            cwd=str(REPO_ROOT),
            check=False,
        )
        self.assertEqual(result.returncode, 0, msg=result.stderr)
        return result.stdout

    def _assert_matches_golden(self, case: str) -> None:
        actual = _strip_generated(self._run_compare(case))
        expected = _strip_generated(_read(FIXTURES / case / "golden-compare.md"))
        self.assertEqual(actual, expected)

    def test_compare_missing_models(self) -> None:
        self._assert_matches_golden("compare-missing-models")

    def test_compare_unequal_runs(self) -> None:
        self._assert_matches_golden("compare-unequal-runs")


class TestReportCli(unittest.TestCase):
    def test_no_args_exits_1(self) -> None:
        result = subprocess.run(
            [sys.executable, str(SCRIPT)],
            capture_output=True,
            text=True,
            cwd=str(REPO_ROOT),
            check=False,
        )
        self.assertEqual(result.returncode, 1)

    def test_help_works(self) -> None:
        result = subprocess.run(
            [sys.executable, str(SCRIPT), "--help"],
            capture_output=True,
            text=True,
            cwd=str(REPO_ROOT),
            check=False,
        )
        self.assertEqual(result.returncode, 0)
        self.assertIn("BENCH_ROOT", result.stdout)


if __name__ == "__main__":
    unittest.main()
