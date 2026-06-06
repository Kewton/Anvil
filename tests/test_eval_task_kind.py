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
    worker_lifecycle: dict[str, object] | None = None,
    extra_edits: list[str] | None = None,
    terminal_diagnostics: dict[str, object] | None = None,
    verify_commands: list[str] | None = None,
    failure_observation: dict[str, object] | None = None,
) -> None:
    messages: list[dict[str, object]] = [
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
    ]
    # Issue #976: extra Edit turns let a test exercise repeated repair targets.
    for edit_path in extra_edits or []:
        messages.append(
            {
                "role": "assistant",
                "content": "edit",
                "tool_calls": [
                    {"name": "Edit", "arguments": {"path": edit_path}},
                ],
            }
        )
    session: dict[str, object] = {
        "id": f"{task_kind}-{pam_variant}",
        "messages": messages,
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
    if terminal_diagnostics is not None:
        merged = dict(eval_record.get("terminal_diagnostics") or {})
        merged.update(terminal_diagnostics)
        eval_record["terminal_diagnostics"] = merged
    if verify_commands is not None:
        eval_record["verify_commands"] = verify_commands
    if failure_observation is not None:
        eval_record["failure_observation"] = failure_observation
    if recovery_strategy_count is not None:
        eval_record["recovery_strategy_count"] = recovery_strategy_count
    if recovery_strategies is not None:
        eval_record["recovery_strategies"] = recovery_strategies
    if worker_lifecycle is not None:
        eval_record["worker_lifecycle"] = worker_lifecycle
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
                "EvidenceBindingFailedJob",
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

    def test_report_aggregates_success_terminal_failure_class_for_non_coding_set(
        self,
    ) -> None:
        # Issue #1008: the expanded non-coding evaluation set
        # (benchmarks/non-coding-lifecycle.yaml) runs on the shared lifecycle.
        # Its per-TaskKind success rate, terminal state, and failure class must
        # aggregate for every non-coding kind -- including `authoring`, which the
        # existing by_task_kind / by_failure_authority tests do not cover.
        with tempfile.TemporaryDirectory() as raw:
            bench_root = pathlib.Path(raw) / "bench-root"
            passing = [
                ("docs", "docs/configuration-reference.md"),
                ("data", "output/event-summary.csv"),
                ("research", "research/logging-strategy-brief.md"),
                ("ops", "runbooks/local-agent-triage.md"),
            ]
            for task_kind, modified_path in passing:
                _make_run(
                    bench_root / "qwen3" / task_kind / "pam_off" / "run-1",
                    task_kind=task_kind,
                    pam_variant="pam_off",
                    modified_path=modified_path,
                    final_outcome="done",
                )
            # One failing authoring run populates the failure-class dimension with
            # a neutral (non-coding) terminal state and authority.
            _make_run(
                bench_root / "qwen3" / "authoring" / "pam_off" / "run-1",
                task_kind="authoring",
                pam_variant="pam_off",
                modified_path="docs/agent-loop-onboarding.md",
                rc=1,
                final_outcome="evidence_failed",
                failure_authority="artifact_classification",
            )

            data = json.loads(
                subprocess.run(
                    [sys.executable, str(REPORT), "--format", "json", str(bench_root)],
                    capture_output=True,
                    text=True,
                    cwd=str(REPO_ROOT),
                    check=True,
                ).stdout
            )

        def by_keys(items: list[dict[str, object]], **criteria: str) -> dict[str, object]:
            for item in items:
                if all(item.get(key) == value for key, value in criteria.items()):
                    return item
            raise AssertionError(f"missing {criteria}: {items}")

        # Every non-coding kind is aggregated with a success-rate projection.
        kinds = {item["task_kind"] for item in data["by_task_kind"]}
        self.assertEqual(kinds, {"docs", "data", "research", "ops", "authoring"})
        authoring = by_keys(data["by_task_kind"], task_kind="authoring")
        self.assertEqual(authoring["runs"], 1)
        self.assertEqual(authoring["terminal_success"]["ok"], 0)
        self.assertEqual(authoring["terminal_success"]["total"], 1)

        # Terminal-state aggregation separates the failing authoring run from the
        # four completed runs (no coding vocabulary is projected onto it).
        completed = by_keys(
            data["by_generic_terminal_state"], generic_terminal_state="completed"
        )
        self.assertEqual(completed["runs"], 4)
        failed = by_keys(
            data["by_generic_terminal_state"], generic_terminal_state="evidence_failed"
        )
        self.assertEqual(failed["runs"], 1)

        # Failure-class distribution carries the authoring failure.
        artifact_cls = by_keys(
            data["by_failure_authority"], failure_authority="artifact_classification"
        )
        self.assertEqual(artifact_cls["runs"], 1)

    def test_analyze_deterministic_postcheck_for_non_coding_fixture_artifacts(
        self,
    ) -> None:
        # Issue #1008 / AC3: the per-kind postcheck checker is deterministic and
        # passes the fixture-shaped artifacts for docs/data/research/ops/authoring
        # with no LLM judge. Each kind's artifact resolves to a stable reason.
        cases = [
            ("docs", "docs/configuration-reference.md", "docs_artifact"),
            ("data", "output/event-summary.csv", "data_artifact"),
            ("research", "research/logging-strategy-brief.md", "research_artifact"),
            ("ops", "runbooks/local-agent-triage.md", "ops_artifact"),
            ("authoring", "docs/agent-loop-onboarding.md", "authoring_artifact"),
        ]
        with tempfile.TemporaryDirectory() as raw:
            root = pathlib.Path(raw)
            for idx, (task_kind, artifact, reason) in enumerate(cases, start=1):
                run_dir = root / f"pass-{idx}"
                _make_run(
                    run_dir,
                    task_kind=task_kind,
                    pam_variant="pam_off",
                    modified_path=artifact,
                    final_outcome="done",
                )
                first = self._analyze(run_dir)
                self.assertTrue(
                    first["postcheck_success"],
                    f"{task_kind} artifact {artifact} should pass deterministically",
                )
                self.assertEqual(first["postcheck_reason"], reason)
                # Determinism: a second analysis yields the identical verdict.
                second = self._analyze(run_dir)
                self.assertEqual(
                    first["postcheck_success"], second["postcheck_success"]
                )
                self.assertEqual(first["postcheck_reason"], second["postcheck_reason"])

            # Negative: a non-docs artifact fails the authoring checker (proving
            # the checker discriminates rather than always passing).
            neg_dir = root / "authoring-neg"
            _make_run(
                neg_dir,
                task_kind="authoring",
                pam_variant="pam_off",
                modified_path="src/lib.rs",
                final_outcome="done",
            )
            neg = self._analyze(neg_dir)
            self.assertFalse(neg["postcheck_success"])
            self.assertEqual(neg["postcheck_reason"], "missing_authoring_artifact")

    def test_analyze_emits_worker_lifecycle_metrics_from_eval_log(self) -> None:
        with tempfile.TemporaryDirectory() as raw:
            run_dir = pathlib.Path(raw) / "run-1"
            _make_run(
                run_dir,
                task_kind="coding",
                pam_variant="pam_off",
                modified_path="tests/test_sales_cli.py",
                final_outcome="missing_evidence",
                worker_lifecycle={
                    "worker_kind": "test_author",
                    "context_pack_kind": "evidence",
                    "context_token_estimate": 384,
                    "context_entry_count": 5,
                    "deliverable_created": True,
                    "evidence_created": False,
                    "runner_bound": False,
                    "diagnostic_class": "missing_test",
                    "diagnostic_classified": True,
                    "repair_applied": False,
                    "rerun_passed": False,
                },
            )
            data = self._analyze(run_dir)

        self.assertEqual(data["worker_kind"], "test_author")
        self.assertEqual(data["context_pack_kind"], "evidence")
        self.assertEqual(data["context_token_estimate"], 384)
        self.assertEqual(data["context_entry_count"], 5)
        self.assertTrue(data["deliverable_created"])
        self.assertFalse(data["evidence_created"])
        self.assertFalse(data["runner_bound"])
        self.assertEqual(data["diagnostic_class"], "missing_test")
        self.assertTrue(data["diagnostic_classified"])
        self.assertFalse(data["repair_applied"])
        self.assertFalse(data["rerun_passed"])
        self.assertEqual(data["lifecycle_failure_stage"], "evidence_authoring")

    def test_bound_runner_failure_refines_missing_evidence_to_evidence_failed(
        self,
    ) -> None:
        with tempfile.TemporaryDirectory() as raw:
            run_dir = pathlib.Path(raw) / "run-1"
            _make_run(
                run_dir,
                task_kind="coding",
                pam_variant="pam_off",
                modified_path="src/lib.rs",
                final_outcome="missing_evidence",
                worker_lifecycle={
                    "worker_kind": "diagnostic_repair",
                    "context_pack_kind": "diagnostic",
                    "deliverable_created": True,
                    "evidence_created": True,
                    "runner_bound": True,
                    "diagnostic_class": "compile_error",
                    "diagnostic_classified": True,
                    "repair_applied": False,
                    "rerun_passed": False,
                },
            )
            data = self._analyze(run_dir)

        self.assertEqual(data["legacy_terminal_state"], "missing_evidence")
        self.assertEqual(data["generic_terminal_state"], "evidence_failed")
        self.assertEqual(data["recovery_job_kind"], "EvidenceFailedJob")
        self.assertEqual(data["lifecycle_failure_stage"], "rerun")

    def test_report_worker_lifecycle_summary_and_json_groupings(self) -> None:
        with tempfile.TemporaryDirectory() as raw:
            bench_root = pathlib.Path(raw) / "bench-root"
            _make_run(
                bench_root / "qwen3" / "coding" / "pam_off" / "run-1",
                task_kind="coding",
                pam_variant="pam_off",
                modified_path="tests/test_sales_cli.py",
                final_outcome="missing_evidence",
                worker_lifecycle={
                    "worker_kind": "test_author",
                    "context_pack_kind": "evidence",
                    "deliverable_created": True,
                    "evidence_created": False,
                    "runner_bound": False,
                    "diagnostic_classified": True,
                    "repair_applied": False,
                    "rerun_passed": False,
                },
            )
            _make_run(
                bench_root / "qwen3" / "docs" / "pam_on" / "run-1",
                task_kind="docs",
                pam_variant="pam_on",
                modified_path="README.md",
                final_outcome="done",
                worker_lifecycle={
                    "worker_kind": "docs",
                    "context_pack_kind": "contract",
                    "deliverable_created": True,
                    "evidence_created": True,
                    "runner_bound": True,
                    "diagnostic_classified": True,
                    "repair_applied": False,
                    "rerun_passed": True,
                },
            )

            markdown = subprocess.run(
                [sys.executable, str(REPORT), str(bench_root)],
                capture_output=True,
                text=True,
                cwd=str(REPO_ROOT),
                check=True,
            ).stdout
            self.assertIn("## Worker Lifecycle Summary", markdown)
            self.assertIn(
                "| test_author | evidence | evidence_authoring | MissingEvidenceJob | 1 | 100% (1/1) | 100% (1/1) | 100% (1/1) | 1 | 0 | 0 | 0 |",
                markdown,
            )
            self.assertIn(
                "| docs | contract | completed | none | 1 | 100% (1/1) | 100% (1/1) | 100% (1/1) | 1 | 0 | 0 | 0 |",
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

        by_keys(data["by_worker_kind"], worker_kind="test_author")
        by_keys(data["by_context_pack_kind"], context_pack_kind="evidence")
        by_keys(
            data["by_lifecycle_failure_stage"],
            lifecycle_failure_stage="evidence_authoring",
        )
        by_keys(
            data["by_worker_lifecycle"],
            worker_kind="docs",
            context_pack_kind="contract",
            lifecycle_failure_stage="completed",
            recovery_job_kind="none",
        )

    def test_non_coding_worker_lifecycle_fixtures_cover_general_purpose_kinds(
        self,
    ) -> None:
        fixtures = [
            ("docs", "README.md", "docs", "contract"),
            ("data", "output/users.csv", "data", "evidence"),
            ("research", "research/report.md", "research", "evidence"),
            ("ops", "scripts/cleanup.sh", "ops", "evidence"),
            ("authoring", "drafts/lesson.md", "authoring", "contract"),
        ]
        with tempfile.TemporaryDirectory() as raw:
            bench_root = pathlib.Path(raw) / "bench-root"
            for task_kind, modified_path, worker_kind, context_pack_kind in fixtures:
                run_dir = bench_root / "qwen3" / task_kind / "pam_off" / "run-1"
                _make_run(
                    run_dir,
                    task_kind=task_kind,
                    pam_variant="pam_off",
                    modified_path=modified_path,
                    final_outcome="done",
                    worker_lifecycle={
                        "worker_kind": worker_kind,
                        "context_pack_kind": context_pack_kind,
                        "context_token_estimate": 256,
                        "context_entry_count": 4,
                        "deliverable_created": True,
                        "evidence_created": True,
                        "runner_bound": True,
                        "diagnostic_class": "none",
                        "diagnostic_classified": True,
                        "repair_applied": False,
                        "rerun_passed": True,
                    },
                )
                analyzed = self._analyze(run_dir)
                self.assertEqual(analyzed["task_kind"], task_kind)
                self.assertEqual(analyzed["worker_kind"], worker_kind)
                self.assertEqual(analyzed["context_pack_kind"], context_pack_kind)
                self.assertEqual(analyzed["lifecycle_failure_stage"], "completed")

            json_result = subprocess.run(
                [sys.executable, str(REPORT), "--format", "json", str(bench_root)],
                capture_output=True,
                text=True,
                cwd=str(REPO_ROOT),
                check=True,
            )
            data = json.loads(json_result.stdout)

        lifecycle_rows = data["by_worker_lifecycle"]
        for task_kind, _modified_path, worker_kind, context_pack_kind in fixtures:
            self.assertTrue(
                any(
                    row.get("worker_kind") == worker_kind
                    and row.get("context_pack_kind") == context_pack_kind
                    and row.get("lifecycle_failure_stage") == "completed"
                    and row.get("recovery_job_kind") == "none"
                    for row in lifecycle_rows
                ),
                task_kind,
            )

    def test_v062_successor_regression_matrix_has_worker_lifecycle_expectations(self) -> None:
        cases = [
            (
                "python_sales_cli_missing_tests",
                "tests/test_sales_cli.py",
                "missing_evidence",
                {
                    "worker_kind": "test_author",
                    "context_pack_kind": "evidence",
                    "deliverable_created": True,
                    "evidence_created": False,
                    "runner_bound": False,
                    "diagnostic_class": "missing_test",
                    "diagnostic_classified": True,
                    "repair_applied": False,
                    "rerun_passed": False,
                },
                "evidence_authoring",
            ),
            (
                "node_json_formatter_missing_tests",
                "tests/main.test.js",
                "missing_evidence",
                {
                    "worker_kind": "test_author",
                    "context_pack_kind": "evidence",
                    "deliverable_created": True,
                    "evidence_created": False,
                    "runner_bound": False,
                    "diagnostic_class": "missing_test",
                    "diagnostic_classified": True,
                    "repair_applied": False,
                    "rerun_passed": False,
                },
                "evidence_authoring",
            ),
            (
                "node_csv_to_json_package_only_partial_state",
                "package.json",
                "missing_deliverable",
                {
                    "worker_kind": "implement",
                    "context_pack_kind": "target",
                    "deliverable_created": False,
                    "evidence_created": False,
                    "runner_bound": False,
                    "diagnostic_class": "partial_scaffold",
                    "diagnostic_classified": True,
                    "repair_applied": False,
                    "rerun_passed": False,
                },
                "deliverable",
            ),
            (
                "rust_slug_compile_failure",
                "src/lib.rs",
                "evidence_failed",
                {
                    "worker_kind": "diagnostic_repair",
                    "context_pack_kind": "diagnostic",
                    "deliverable_created": True,
                    "evidence_created": True,
                    "runner_bound": True,
                    "diagnostic_class": "compile_error",
                    "diagnostic_classified": True,
                    "repair_applied": False,
                    "rerun_passed": False,
                },
                "rerun",
            ),
            (
                "rust_ndjson_trivial_compile_repair",
                "tests/ndjson.rs",
                "done",
                {
                    "worker_kind": "diagnostic_repair",
                    "context_pack_kind": "repair",
                    "deliverable_created": True,
                    "evidence_created": True,
                    "runner_bound": True,
                    "diagnostic_class": "compile_error",
                    "diagnostic_classified": True,
                    "repair_applied": True,
                    "rerun_passed": True,
                },
                "completed",
            ),
        ]

        with tempfile.TemporaryDirectory() as raw:
            root = pathlib.Path(raw)
            analyzed: list[dict[str, object]] = []
            for idx, (
                case_name,
                modified_path,
                final_outcome,
                worker_lifecycle,
                expected_stage,
            ) in enumerate(cases, start=1):
                run_dir = root / f"run-{idx}"
                _make_run(
                    run_dir,
                    task_kind="coding",
                    pam_variant="pam_off",
                    modified_path=modified_path,
                    final_outcome=final_outcome,
                    worker_lifecycle=worker_lifecycle,
                )
                data = self._analyze(run_dir)
                analyzed.append(data)
                self.assertEqual(data["case"], "coding-case")
                self.assertEqual(
                    data["lifecycle_failure_stage"],
                    expected_stage,
                    case_name,
                )

        legacy_states = [row["legacy_terminal_state"] for row in analyzed]
        self.assertNotIn("missing_verification", legacy_states)
        self.assertEqual(
            sum(row["generic_terminal_state"] == "missing_evidence" for row in analyzed),
            2,
        )
        for row in analyzed:
            if row.get("runner_bound") is True and row.get("diagnostic_class") == "compile_error":
                self.assertNotEqual(
                    row["legacy_terminal_state"],
                    "safe_stop_verifier_missing",
                )
                self.assertNotEqual(
                    row["generic_terminal_state"],
                    "evidence_runner_missing",
                )

    def test_evidence_binding_failure_fixtures_cover_node_docs_data_research(
        self,
    ) -> None:
        """Issue #993 (parent #988, Issue E): a deliverable that exists but
        cannot bind its evidence runner is an ``evidence_binding_failed``
        transition, not a generic ``missing_evidence`` (legacy
        ``missing_verification``) terminal. The runtime-specific binding (Node
        manifest / docs document / data schema / research citation) all share
        the same generic lifecycle, recovery job, and runner-binding stage.
        """
        # task_kind, evidence deliverable path, binding-failure description.
        binding_fixtures = [
            ("coding", "tests/main.test.js"),  # no package.json -> runner unbound
            ("docs", "README.md"),  # no target document for content check
            ("data", "output/users.csv"),  # no output for schema check
            ("research", "research/report.md"),  # no source notes for citation check
        ]
        with tempfile.TemporaryDirectory() as raw:
            root = pathlib.Path(raw)
            for idx, (task_kind, modified_path) in enumerate(binding_fixtures, start=1):
                run_dir = root / f"binding-{idx}"
                _make_run(
                    run_dir,
                    task_kind=task_kind,
                    pam_variant="pam_off",
                    modified_path=modified_path,
                    rc=1,
                    final_outcome="missing_evidence",
                    worker_lifecycle={
                        "worker_kind": "evidence_binding",
                        "context_pack_kind": "evidence",
                        # The evidence deliverable was authored ...
                        "deliverable_created": True,
                        "evidence_created": True,
                        # ... but no evidence runner could be bound to it.
                        "runner_bound": False,
                        "diagnostic_classified": True,
                        "repair_applied": False,
                        "rerun_passed": False,
                    },
                )
                data = self._analyze(run_dir)
                self.assertEqual(data["task_kind"], task_kind)
                # Legacy wire label is preserved for compatibility consumers.
                self.assertEqual(
                    data["legacy_terminal_state"], "missing_evidence", task_kind
                )
                # ... but the generic lifecycle is the binding-failure transition.
                self.assertEqual(
                    data["generic_terminal_state"],
                    "evidence_binding_failed",
                    task_kind,
                )
                self.assertEqual(
                    data["recovery_job_kind"],
                    "EvidenceBindingFailedJob",
                    task_kind,
                )
                self.assertEqual(
                    data["lifecycle_failure_stage"], "runner_binding", task_kind
                )

    def test_missing_evidence_without_deliverable_is_not_binding_failure(self) -> None:
        """Contrast to the binding fixtures: when the evidence deliverable was
        never authored (``evidence_created=False``), the run stays a generic
        ``missing_evidence`` terminal. Only the binding-order case (deliverable
        present, runner unbound) migrates off ``missing_verification``."""
        with tempfile.TemporaryDirectory() as raw:
            run_dir = pathlib.Path(raw) / "run-1"
            _make_run(
                run_dir,
                task_kind="coding",
                pam_variant="pam_off",
                modified_path="src/lib.rs",
                rc=1,
                final_outcome="missing_evidence",
                worker_lifecycle={
                    "worker_kind": "test_author",
                    "context_pack_kind": "evidence",
                    "deliverable_created": True,
                    "evidence_created": False,
                    "runner_bound": False,
                    "diagnostic_classified": True,
                    "repair_applied": False,
                    "rerun_passed": False,
                },
            )
            data = self._analyze(run_dir)
        self.assertEqual(data["generic_terminal_state"], "missing_evidence")
        self.assertEqual(data["recovery_job_kind"], "MissingEvidenceJob")
        self.assertEqual(data["lifecycle_failure_stage"], "evidence_authoring")

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


class TestFailureObservationClassifier(unittest.TestCase):
    """Issue #976 (parent #974, Issue B): FailureObservation replay classifier
    and transition metrics."""

    def _analyze(self, run_dir: pathlib.Path) -> dict:
        result = subprocess.run(
            [sys.executable, str(ANALYZE), str(run_dir)],
            capture_output=True,
            text=True,
            cwd=str(REPO_ROOT),
            check=True,
        )
        return json.loads(result.stdout)

    def _observe(self, run_dir: pathlib.Path) -> dict:
        return self._analyze(run_dir)["failure_observation"]

    def _report_json(self, bench_root: pathlib.Path) -> dict:
        result = subprocess.run(
            [sys.executable, str(REPORT), "--format", "json", str(bench_root)],
            capture_output=True,
            text=True,
            cwd=str(REPO_ROOT),
            check=True,
        )
        return json.loads(result.stdout)

    def test_success_emits_neutral_observation(self) -> None:
        with tempfile.TemporaryDirectory() as raw:
            run_dir = pathlib.Path(raw) / "run-1"
            _make_run(
                run_dir,
                task_kind="coding",
                pam_variant="pam_off",
                modified_path="src/main.rs",
                final_outcome="done",
            )
            fo = self._observe(run_dir)
        self.assertEqual(fo["failure_class"], "none")
        self.assertEqual(fo["target_role"], "none")
        self.assertEqual(fo["terminal_state"], "completed")
        self.assertFalse(fo["runner_present_but_failed"])
        self.assertFalse(fo["wrong_target_repair"])
        self.assertFalse(fo["tool_protocol_error"])

    def test_runner_present_but_failed_under_verifier_missing(self) -> None:
        """Acceptance: a `safe_stop_verifier_missing` run whose runner actually
        ran and failed is detected and reclassified to evidence_failed."""
        with tempfile.TemporaryDirectory() as raw:
            run_dir = pathlib.Path(raw) / "run-1"
            _make_run(
                run_dir,
                task_kind="coding",
                pam_variant="pam_off",
                modified_path="src/lib.rs",
                rc=1,
                final_outcome="safe_stop_verifier_missing",
                terminal_diagnostics={
                    "verifier_status": "failed",
                    "last_failure_signature": "error[E0432]: unresolved import",
                },
                anvil_score={"build_passed": False, "compile_error_count": 3},
            )
            data = self._analyze(run_dir)
        fo = data["failure_observation"]
        # legacy + generic terminal states are both observable (acceptance).
        self.assertEqual(data["legacy_terminal_state"], "safe_stop_verifier_missing")
        self.assertEqual(data["generic_terminal_state"], "evidence_runner_missing")
        self.assertEqual(fo["terminal_state"], "evidence_runner_missing")
        self.assertTrue(fo["runner_present_but_failed"])
        self.assertEqual(fo["failure_class"], "evidence_failed")
        self.assertTrue(fo["evidence_runner_executed"])

    def test_repair_exhausted_should_target_test_or_setup(self) -> None:
        """Acceptance: a `repair_exhausted` run that repeatedly edited the
        implementation while the failure is a test import is detected."""
        with tempfile.TemporaryDirectory() as raw:
            run_dir = pathlib.Path(raw) / "run-1"
            _make_run(
                run_dir,
                task_kind="coding",
                pam_variant="pam_off",
                modified_path="src/main.py",
                rc=1,
                final_outcome="repair_exhausted",
                extra_edits=["src/main.py"],  # repeated impl target
                terminal_diagnostics={
                    "last_failure_signature": (
                        "ModuleNotFoundError: No module named 'tests.helpers'"
                    ),
                },
                anvil_score={
                    "implementation_files_changed": 2,
                    "test_files_changed": 0,
                    "setup_files_changed": 0,
                    "test_failure_count": 1,
                    "consecutive_no_progress_turns": 3,
                },
            )
            fo = self._observe(run_dir)
        self.assertEqual(fo["terminal_state"], "evidence_repair_exhausted")
        self.assertEqual(fo["failure_class"], "recovery_exhausted")
        self.assertTrue(fo["repair_should_target_test_or_setup"])
        self.assertTrue(fo["wrong_target_repair"])
        self.assertEqual(fo["target_role"], "test_or_setup")
        self.assertEqual(fo["repeated_targets"], ["src/main.py"])
        self.assertTrue(fo["same_diagnostic_repeated"])

    def test_tool_protocol_failure_classified(self) -> None:
        with tempfile.TemporaryDirectory() as raw:
            run_dir = pathlib.Path(raw) / "run-1"
            _make_run(
                run_dir,
                task_kind="coding",
                pam_variant="pam_off",
                modified_path="src/main.rs",
                rc=1,
                final_outcome="tool_call_format_error",
            )
            fo = self._observe(run_dir)
        self.assertEqual(fo["terminal_state"], "model_output_failure")
        self.assertEqual(fo["failure_class"], "tool_protocol_failure")
        self.assertTrue(fo["tool_protocol_error"])
        self.assertEqual(fo["target_role"], "tool_protocol")
        self.assertFalse(fo["evidence_runner_executed"])

    def test_missing_evidence_and_deliverable_classes(self) -> None:
        with tempfile.TemporaryDirectory() as raw:
            ev_dir = pathlib.Path(raw) / "ev"
            _make_run(
                ev_dir,
                task_kind="coding",
                pam_variant="pam_off",
                modified_path="src/main.rs",
                rc=1,
                final_outcome="missing_evidence",
            )
            ev = self._observe(ev_dir)
            del_dir = pathlib.Path(raw) / "del"
            _make_run(
                del_dir,
                task_kind="coding",
                pam_variant="pam_off",
                modified_path="src/main.rs",
                rc=1,
                final_outcome="missing_deliverable",
            )
            de = self._observe(del_dir)
        self.assertEqual(ev["failure_class"], "missing_evidence")
        self.assertTrue(ev["missing_evidence"])
        self.assertEqual(ev["target_role"], "evidence")
        self.assertEqual(de["failure_class"], "missing_deliverable")
        self.assertTrue(de["missing_deliverable"])
        self.assertEqual(de["target_role"], "deliverable")

    def test_failure_observation_override_block_surfaced(self) -> None:
        """Fields the eval log does not derive itself (exit code, excerpts,
        invalid-proposal/repair counts, operator hit) ride an optional
        `failure_observation` block and override the defaults."""
        with tempfile.TemporaryDirectory() as raw:
            run_dir = pathlib.Path(raw) / "run-1"
            _make_run(
                run_dir,
                task_kind="coding",
                pam_variant="pam_off",
                modified_path="src/main.rs",
                rc=1,
                final_outcome="evidence_failed",
                verify_commands=["pytest -q"],
                failure_observation={
                    "evidence_command": "pytest -q tests/",
                    "evidence_exit_code": 1,
                    "stdout_excerpt": "1 failed",
                    "stderr_excerpt": "AssertionError",
                    "invalid_proposal_count": 2,
                    "repair_count": 4,
                    "deterministic_operator_hit": True,
                },
            )
            fo = self._observe(run_dir)
        self.assertEqual(fo["evidence_command"], "pytest -q tests/")
        self.assertEqual(fo["evidence_exit_code"], 1)
        self.assertEqual(fo["stdout_excerpt"], "1 failed")
        self.assertEqual(fo["stderr_excerpt"], "AssertionError")
        self.assertEqual(fo["invalid_proposal_count"], 2)
        self.assertEqual(fo["repair_count"], 4)
        self.assertTrue(fo["deterministic_operator_hit"])
        self.assertTrue(fo["evidence_runner_executed"])

    def test_deterministic_operator_hit_from_recovery_strategies(self) -> None:
        with tempfile.TemporaryDirectory() as raw:
            run_dir = pathlib.Path(raw) / "run-1"
            _make_run(
                run_dir,
                task_kind="coding",
                pam_variant="pam_off",
                modified_path="src/main.rs",
                rc=1,
                final_outcome="evidence_failed",
                recovery_strategies=["deterministic_compile_repair"],
            )
            fo = self._observe(run_dir)
        self.assertTrue(fo["deterministic_operator_hit"])

    def test_deterministic_operator_hit_from_rust_binding_repair(self) -> None:
        # Issue #991: the Rust binding-repair controller strategy
        # (`deterministic_binding_repair`) must surface in the eval/report
        # deterministic-operator hit rate.
        with tempfile.TemporaryDirectory() as raw:
            run_dir = pathlib.Path(raw) / "run-1"
            _make_run(
                run_dir,
                task_kind="coding",
                pam_variant="pam_off",
                modified_path="Cargo.toml",
                rc=1,
                final_outcome="evidence_failed",
                recovery_strategies=["deterministic_binding_repair"],
            )
            fo = self._observe(run_dir)
        self.assertTrue(fo["deterministic_operator_hit"])

    def test_report_transition_metrics_aggregate(self) -> None:
        with tempfile.TemporaryDirectory() as raw:
            bench_root = pathlib.Path(raw) / "bench-root"
            base = bench_root / "qwen3" / "coding" / "pam_off"
            _make_run(
                base / "run-1",
                task_kind="coding",
                pam_variant="pam_off",
                modified_path="src/main.rs",
                final_outcome="done",
                anvil_score={"tests_passed": True},
            )
            _make_run(
                base / "run-2",
                task_kind="coding",
                pam_variant="pam_off",
                modified_path="src/lib.rs",
                rc=1,
                final_outcome="safe_stop_verifier_missing",
                terminal_diagnostics={
                    "verifier_status": "failed",
                    "last_failure_signature": "error[E0432]: unresolved import",
                },
                anvil_score={"build_passed": False, "compile_error_count": 1},
            )
            _make_run(
                base / "run-3",
                task_kind="coding",
                pam_variant="pam_off",
                modified_path="src/main.py",
                rc=1,
                final_outcome="repair_exhausted",
                extra_edits=["src/main.py"],
                terminal_diagnostics={
                    "last_failure_signature": (
                        "ModuleNotFoundError: No module named 'tests.helpers'"
                    ),
                },
                anvil_score={
                    "implementation_files_changed": 2,
                    "test_files_changed": 0,
                    "setup_files_changed": 0,
                    "test_failure_count": 1,
                    "consecutive_no_progress_turns": 3,
                },
            )
            _make_run(
                base / "run-4",
                task_kind="coding",
                pam_variant="pam_off",
                modified_path="src/main.rs",
                rc=1,
                final_outcome="tool_call_format_error",
            )
            metrics = self._report_json(bench_root)["transition_metrics"]
            markdown = subprocess.run(
                [sys.executable, str(REPORT), str(bench_root)],
                capture_output=True,
                text=True,
                cwd=str(REPO_ROOT),
                check=True,
            ).stdout

        self.assertEqual(metrics["runs"], 4)
        self.assertEqual(metrics["failure_class"]["none"], 1)
        self.assertEqual(metrics["failure_class"]["evidence_failed"], 1)
        self.assertEqual(metrics["failure_class"]["recovery_exhausted"], 1)
        self.assertEqual(metrics["failure_class"]["tool_protocol_failure"], 1)
        self.assertEqual(metrics["evidence_failed"], 1)
        self.assertEqual(metrics["recovery_exhausted"], 1)
        self.assertEqual(metrics["tool_protocol_failure"], 1)
        self.assertEqual(metrics["wrong_target_repair"], 1)
        self.assertEqual(metrics["same_diagnostic_repeated"], 1)
        self.assertEqual(metrics["runner_present_but_failed"], 1)
        self.assertEqual(metrics["repair_should_target_test_or_setup"], 1)
        self.assertEqual(metrics["evidence_runner_executed"]["ok"], 3)
        self.assertEqual(metrics["evidence_runner_executed"]["total"], 4)
        self.assertAlmostEqual(metrics["evidence_runner_executed"]["rate"], 0.75)
        self.assertEqual(metrics["deterministic_operator_hit"]["ok"], 0)

        self.assertIn("## Transition Metrics", markdown)
        self.assertIn("| evidence_failed | 1 |", markdown)
        self.assertIn("| evidence_runner_executed | 3/4 (75%) |", markdown)


if __name__ == "__main__":
    unittest.main()
