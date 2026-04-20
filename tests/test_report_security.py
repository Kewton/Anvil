"""Security regression tests for scripts/report.py.

Built dynamically on a tempdir so they do not pollute the golden fixtures.

Run from repo root:
    python3 -m unittest tests.test_report_security -v
"""

from __future__ import annotations

import json
import os
import pathlib
import subprocess
import sys
import tempfile
import unittest

REPO_ROOT = pathlib.Path(__file__).resolve().parents[1]
SCRIPT = REPO_ROOT / "scripts" / "report.py"


def _run(*args: str) -> subprocess.CompletedProcess:
    return subprocess.run(
        [sys.executable, str(SCRIPT), *args],
        capture_output=True,
        text=True,
        cwd=str(REPO_ROOT),
        check=False,
    )


def _write_json(path: pathlib.Path, obj: object) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(json.dumps(obj), encoding="utf-8")


def _make_run_dir(run_dir: pathlib.Path, *, rid: str = "x", rc: int = 0, elapsed: int = 1) -> None:
    _write_json(
        run_dir / "session.json",
        {
            "id": rid,
            "messages": [
                {"role": "assistant", "content": "ok", "tool_calls": []}
            ],
        },
    )
    _write_json(run_dir / "meta.json", {"rc": rc, "elapsed_s": elapsed})


