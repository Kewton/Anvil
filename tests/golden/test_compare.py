"""Snapshot tests for scripts/compare.py.

Each fixture under tests/golden/compare/<case>/ consists of:
  baseline/<slug>/run-*/                 baseline BENCH_ROOT
  experiment/<slug>/run-*/               experiment BENCH_ROOT
  golden.md                              expected markdown output
  golden.json                            expected JSON output

Regenerate goldens (from repo root):
    UPDATE_GOLDEN=1 COMPARE_NOW=2026-01-01T00:00:00Z \
        python3 tests/golden/test_compare.py

Run from repo root:
    COMPARE_NOW=2026-01-01T00:00:00Z python3 tests/golden/test_compare.py
"""

from __future__ import annotations

import json
import os
import pathlib
import subprocess
import sys
import unittest

CASES = ["basic"]

REPO_ROOT = pathlib.Path(__file__).resolve().parents[2]
SCRIPT = REPO_ROOT / "scripts" / "compare.py"
FIXTURE_ROOT = pathlib.Path(__file__).parent / "compare"

COMPARE_NOW = os.environ.get("COMPARE_NOW", "2026-01-01T00:00:00Z")


def _run(case: str, fmt: str) -> str:
    case_dir = FIXTURE_ROOT / case
    env = os.environ.copy()
    env["COMPARE_NOW"] = COMPARE_NOW
    result = subprocess.run(
        [
            sys.executable,
            str(SCRIPT),
            str(case_dir / "baseline"),
            str(case_dir / "experiment"),
            "--format",
            fmt,
        ],
        capture_output=True,
        text=True,
        check=True,
        cwd=str(REPO_ROOT),
        env=env,
    )
    # Normalize absolute fixture paths to a repo-relative form so goldens are
    # stable across checkouts.
    out = result.stdout
    out = out.replace(str(REPO_ROOT), "<REPO_ROOT>")
    return out


def _maybe_update(case: str, fmt: str, actual: str) -> None:
    if os.environ.get("UPDATE_GOLDEN") != "1":
        return
    case_dir = FIXTURE_ROOT / case
    golden = case_dir / ("golden.md" if fmt == "markdown" else "golden.json")
    golden.write_text(actual, encoding="utf-8")


class TestCompare(unittest.TestCase):
    def _check(self, case: str, fmt: str) -> None:
        actual = _run(case, fmt)
        _maybe_update(case, fmt, actual)
        golden = FIXTURE_ROOT / case / ("golden.md" if fmt == "markdown" else "golden.json")
        self.assertTrue(
            golden.exists(),
            f"golden missing: {golden} (run with UPDATE_GOLDEN=1 to create)",
        )
        expected = golden.read_text(encoding="utf-8")
        if fmt == "json":
            # compare JSON structurally to tolerate key-order in case json.dumps
            # reorders (we already sort_keys, but defense-in-depth).
            self.assertEqual(json.loads(actual), json.loads(expected))
        else:
            self.assertEqual(actual, expected)

    def test_basic_markdown(self) -> None:
        self._check("basic", "markdown")

    def test_basic_json(self) -> None:
        self._check("basic", "json")


if __name__ == "__main__":
    unittest.main()
