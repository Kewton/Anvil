"""Security regression tests for scripts/analyze_run.py.

These cases are built dynamically on a tempdir so they do not pollute
the golden snapshot fixtures. They cover symlink escape, path traversal,
and size-limit DoS guards.

Run from repo root:
    python3 -m unittest tests/test_analyze_run_security.py
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
SCRIPT = REPO_ROOT / "scripts" / "analyze_run.py"


def _run(run_dir: pathlib.Path, *, check: bool = False) -> subprocess.CompletedProcess:
    return subprocess.run(
        [sys.executable, str(SCRIPT), str(run_dir)],
        capture_output=True,
        text=True,
        check=check,
    )


def _write_json(path: pathlib.Path, obj: object) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(json.dumps(obj), encoding="utf-8")


class TestAnalyzeRunSecurity(unittest.TestCase):
    def setUp(self) -> None:
        self._tmp = tempfile.TemporaryDirectory()
        self.addCleanup(self._tmp.cleanup)
        self.tmp = pathlib.Path(self._tmp.name)
        self.run_dir = self.tmp / "run-dir"
        self.run_dir.mkdir()

    def _base_session(self, tool_calls: list[dict] | None = None) -> None:
        messages = [
            {
                "role": "assistant",
                "content": "ok",
                "tool_calls": tool_calls or [],
            }
        ]
        _write_json(self.run_dir / "session.json", {"id": "sec", "messages": messages})

    def test_page_tsx_symlink_outside_run_dir_yields_null(self) -> None:
        self._base_session()
        workdir = self.run_dir / "workdir" / "src" / "app"
        workdir.mkdir(parents=True)
        outside = self.tmp / "outside-page.tsx"
        outside.write_text("game enemy player", encoding="utf-8")
        os.symlink(outside, workdir / "page.tsx")

        r = _run(self.run_dir)
        self.assertEqual(r.returncode, 0, r.stderr)
        out = json.loads(r.stdout)
        self.assertIsNone(out["page_tsx_has_game_keywords"])

    def test_meta_symlink_outside_run_dir_yields_null(self) -> None:
        self._base_session()
        outside_meta = self.tmp / "outside-meta.json"
        _write_json(outside_meta, {"rc": 0, "elapsed_s": 42})
        os.symlink(outside_meta, self.run_dir / "meta.json")

        r = _run(self.run_dir)
        self.assertEqual(r.returncode, 0, r.stderr)
        out = json.loads(r.stdout)
        self.assertIsNone(out["rc"])
        self.assertIsNone(out["elapsed_s"])

    def test_llm_io_symlink_outside_run_dir_yields_null(self) -> None:
        self._base_session()
        outside_log = self.tmp / "outside-llm.jsonl"
        outside_log.write_text(
            json.dumps(
                {
                    "event": "ollama.generate.error",
                    "payload": {"kind": "status", "status": 500},
                }
            )
            + "\n",
            encoding="utf-8",
        )
        logs_dir = self.run_dir / "logs"
        logs_dir.mkdir()
        os.symlink(outside_log, logs_dir / "llm-io.jsonl")

        r = _run(self.run_dir)
        self.assertEqual(r.returncode, 0, r.stderr)
        out = json.loads(r.stdout)
        self.assertIsNone(out["error_500_count"])

    def test_session_symlink_exits_2(self) -> None:
        real = self.tmp / "real-session.json"
        _write_json(real, {"id": "x", "messages": []})
        os.symlink(real, self.run_dir / "session.json")

        r = _run(self.run_dir)
        self.assertEqual(r.returncode, 2)

    def test_session_arg_path_escape_not_emitted(self) -> None:
        (self.run_dir / "workdir").mkdir()
        self._base_session(
            tool_calls=[
                {"id": "1", "name": "Write", "arguments": {"path": "/etc/passwd"}},
                {"id": "2", "name": "Edit", "arguments": {"path": "../secret.txt"}},
                {
                    "id": "3",
                    "name": "Write",
                    "arguments": {"path": "src/app/page.tsx"},
                },
            ]
        )
        r = _run(self.run_dir)
        self.assertEqual(r.returncode, 0, r.stderr)
        out = json.loads(r.stdout)
        self.assertEqual(out["files_modified"], ["src/app/page.tsx"])

    def test_run_dir_symlink_exits_2(self) -> None:
        real = self.tmp / "real-run-dir"
        real.mkdir()
        _write_json(real / "session.json", {"id": "x", "messages": []})
        link = self.tmp / "link-run-dir"
        os.symlink(real, link)

        r = _run(link)
        self.assertEqual(r.returncode, 2)

    def test_oversized_meta_yields_null(self) -> None:
        self._base_session()
        meta_path = self.run_dir / "meta.json"
        # > 256 KiB JSON (use repeated key/value to exceed limit)
        huge = {"rc": 0, "elapsed_s": 1, "pad": "x" * (300 * 1024)}
        _write_json(meta_path, huge)

        r = _run(self.run_dir)
        self.assertEqual(r.returncode, 0, r.stderr)
        out = json.loads(r.stdout)
        self.assertIsNone(out["rc"])
        self.assertIsNone(out["elapsed_s"])

    def test_missing_run_dir_exits_2(self) -> None:
        missing = self.tmp / "does-not-exist"
        r = _run(missing)
        self.assertEqual(r.returncode, 2)

    def test_missing_argument_exits_1(self) -> None:
        r = subprocess.run(
            [sys.executable, str(SCRIPT)],
            capture_output=True,
            text=True,
        )
        self.assertEqual(r.returncode, 1)

    def test_broken_session_exits_3(self) -> None:
        (self.run_dir / "session.json").write_text("{not json", encoding="utf-8")
        r = _run(self.run_dir)
        self.assertEqual(r.returncode, 3)


if __name__ == "__main__":
    unittest.main()
