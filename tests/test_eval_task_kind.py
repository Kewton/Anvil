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


# Issue #925: sentinel meaning "classified == task_kind" (the common, R5-clean
# case). Pass an explicit kind to simulate a misroute, or None to simulate a
# missing classification (which R5 must fail closed for non-coding cases).
_MATCH_TASK_KIND = object()


def _make_run(
    run_dir: pathlib.Path,
    *,
    task_kind: str,
    pam_variant: str,
    modified_path: str,
    rc: int = 0,
    final_outcome: str | None = None,
    postcheck_success: bool | None = None,
    postcheck_reason: str | None = None,
    failure_authority: str | None = None,
    last_feedback_kind: str | None = None,
    anvil_score: dict[str, object] | None = None,
    classified_task_kind: object = _MATCH_TASK_KIND,
    recovery_strategy_count: int | None = None,
    recovery_strategies: list[str] | None = None,
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

    # Issue #925: surface the agent's classified kind via a top-level
    # `classified_task_kind` in logs/eval.jsonl (the field analyze_run.py reads
    # for R5). Default = match task_kind (no misroute). `None` => omit the file
    # entirely (simulates a missing classification).
    effective_classified = (
        task_kind
        if classified_task_kind is _MATCH_TASK_KIND
        else classified_task_kind
    )
    eval_record: dict[str, object] = {}
    if effective_classified is not None:
        eval_record["classified_task_kind"] = effective_classified
    if final_outcome is not None:
        eval_record["final_outcome"] = final_outcome
        eval_record["terminal_diagnostics"] = {"outcome": final_outcome}
    if recovery_strategy_count is not None:
        eval_record["recovery_strategy_count"] = recovery_strategy_count
    if recovery_strategies is not None:
        eval_record["recovery_strategies"] = recovery_strategies
    if eval_record:
        logs_dir = run_dir / "logs"
        logs_dir.mkdir(parents=True, exist_ok=True)
        (logs_dir / "eval.jsonl").write_text(
            json.dumps(eval_record) + "\n",
            encoding="utf-8",
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

    def test_analyze_emits_objective_projection_for_general_task_kinds(self) -> None:
        cases = [
            ("docs", "README.md", "document_sections", "content_check"),
            ("data", "output/users.csv", "output_file", "schema_check"),
            (
                "research",
                "research/report.md",
                "research_notes",
                "source_fetch_evidence",
            ),
            (
                "ops",
                "scripts/cleanup.sh",
                "command_observation",
                "safety_boundary_evidence",
            ),
            ("authoring", "drafts/lesson.md", "prose_artifact", "content_acceptance"),
            ("answer_only", "logs/eval.jsonl", "answer", "content_acceptance"),
        ]
        with tempfile.TemporaryDirectory() as raw:
            root = pathlib.Path(raw)
            for idx, (task_kind, path, deliverable, evidence) in enumerate(cases, start=1):
                run_dir = root / f"run-{idx}"
                _make_run(
                    run_dir,
                    task_kind=task_kind,
                    pam_variant="pam_off",
                    modified_path=path,
                    final_outcome="done",
                )
                data = self._analyze(run_dir)
                self.assertEqual(data["task_kind"], task_kind)
                self.assertEqual(data["deliverable_kind"], deliverable)
                self.assertEqual(data["evidence_kind"], evidence)
                self.assertEqual(data["legacy_terminal_state"], "done")
                self.assertEqual(data["generic_terminal_state"], "completed")
                self.assertEqual(data["recovery_job_kind"], "none")

    def test_analyze_projects_v061_failure_modes_to_generic_recovery(self) -> None:
        cases = [
            ("missing_verification", "missing_evidence", "MissingEvidenceJob"),
            ("missing_evidence", "missing_evidence", "MissingEvidenceJob"),
            (
                "safe_stop_verifier_weak",
                "evidence_binding_failed",
                "EvidenceFailedJob",
            ),
            (
                "safe_stop_verifier_missing",
                "evidence_runner_missing",
                "ToolFailureJob",
            ),
            (
                "repair_exhausted",
                "evidence_repair_exhausted",
                "EvidenceFailedJob",
            ),
            ("missing_repo_edits", "missing_deliverable", "MissingDeliverableJob"),
        ]
        with tempfile.TemporaryDirectory() as raw:
            root = pathlib.Path(raw)
            for idx, (final_outcome, generic_state, recovery_job) in enumerate(
                cases, start=1
            ):
                run_dir = root / f"run-{idx}"
                _make_run(
                    run_dir,
                    task_kind="coding",
                    pam_variant="pam_off",
                    modified_path="src/lib.rs",
                    rc=1,
                    final_outcome=final_outcome,
                    recovery_strategy_count=3 if final_outcome == "repair_exhausted" else 0,
                    recovery_strategies=(
                        ["tool_first_retry", "targeted_artifact_retry", "evidence_action"]
                        if final_outcome == "repair_exhausted"
                        else []
                    ),
                )
                data = self._analyze(run_dir)
                self.assertEqual(data["legacy_terminal_state"], final_outcome)
                self.assertEqual(data["generic_terminal_state"], generic_state)
                self.assertEqual(data["recovery_job_kind"], recovery_job)
                if final_outcome == "repair_exhausted":
                    self.assertEqual(data["recovery_strategy_count"], 3)
                    self.assertEqual(
                        data["recovery_strategies"],
                        [
                            "tool_first_retry",
                            "targeted_artifact_retry",
                            "evidence_action",
                        ],
                    )

    def test_report_objective_matrix_covers_non_coding_fixture_set(self) -> None:
        with tempfile.TemporaryDirectory() as raw:
            bench_root = pathlib.Path(raw) / "bench-root"
            fixtures = [
                ("docs", "pam_on", "README.md"),
                ("data", "pam_off", "output/users.csv"),
                ("research", "pam_off", "research/report.md"),
                ("ops", "pam_on", "scripts/cleanup.sh"),
                ("authoring", "pam_off", "drafts/lesson.md"),
                ("answer_only", "pam_on", "logs/eval.jsonl"),
            ]
            for task_kind, pam_variant, modified_path in fixtures:
                _make_run(
                    bench_root / "qwen3" / task_kind / pam_variant / "run-1",
                    task_kind=task_kind,
                    pam_variant=pam_variant,
                    modified_path=modified_path,
                    final_outcome="done",
                )

            markdown = subprocess.run(
                [sys.executable, str(REPORT), str(bench_root)],
                capture_output=True,
                text=True,
                cwd=str(REPO_ROOT),
                check=True,
            ).stdout
            self.assertIn("## Objective Matrix", markdown)
            self.assertIn("## Terminal State Summary", markdown)
            self.assertIn("## Recovery Job Summary", markdown)
            self.assertIn(
                "| docs | document_sections | content_check | completed | none | 1 | 100% (1/1) | 100% (1/1) | 100% (1/1) | 1 | 0 | 0 | 0 |",
                markdown,
            )
            self.assertIn(
                "| research | research_notes | source_fetch_evidence | completed | none | 1 | 100% (1/1) | 100% (1/1) | 100% (1/1) | 1 | 0 | 0 | 0 |",
                markdown,
            )

            json_result = subprocess.run(
                [sys.executable, str(REPORT), "--format", "json", str(bench_root)],
                capture_output=True,
                text=True,
                cwd=str(REPO_ROOT),
                check=True,
            )
            data = json.loads(json_result.stdout)

        def by_keys(items: list[dict[str, object]], **criteria: str) -> dict[str, object]:
            for item in items:
                if all(item.get(key) == value for key, value in criteria.items()):
                    return item
            raise AssertionError(f"missing {criteria}: {items}")

        by_keys(
            data["by_objective_matrix"],
            task_kind="ops",
            deliverable_kind="command_observation",
            evidence_kind="safety_boundary_evidence",
            generic_terminal_state="completed",
            recovery_job_kind="none",
        )
        answer = by_keys(
            data["by_objective_matrix"],
            task_kind="answer_only",
            deliverable_kind="answer",
            evidence_kind="content_acceptance",
        )
        self.assertEqual(answer["runs"], 1)
        completed = by_keys(
            data["by_generic_terminal_state"],
            generic_terminal_state="completed",
        )
        self.assertEqual(completed["terminal_success"]["ok"], 6)
        self.assertEqual(data["by_recovery_job_kind"][0]["recovery_job_kind"], "none")

    # ---- Issue #925 (P8): R5 misroute fail-closed gate ---------------------

    def _analyze(self, run_dir: pathlib.Path) -> dict:
        result = subprocess.run(
            [sys.executable, str(ANALYZE), str(run_dir)],
            capture_output=True,
            text=True,
            cwd=str(REPO_ROOT),
            check=True,
        )
        return json.loads(result.stdout)

    def test_r5_misroute_fails_noncoding_case(self) -> None:
        """A non-coding case whose classified kind differs from the expected
        kind is a misroute: postcheck is forced False even though the docs
        artifact is present."""
        with tempfile.TemporaryDirectory() as raw:
            run_dir = pathlib.Path(raw) / "run-1"
            _make_run(
                run_dir,
                task_kind="docs",
                pam_variant="pam_on",
                modified_path="README.md",
                classified_task_kind="coding",  # agent misrouted docs -> coding
            )
            data = self._analyze(run_dir)
        self.assertEqual(data["classified_task_kind"], "coding")
        self.assertTrue(data["task_kind_misroute"])
        self.assertFalse(data["postcheck_success"])
        self.assertEqual(data["postcheck_reason"], "task_kind_misroute")

    def test_r5_missing_classification_fails_noncoding_case(self) -> None:
        """A non-coding case with NO classified kind fails closed (default-to-
        pass is prohibited); the absent field is omitted from output."""
        with tempfile.TemporaryDirectory() as raw:
            run_dir = pathlib.Path(raw) / "run-1"
            _make_run(
                run_dir,
                task_kind="data",
                pam_variant="pam_off",
                modified_path="output/users.csv",
                classified_task_kind=None,  # no eval.jsonl classified field
            )
            data = self._analyze(run_dir)
        self.assertNotIn("classified_task_kind", data)
        self.assertTrue(data["task_kind_misroute"])
        self.assertFalse(data["postcheck_success"])
        self.assertEqual(data["postcheck_reason"], "missing_classification")

    def test_r5_match_passes_and_surfaces_classified(self) -> None:
        """When classified == expected, R5 does not fire: postcheck reflects the
        artifact, the classified kind is surfaced, and no misroute flag is set."""
        with tempfile.TemporaryDirectory() as raw:
            run_dir = pathlib.Path(raw) / "run-1"
            _make_run(
                run_dir,
                task_kind="data",
                pam_variant="pam_on",
                modified_path="output/users.csv",  # data artifact present
            )
            data = self._analyze(run_dir)
        self.assertEqual(data["classified_task_kind"], "data")
        self.assertNotIn("task_kind_misroute", data)
        self.assertTrue(data["postcheck_success"])
        self.assertEqual(data["postcheck_reason"], "data_artifact")

    def test_r5_bypasses_coding_case(self) -> None:
        """Coding cases bypass R5 entirely: even a divergent classified kind
        does not flip the verdict (the field is still surfaced)."""
        with tempfile.TemporaryDirectory() as raw:
            run_dir = pathlib.Path(raw) / "run-1"
            _make_run(
                run_dir,
                task_kind="coding",
                pam_variant="pam_off",
                modified_path="src/main.rs",  # coding artifact present
                classified_task_kind="docs",  # divergent, but coding bypasses R5
            )
            data = self._analyze(run_dir)
        self.assertEqual(data["classified_task_kind"], "docs")
        self.assertNotIn("task_kind_misroute", data)
        self.assertTrue(data["postcheck_success"])
        self.assertEqual(data["postcheck_reason"], "coding_artifact")

    def test_r5_corrupt_tail_after_valid_classified_fails_closed(self) -> None:
        """CB-001 regression: a malformed line AFTER a valid classified line must
        NOT let the earlier (stale) value pass. The reader fails closed, so a
        non-coding run is treated as a missing classification (fail-closed)."""
        with tempfile.TemporaryDirectory() as raw:
            run_dir = pathlib.Path(raw) / "run-1"
            _make_run(
                run_dir,
                task_kind="data",
                pam_variant="pam_on",
                modified_path="output/users.csv",  # valid classified=data line
            )
            # Append a malformed JSON line after the valid record.
            eval_path = run_dir / "logs" / "eval.jsonl"
            with eval_path.open("a", encoding="utf-8") as fh:
                fh.write("{not valid json\n")
            data = self._analyze(run_dir)
        self.assertNotIn("classified_task_kind", data)
        self.assertTrue(data["task_kind_misroute"])
        self.assertFalse(data["postcheck_success"])
        self.assertEqual(data["postcheck_reason"], "missing_classification")


if __name__ == "__main__":
    unittest.main()
