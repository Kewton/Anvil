"""Task-kind evaluation reporting regressions for Issue #879."""

from __future__ import annotations

import json
import pathlib
import subprocess
import sys
import tempfile
import unittest

REPO_ROOT = pathlib.Path(__file__).resolve().parents[1]
ANALYZE = REPO_ROOT / "scripts" / "analyze_run.py"
REPORT = REPO_ROOT / "scripts" / "report.py"


def _write_json(path: pathlib.Path, obj: object) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(json.dumps(obj), encoding="utf-8")


def _make_run(
    run_dir: pathlib.Path,
    *,
    task_kind: str,
    pam_variant: str,
    modified_path: str,
    rc: int = 0,
) -> None:
    _write_json(
        run_dir / "session.json",
        {
            "id": f"{task_kind}-{pam_variant}",
            "messages": [
                {
                    "role": "assistant",
                    "content": "done",
                    "tool_calls": [
                        {
                            "name": "Write",
                            "arguments": {"path": modified_path},
                        }
                    ],
                }
            ],
        },
    )
    _write_json(
        run_dir / "meta.json",
        {
            "case": f"{task_kind}-case",
            "elapsed_s": 1,
            "model": "qwen3",
            "pam_variant": pam_variant,
            "rc": rc,
            "task_kind": task_kind,
        },
    )


class TestTaskKindEvalReporting(unittest.TestCase):
    def test_analyze_excludes_protected_files_from_postcheck(self) -> None:
        with tempfile.TemporaryDirectory() as raw:
            run_dir = pathlib.Path(raw) / "run-1"
            _make_run(
                run_dir,
                task_kind="docs",
                pam_variant="pam_on",
                modified_path="logs/llm-io.jsonl",
            )

            result = subprocess.run(
                [sys.executable, str(ANALYZE), str(run_dir)],
                capture_output=True,
                text=True,
                cwd=str(REPO_ROOT),
                check=True,
            )
            data = json.loads(result.stdout)

        self.assertEqual(data["artifact_files"], [])
        self.assertEqual(data["artifact_file_count"], 0)
        self.assertFalse(data["postcheck_success"])
        self.assertEqual(data["postcheck_reason"], "no_user_artifact")

    def test_report_summarizes_nested_task_kind_and_pam_runs(self) -> None:
        with tempfile.TemporaryDirectory() as raw:
            bench_root = pathlib.Path(raw) / "bench-root"
            _make_run(
                bench_root / "qwen3" / "docs" / "pam_on" / "run-1",
                task_kind="docs",
                pam_variant="pam_on",
                modified_path="README.md",
            )
            _make_run(
                bench_root / "qwen3" / "data" / "pam_off" / "run-1",
                task_kind="data",
                pam_variant="pam_off",
                modified_path="logs/eval.jsonl",
            )

            result = subprocess.run(
                [sys.executable, str(REPORT), str(bench_root)],
                capture_output=True,
                text=True,
                cwd=str(REPO_ROOT),
                check=True,
            )
            out = result.stdout

        self.assertIn("## Task Kind Summary", out)
        self.assertIn("## PAM By Task Kind", out)
        self.assertIn("| docs | 1 | 100% (1/1) | 100% (1/1) | 100% (1/1) |", out)
        self.assertIn("| data | 1 | 100% (1/1) | 0% (0/1) | 0% (0/1) |", out)
        self.assertIn("| docs | pam_on | 1 | 100% (1/1) | 100% (1/1) | 100% (1/1) |", out)
        self.assertIn("| data | pam_off | 1 | 100% (1/1) | 0% (0/1) | 0% (0/1) |", out)


if __name__ == "__main__":
    unittest.main()
