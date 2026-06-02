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
    postcheck_success: bool | None = None,
    postcheck_reason: str | None = None,
    failure_authority: str | None = None,
    last_feedback_kind: str | None = None,
    anvil_score: dict[str, object] | None = None,
) -> None:
    session: dict[str, object] = {
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
    }
    if last_feedback_kind is not None:
        session["last_feedback"] = {"kind": last_feedback_kind}
    if anvil_score is not None:
        session["last_anvil_score"] = anvil_score
    _write_json(
        run_dir / "session.json",
        session,
    )
    meta: dict[str, object] = {
        "case": f"{task_kind}-case",
        "elapsed_s": 1,
        "model": "qwen3",
        "pam_variant": pam_variant,
        "rc": rc,
        "task_kind": task_kind,
    }
    if postcheck_success is not None:
        meta["postcheck_success"] = postcheck_success
    if postcheck_reason is not None:
        meta["postcheck_reason"] = postcheck_reason
    if failure_authority is not None:
        meta["failure_authority"] = failure_authority
    _write_json(
        run_dir / "meta.json",
        meta,
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

    def test_analyze_classifies_generated_test_bug_as_false_negative(self) -> None:
        with tempfile.TemporaryDirectory() as raw:
            run_dir = pathlib.Path(raw) / "run-1"
            _make_run(
                run_dir,
                task_kind="coding",
                pam_variant="pam_off",
                modified_path="tests/generated_test.py",
                rc=1,
                last_feedback_kind="test_failure",
                anvil_score={
                    "tests_passed": False,
                    "test_failure_count": 1,
                    "test_files_changed": 1,
                    "implementation_files_changed": 0,
                    "setup_files_changed": 0,
                },
            )

            result = subprocess.run(
                [sys.executable, str(ANALYZE), str(run_dir)],
                capture_output=True,
                text=True,
                cwd=str(REPO_ROOT),
                check=True,
            )
            data = json.loads(result.stdout)

        self.assertTrue(data["postcheck_success"])
        self.assertEqual(data["outcome_agreement"], "false_negative")
        self.assertEqual(data["failure_authority"], "generated_test_bug")
        self.assertNotEqual(data["failure_authority"], "implementation_bug")
        self.assertEqual(
            data["evaluation_taxonomy"]["failure_authority"], "generated_test_bug"
        )

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

        self.assertIn("## PAM Summary", out)
        self.assertIn("## Task Kind Summary", out)
        self.assertIn("## PAM By Task Kind", out)
        self.assertIn("false_positive", out)
        self.assertIn("false_negative", out)
        self.assertIn(
            "| docs | 1 | 100% (1/1) | 100% (1/1) | 100% (1/1) | 1 | 0 | 0 | 0 |",
            out,
        )
        self.assertIn(
            "| data | 1 | 100% (1/1) | 0% (0/1) | 0% (0/1) | 0 | 1 | 0 | 0 |",
            out,
        )
        self.assertIn(
            "| docs | pam_on | 1 | 100% (1/1) | 100% (1/1) | 100% (1/1) | 1 | 0 | 0 | 0 |",
            out,
        )
        self.assertIn(
            "| data | pam_off | 1 | 100% (1/1) | 0% (0/1) | 0% (0/1) | 0 | 1 | 0 | 0 |",
            out,
        )

    def test_report_json_splits_pam_task_kind_and_failure_authority(self) -> None:
        with tempfile.TemporaryDirectory() as raw:
            bench_root = pathlib.Path(raw) / "bench-root"
            _make_run(
                bench_root / "qwen3" / "docs" / "pam_off" / "run-1",
                task_kind="docs",
                pam_variant="pam_off",
                modified_path="README.md",
                rc=0,
                postcheck_success=False,
                postcheck_reason="external_docs_postcheck",
                failure_authority="artifact_classification",
            )
            _make_run(
                bench_root / "qwen3" / "data" / "pam_off" / "run-1",
                task_kind="data",
                pam_variant="pam_off",
                modified_path="tests/generated_data_test.py",
                rc=1,
                postcheck_success=True,
                postcheck_reason="external_data_postcheck",
                failure_authority="generated_test_bug",
            )
            _make_run(
                bench_root / "qwen3" / "coding" / "pam_on" / "run-1",
                task_kind="coding",
                pam_variant="pam_on",
                modified_path="src/lib.rs",
                rc=0,
                postcheck_success=True,
                postcheck_reason="external_coding_postcheck",
                failure_authority="success",
            )
            _make_run(
                bench_root / "qwen3" / "coding" / "pam_on" / "run-2",
                task_kind="coding",
                pam_variant="pam_on",
                modified_path="src/main.rs",
                rc=1,
                postcheck_success=False,
                postcheck_reason="external_coding_postcheck",
                failure_authority="implementation_bug",
            )

            result = subprocess.run(
                [sys.executable, str(REPORT), "--format", "json", str(bench_root)],
                capture_output=True,
                text=True,
                cwd=str(REPO_ROOT),
                check=True,
            )
            data = json.loads(result.stdout)

        def by_key(items: list[dict[str, object]], key: str, value: str) -> dict[str, object]:
            for item in items:
                if item.get(key) == value:
                    return item
            raise AssertionError(f"missing {key}={value}: {items}")

        pam_off = by_key(data["by_pam_variant"], "pam_variant", "pam_off")
        self.assertEqual(
            pam_off["outcome_agreement"]["false_positive"], 1
        )
        self.assertEqual(
            pam_off["outcome_agreement"]["false_negative"], 1
        )
        docs = by_key(data["by_task_kind"], "task_kind", "docs")
        data_kind = by_key(data["by_task_kind"], "task_kind", "data")
        self.assertEqual(docs["outcome_agreement"]["false_positive"], 1)
        self.assertEqual(data_kind["outcome_agreement"]["false_negative"], 1)

        generated = by_key(
            data["by_failure_authority"],
            "failure_authority",
            "generated_test_bug",
        )
        implementation = by_key(
            data["by_failure_authority"],
            "failure_authority",
            "implementation_bug",
        )
        self.assertEqual(generated["runs"], 1)
        self.assertEqual(implementation["runs"], 1)
        self.assertEqual(data["overall"]["failure_authority"]["generated_test_bug"], 1)
        self.assertEqual(data["overall"]["failure_authority"]["implementation_bug"], 1)


if __name__ == "__main__":
    unittest.main()