class TestReportSecurity(unittest.TestCase):
    def setUp(self) -> None:
        self._tmp = tempfile.TemporaryDirectory()
        self.addCleanup(self._tmp.cleanup)
        self.tmp = pathlib.Path(self._tmp.name)

    # ------------------------------------------------------------------
    # BENCH_ROOT-level rejection
    # ------------------------------------------------------------------

    def test_bench_root_symlink_exits_1(self) -> None:
        real = self.tmp / "real-root"
        real.mkdir()
        link = self.tmp / "link-root"
        os.symlink(real, link)

        r = _run(str(link))
        self.assertEqual(r.returncode, 1)
        self.assertIn("symlink", r.stderr)

    def test_missing_bench_root_exits_1(self) -> None:
        missing = self.tmp / "does-not-exist"
        r = _run(str(missing))
        self.assertEqual(r.returncode, 1)

    def test_bench_root_is_file_exits_1(self) -> None:
        f = self.tmp / "not-a-dir"
        f.write_text("x", encoding="utf-8")
        r = _run(str(f))
        self.assertEqual(r.returncode, 1)

    def test_traversal_arg_rejected(self) -> None:
        # `../something-not-here` - resolution will fail strict=True.
        r = _run(str(self.tmp / ".." / "definitely-missing-xyz"))
        self.assertEqual(r.returncode, 1)

    # ------------------------------------------------------------------
    # symlinks within BENCH_ROOT are skipped, not fatal
    # ------------------------------------------------------------------

    def test_model_dir_symlink_skipped(self) -> None:
        bench_root = self.tmp / "bench-root"
        bench_root.mkdir()
        # real model with one run
        _make_run_dir(bench_root / "qwen3" / "run-1", rid="ok")
        # symlinked model dir (target is outside bench_root)
        outside_model = self.tmp / "outside-model"
        outside_model.mkdir()
        _make_run_dir(outside_model / "run-1", rid="evil")
        os.symlink(outside_model, bench_root / "evil-model")

        r = _run(str(bench_root))
        self.assertEqual(r.returncode, 0, msg=r.stderr)
        # only the real model appears in stdout
        self.assertIn("qwen3", r.stdout)
        self.assertNotIn("evil-model", r.stdout)
        self.assertIn("symlink", r.stderr)

    def test_run_dir_symlink_skipped(self) -> None:
        bench_root = self.tmp / "bench-root"
        bench_root.mkdir()
        model_dir = bench_root / "qwen3"
        model_dir.mkdir()
        # real run-1
        _make_run_dir(model_dir / "run-1", rid="ok")
        # symlinked run-2
        outside_run = self.tmp / "outside-run"
        _make_run_dir(outside_run, rid="evil")
        os.symlink(outside_run, model_dir / "run-2")

        r = _run(str(bench_root))
        self.assertEqual(r.returncode, 0, msg=r.stderr)
        # the symlinked run is skipped; only run-1 appears
        self.assertIn("| 1 | qwen3", r.stdout)
        self.assertNotIn("| 2 | qwen3", r.stdout)
        self.assertIn("symlink", r.stderr)

    # ------------------------------------------------------------------
    # invalid-named directories are silently skipped (no listing)
    # ------------------------------------------------------------------

    def test_invalid_model_slug_skipped(self) -> None:
        bench_root = self.tmp / "bench-root"
        bench_root.mkdir()
        # invalid name with whitespace and unicode
        bad = bench_root / "bad name!"
        bad.mkdir()
        _make_run_dir(bad / "run-1", rid="bad")
        # valid sibling
        _make_run_dir(bench_root / "qwen3" / "run-1", rid="ok")

        r = _run(str(bench_root))
        self.assertEqual(r.returncode, 0, msg=r.stderr)
        self.assertIn("qwen3", r.stdout)
        self.assertNotIn("bad name", r.stdout)

    def test_non_run_subdir_skipped(self) -> None:
        bench_root = self.tmp / "bench-root"
        bench_root.mkdir()
        model_dir = bench_root / "qwen3"
        model_dir.mkdir()
        _make_run_dir(model_dir / "run-1", rid="ok")
        # extra non-run-* directory
        (model_dir / "scratch").mkdir()
        _write_json(model_dir / "scratch" / "session.json", {"id": "no", "messages": []})

        r = _run(str(bench_root))
        self.assertEqual(r.returncode, 0, msg=r.stderr)
        self.assertIn("| 1 | qwen3", r.stdout)
        # scratch should not appear as a row
        self.assertEqual(r.stdout.count("qwen3 |"), 1)

    # ------------------------------------------------------------------
    # analyze_run.py failure: warn and continue
    # ------------------------------------------------------------------

    def test_partial_failure_continues(self) -> None:
        bench_root = self.tmp / "bench-root"
        bench_root.mkdir()
        model_dir = bench_root / "qwen3"
        model_dir.mkdir()
        _make_run_dir(model_dir / "run-1", rid="ok")
        # broken session.json -> analyze_run.py exits 3
        broken = model_dir / "run-2"
        broken.mkdir()
        (broken / "session.json").write_text("{not json", encoding="utf-8")

        r = _run(str(bench_root))
        self.assertEqual(r.returncode, 0, msg=r.stderr)
        self.assertIn("| 1 | qwen3", r.stdout)
        # row for run-2 still present (with N/A) and warning emitted
        self.assertIn("| 2 | qwen3", r.stdout)
        self.assertIn("rc=3", r.stderr)

    # ------------------------------------------------------------------
    # markdown injection via tool_calls names in --compare output
    # ------------------------------------------------------------------

    def test_compare_tool_calls_name_is_escaped(self) -> None:
        """A malicious tool_calls name must not break the Markdown table.

        analyze_run.py extracts the ``name`` field of each ``tool_calls``
        entry from session.json verbatim. report.py must escape the name
        before embedding it into the Markdown comparison table; otherwise
        a hostile session can inject extra columns or rows.
        """
        # Build root A with a normal run and a hostile tool_calls name.
        root_a = self.tmp / "root-a"
        root_a.mkdir()
        run_a = root_a / "qwen3" / "run-1"
        evil_name = "evil | 999 | 999\nINJECTED ROW | 1 | 1\n`code`"
        _write_json(
            run_a / "session.json",
            {
                "id": "a1",
                "messages": [
                    {
                        "role": "assistant",
                        "content": "ok",
                        "tool_calls": [
                            {"name": evil_name, "arguments": {}}
                        ],
                    }
                ],
            },
        )
        _write_json(run_a / "meta.json", {"rc": 0, "elapsed_s": 1})

        # Build root B with a normal run (no overlap on tool name).
        root_b = self.tmp / "root-b"
        root_b.mkdir()
        _make_run_dir(root_b / "qwen3" / "run-1", rid="b1")

        r = _run("--compare", str(root_a), str(root_b))
        self.assertEqual(r.returncode, 0, msg=r.stderr)

        # The table row for the hostile tool name must be a single line
        # containing exactly four cells (4 unescaped pipes -> 5 segments).
        # Find the "Tool Call Comparison" section.
        self.assertIn("## Tool Call Comparison", r.stdout)
        tc_section = r.stdout.split("## Tool Call Comparison", 1)[1]
        # Locate the row that mentions our hostile name (escaped).
        row_lines = [
            line
            for line in tc_section.splitlines()
            if line.startswith("| ") and "evil" in line
        ]
        self.assertEqual(
            len(row_lines),
            1,
            msg=f"expected exactly one row for the hostile name; got {row_lines!r}",
        )
        row = row_lines[0]
        # Pipe characters in the embedded name MUST be escaped (\\|), so
        # the literal sequence " | 999 | 999" must NOT appear unescaped.
        self.assertNotIn(" | 999 | 999", row)
        # The injected newline-separated row must NOT appear as a real row.
        injected_rows = [
            line
            for line in tc_section.splitlines()
            if line.startswith("| INJECTED ROW")
        ]
        self.assertEqual(injected_rows, [])
        # Backticks in the name must be escaped.
        self.assertNotIn("`code`", row)


if __name__ == "__main__":
    unittest.main()
