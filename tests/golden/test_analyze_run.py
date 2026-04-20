"""Snapshot tests for scripts/analyze_run.py.

Each fixture under tests/golden/<case>/ consists of:
  run-dir/        input directory passed to analyze_run.py
  golden.json     expected JSON output (dict-sorted keys, schema v1)

Run from repo root:
    python3 -m unittest tests/golden/test_analyze_run.py
"""

from __future__ import annotations

import json
import pathlib
import subprocess
import sys
import unittest

CASES = ["full", "no-meta", "duplicate-root", "legacy-session"]

REPO_ROOT = pathlib.Path(__file__).resolve().parents[2]
SCRIPT = REPO_ROOT / "scripts" / "analyze_run.py"


class TestAnalyzeRun(unittest.TestCase):
    def _run(self, case: str) -> dict:
        run_dir = pathlib.Path(__file__).parent / case / "run-dir"
        result = subprocess.run(
            [sys.executable, str(SCRIPT), str(run_dir)],
            capture_output=True,
            text=True,
            check=True,
            cwd=str(REPO_ROOT),
        )
        return json.loads(result.stdout)

    def _golden(self, case: str) -> dict:
        golden = pathlib.Path(__file__).parent / case / "golden.json"
        return json.loads(golden.read_text())

    def test_full(self) -> None:
        self.assertEqual(self._run("full"), self._golden("full"))

    def test_no_meta(self) -> None:
        self.assertEqual(self._run("no-meta"), self._golden("no-meta"))

    def test_duplicate_root(self) -> None:
        self.assertEqual(self._run("duplicate-root"), self._golden("duplicate-root"))

    def test_legacy_session(self) -> None:
        self.assertEqual(self._run("legacy-session"), self._golden("legacy-session"))


if __name__ == "__main__":
    unittest.main()
