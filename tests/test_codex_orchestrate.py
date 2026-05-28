from __future__ import annotations

import importlib.util
import json
import sys
from pathlib import Path
from types import ModuleType

import pytest

SCRIPT_PATH = Path(__file__).resolve().parents[1] / "scripts" / "codex_orchestrate.py"


def load_script() -> ModuleType:
    spec = importlib.util.spec_from_file_location("codex_orchestrate", SCRIPT_PATH)
    assert spec is not None
    assert spec.loader is not None
    module = importlib.util.module_from_spec(spec)
    sys.modules[spec.name] = module
    spec.loader.exec_module(module)
    return module


def test_slugify_keeps_safe_branch_text() -> None:
    module = load_script()

    assert module.slugify("Add Codex Harness!") == "add-codex-harness"
    assert module.slugify("!!!") == "task"


def test_analyze_issue_extracts_acceptance_files_and_tests() -> None:
    module = load_script()
    issue = module.Issue(
        number=12,
        title="Add dry-run planner",
        body=(
            "Implement planner for `scripts/codex_orchestrate.py`.\n\n"
            "## Acceptance Criteria\n"
            "- `pytest -q` passes\n"
            "- writes `workspace/management/runs/example/manifest.md`\n"
        ),
        labels=("feature",),
    )

    analysis = module.analyze_issue(issue, "Anvil", skip_enhance=False)

    assert analysis.enhancement_needed is False
    assert analysis.branch_name == "feature/issue-12-add-dry-run-planner"
    assert "scripts/codex_orchestrate.py" in analysis.suspected_files
    assert "pytest" in analysis.test_expectations
    assert analysis.acceptance_criteria


def test_analyze_issue_skips_markdown_heading_for_objective() -> None:
    module = load_script()
    issue = module.Issue(
        number=2,
        title="[P0][M1] Define v1 sidecar schema",
        body=(
            "## 概要\n"
            "`workspace/v0.1.0/05_development_preparation_plan.md` の M1 Schema First に従い、"
            "v1 sidecar schema を実装する。\n\n"
            "## 完了条件\n"
            "- `schema_version` が request / response / event に必須である\n"
        ),
    )

    analysis = module.analyze_issue(issue, "Anvil", skip_enhance=False)

    assert analysis.objective.startswith("`workspace/v0.1.0/05_development_preparation_plan.md`")
    assert analysis.objective != "概要"


def test_extract_file_candidates_ignores_absolute_path_fragments() -> None:
    module = load_script()
    candidates = module.extract_file_candidates(
        "参照: `/Users/me/repo/external/scripts/export_agent_training_data.py`\n"
        "対象: `src/memory/sanitizer.py`\n"
    )

    assert "Users/me/repo/external/scripts/export_agent_training_data.py" not in candidates
    assert "src/memory/sanitizer.py" in candidates


def test_classify_file_candidates_splits_external_references() -> None:
    module = load_script()

    suspected, references = module.classify_file_candidates(
        [
            "photon-mlx-develop/scripts/export_agent_training_data.py",
            "src/memory/sanitizer.py",
        ]
    )

    assert suspected == ["src/memory/sanitizer.py"]
    assert references == ["photon-mlx-develop/scripts/export_agent_training_data.py"]


def test_issue_2_to_5_dependency_batches_are_not_fully_serial() -> None:
    module = load_script()
    issues = [
        module.Issue(2, "[P0][M1] Define v1 sidecar schema", "schema EventRecord"),
        module.Issue(3, "[P0][M3] Implement sanitizer module", "sanitizer redact secret token"),
        module.Issue(
            4, "[P0][M2] Implement local SQLite event store", "SQLite local-first storage"
        ),
        module.Issue(
            5, "[P0][M2] Implement sidecar health/events/suggest", "FastAPI sidecar endpoint client"
        ),
    ]
    analyses = [
        module.analyze_issue(issue, "Anvil", skip_enhance=True) for issue in issues
    ]

    batches, merge_order = module.classify_batches(analyses, "")

    assert batches[0] == [2, 3]
    assert batches[1:] == [[4], [5]]
    assert merge_order == [2, 3, 4, 5]


def test_build_issue_body_with_orchestration_notes_is_idempotent() -> None:
    module = load_script()
    issue = module.Issue(
        number=9,
        title="Clarify issue",
        body="Original body\n\n## 完了条件\n- Works\n",
    )
    analysis = module.analyze_issue(issue, "Anvil", skip_enhance=False)

    first = module.build_issue_body_with_orchestration_notes(analysis)
    second = module.build_issue_body_with_orchestration_notes(
        module.IssueAnalysis(
            issue=module.Issue(issue.number, issue.title, first, issue.labels),
            objective=analysis.objective,
            acceptance_criteria=analysis.acceptance_criteria,
            suspected_files=analysis.suspected_files,
            reference_files=analysis.reference_files,
            test_expectations=analysis.test_expectations,
            enhancement_needed=analysis.enhancement_needed,
            questions=analysis.questions,
            branch_name=analysis.branch_name,
            worktree_path=analysis.worktree_path,
            dependency_hints=analysis.dependency_hints,
        )
    )

    assert first == second
    assert first.count("<!-- codex-orchestrate-notes -->") == 1


def test_write_artifacts_from_fixture(tmp_path: Path) -> None:
    module = load_script()
    fixture = tmp_path / "issues.json"
    fixture.write_text(
        json.dumps(
            {
                "issues": [
                    {
                        "number": 1,
                        "title": "Update schema docs",
                        "body": "Touch `workspace/management/codex_harness_spec.md`.\n",
                        "labels": ["docs"],
                    },
                    {
                        "number": 2,
                        "title": "Add script tests",
                        "body": (
                            "Update `tests/test_codex_orchestrate.py`.\n\n"
                            "## Acceptance Criteria\n- pytest passes\n"
                        ),
                        "labels": ["test"],
                    },
                ]
            }
        ),
        encoding="utf-8",
    )

    args = module.parse_args_from_list_for_test(
        [
            "1",
            "2",
            "--dry-run",
            "--issue-json",
            str(fixture),
            "--run-id",
            "test-run",
            "--runs-dir",
            str(tmp_path / "runs"),
        ]
    )
    issues = module.load_issues(args.issues, args.issue_json)
    analyses = [
        module.analyze_issue(issue, "Anvil", skip_enhance=args.skip_enhance)
        for issue in issues
    ]
    run_dir = module.write_artifacts(args, analyses)

    assert (run_dir / "manifest.md").exists()
    assert (run_dir / "issue-analysis.md").exists()
    assert (run_dir / "dependency-plan.md").exists()
    assert "Issue #1" in (run_dir / "issue-analysis.md").read_text(encoding="utf-8")


def test_worktree_planning_does_not_mutate_in_dry_run() -> None:
    module = load_script()
    issue = module.Issue(
        number=3,
        title="Add worktree manager",
        body="Update `scripts/codex_orchestrate.py`.\n\n## Acceptance Criteria\n- dry-run only\n",
    )
    analysis = module.analyze_issue(issue, "Anvil", skip_enhance=False)

    results = module.create_or_reuse_worktrees([analysis], dry_run=True)

    assert results[0].status == "planned"
    assert results[0].branch_name == "feature/issue-3-add-worktree-manager"


def test_commandmate_send_command_omits_agent_by_default() -> None:
    module = load_script()

    cmd = module.build_commandmate_send_command(
        "repo-issue-1",
        "hello",
        duration="3h",
        codex_agent_name="",
    )

    assert "--agent" not in cmd
    assert cmd == [
        "commandmatedev",
        "send",
        "repo-issue-1",
        "hello",
        "--auto-yes",
        "--duration",
        "3h",
    ]


def test_dispatch_commandmate_sends_only_worker_task() -> None:
    module = load_script()
    issue = module.Issue(
        number=1,
        title="Add worker task",
        body="Implement the issue.\n\n## Acceptance Criteria\n- Done\n",
    )
    analysis = module.analyze_issue(issue, "Anvil", skip_enhance=True)
    worktree = module.WorktreeResult(
        issue_number=1,
        branch_name="feature/issue-1-add-worker-task",
        worktree_path=Path("/tmp/Anvil-issue-1-add-worker-task"),
        status="created",
        message="worktree created",
    )
    calls: list[list[str]] = []

    def fake_runner(args, **kwargs):  # type: ignore[no-untyped-def]
        calls.append(args)
        return module.subprocess.CompletedProcess(args, 0, "", "")

    results = module.dispatch_commandmate(
        [analysis],
        [worktree],
        dry_run=False,
        duration="3h",
        codex_agent_name="",
        poll=False,
        runner=fake_runner,
    )

    assert len(calls) == 1
    assert calls[0][:3] == [
        "commandmatedev",
        "send",
        "Anvil-feature-issue-1-add-worker-task",
    ]
    assert calls[0][3] != "hello"
    assert results[0].commands == (" ".join(calls[0]),)


def test_dispatch_commandmate_reports_send_failure_as_blocked() -> None:
    module = load_script()
    issue = module.Issue(
        number=1,
        title="Add worker task",
        body="Implement the issue.\n\n## Acceptance Criteria\n- Done\n",
    )
    analysis = module.analyze_issue(issue, "Anvil", skip_enhance=True)
    worktree = module.WorktreeResult(
        issue_number=1,
        branch_name="feature/issue-1-add-worker-task",
        worktree_path=Path("/tmp/Anvil-issue-1-add-worker-task"),
        status="created",
        message="worktree created",
    )

    def fake_runner(args, **kwargs):  # type: ignore[no-untyped-def]
        raise module.subprocess.CalledProcessError(
            99,
            args,
            output="",
            stderr="Error: Resource not found. Check the worktree ID.",
        )

    results = module.dispatch_commandmate(
        [analysis],
        [worktree],
        dry_run=False,
        duration="3h",
        codex_agent_name="",
        poll=False,
        runner=fake_runner,
    )

    assert results[0].status == "blocked"
    assert results[0].message == "Error: Resource not found. Check the worktree ID."


def test_commandmate_ls_command_omits_empty_branch_prefix() -> None:
    module = load_script()

    assert module.build_commandmate_ls_command(branch_prefix="feature/issue-") == [
        "commandmatedev",
        "ls",
        "--branch",
        "feature/issue-",
        "--json",
    ]
    assert module.build_commandmate_ls_command(branch_prefix=None) == [
        "commandmatedev",
        "ls",
        "--json",
    ]
    assert module.build_commandmate_ls_command(branch_prefix="") == [
        "commandmatedev",
        "ls",
        "--json",
    ]


def test_commandmate_worktree_id_uses_commandmate_branch_format() -> None:
    module = load_script()

    assert (
        module.commandmate_worktree_id("feature/issue-2-p0-m1-define-v1-sidecar-schema")
        == "Anvil-feature-issue-2-p0-m1-define-v1-sidecar-schema"
    )


def test_commandmate_repository_name_strips_issue_worktree_suffix(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    module = load_script()
    monkeypatch.setattr(
        module,
        "REPO_ROOT",
        Path("/tmp/Anvil-issue-2-p0-m1-define-v1-sidecar-schema"),
    )

    assert module.commandmate_repository_name() == "Anvil"


def test_commandmate_repository_name_strips_develop_worktree_suffix(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    module = load_script()
    monkeypatch.setattr(module, "REPO_ROOT", Path("/tmp/Anvil-develop"))

    assert module.commandmate_repository_name() == "Anvil"


def test_poll_worker_startup_reports_started_idle_without_prompting() -> None:
    module = load_script()
    calls: list[list[str]] = []

    def fake_runner(args, **kwargs):  # type: ignore[no-untyped-def]
        calls.append(args)
        if args == ["commandmatedev", "ls", "--json"]:
            return module.subprocess.CompletedProcess(
                args,
                0,
                json.dumps(
                    {
                        "worktrees": [
                            {
                                "id": "repo-issue-1",
                                "status": "running",
                                "isProcessing": False,
                            }
                        ]
                    }
                ),
                "",
            )
        return module.subprocess.CompletedProcess(args, 0, "", "")

    result = module.poll_worker_startup(
        1,
        "repo-issue-1",
        codex_agent_name="",
        commands=("hello", "task"),
        runner=fake_runner,
    )

    assert result.status == "started-but-idle"
    assert result.message == "worker session is running but not processing"
    assert result.running is True
    assert result.processing is False
    assert calls == [["commandmatedev", "ls", "--json"]]


def test_poll_worker_startup_reports_commandmate_unreachable() -> None:
    module = load_script()
    calls: list[list[str]] = []

    def fake_runner(args, **kwargs):  # type: ignore[no-untyped-def]
        calls.append(args)
        return module.subprocess.CompletedProcess(
            args,
            1,
            "",
            "Error: Server is not running. Start it with: commandmate start",
        )

    result = module.poll_worker_startup(
        1,
        "repo-issue-1",
        codex_agent_name="",
        commands=("hello", "task"),
        runner=fake_runner,
    )

    assert result.status == "blocked"
    assert result.message.startswith("commandmate-unreachable:")
    assert result.running is None
    assert result.processing is None
    assert calls == [["commandmatedev", "ls", "--json"]]


def test_wait_for_commandmate_workers_uses_wait_without_starting_server() -> None:
    module = load_script()
    calls: list[list[str]] = []
    sessions = [
        module.WorkerSessionResult(
            11,
            "repo-issue-11",
            "started-but-idle",
            False,
            True,
            "running",
            (),
        )
    ]

    def fake_runner(args, **kwargs):  # type: ignore[no-untyped-def]
        calls.append(args)
        return module.subprocess.CompletedProcess(args, 0, "completed\n", "")

    results = module.wait_for_commandmate_workers(
        sessions,
        timeout_seconds=600,
        stall_timeout_seconds=120,
        runner=fake_runner,
    )

    assert results[0].status == "completed"
    assert calls == [
        [
            "commandmatedev",
            "wait",
            "repo-issue-11",
            "--timeout",
            "600",
            "--stall-timeout",
            "120",
        ]
    ]


def test_wait_for_commandmate_workers_classifies_unreachable() -> None:
    module = load_script()
    sessions = [
        module.WorkerSessionResult(
            11,
            "repo-issue-11",
            "sent",
            None,
            None,
            "sent",
            (),
        )
    ]

    def fake_runner(args, **kwargs):  # type: ignore[no-untyped-def]
        return module.subprocess.CompletedProcess(
            args,
            1,
            "",
            "Error: Server is not running. Start it with: commandmate start",
        )

    results = module.wait_for_commandmate_workers(
        sessions,
        timeout_seconds=600,
        stall_timeout_seconds=0,
        runner=fake_runner,
    )

    assert results[0].status == "blocked"
    assert results[0].message.startswith("commandmate-unreachable:")


def test_render_uat_report_includes_manual_evidence() -> None:
    module = load_script()
    issue = module.Issue(
        number=4,
        title="GUI check",
        body="## Acceptance Criteria\n- Button is visible on device\n",
    )
    analysis = module.analyze_issue(issue, "Anvil", skip_enhance=False)

    report = module.render_uat_report([analysis])

    assert "Manual GUI / Real-device Checks" in report
    assert "screenshot" in report


def test_render_uat_fix_prompts_maps_failure_to_issue() -> None:
    module = load_script()
    issue = module.Issue(
        number=5,
        title="Fix GUI regression",
        body="## Acceptance Criteria\n- GUI works\n",
    )
    analysis = module.analyze_issue(issue, "Anvil", skip_enhance=False)
    failure = module.UatFailure(
        issue_number=5,
        scenario="Open settings",
        expected="Settings opens",
        actual="Blank screen",
        evidence="screenshot.png",
    )

    prompt = module.render_uat_fix_prompts([failure], [analysis])

    assert "UAT failure fix for Issue #5" in prompt
    assert "Blank screen" in prompt
    assert "feature/issue-5-fix-gui-regression" in prompt


def test_create_uat_fix_worktrees_dry_run() -> None:
    module = load_script()
    failure = module.UatFailure(5, "Open settings", "Settings opens", "Blank", "shot.png")

    results = module.create_uat_fix_worktrees([failure], dry_run=True)

    assert results[0].status == "planned"
    assert results[0].branch_name.startswith("fix/issue-5-uat-open-settings")


def test_photon_event_payload_redacts_paths_without_failing() -> None:
    module = load_script()

    result = module.emit_photon_event(
        "",
        event_kind="worker.blocked",
        run_id="run-1",
        payload={"path": "/Users/example/project/secret.txt"},
    )

    assert result.status == "skipped"
    assert module.sanitize_event_payload({"path": "/Users/example/project/secret.txt"}) == {
        "path": "[REDACTED_PATH]"
    }


def test_render_pr_body_contains_required_sections() -> None:
    module = load_script()
    issue = module.Issue(
        number=6,
        title="Add PR support",
        body=(
            "Update `scripts/codex_orchestrate.py`.\n\n"
            "## Acceptance Criteria\n- PR body lists tests\n"
        ),
    )
    analysis = module.analyze_issue(issue, "Anvil", skip_enhance=False)

    body = module.render_pr_body(analysis, "run-1")

    assert "Closes #6" in body
    assert "## Tests Run" in body
    assert "run-1" in body


def test_create_pull_requests_pushes_branch_before_develop_pr() -> None:
    module = load_script()
    issue = module.Issue(
        number=11,
        title="[P2] Add adapter",
        body="Update `src/models/photon_adapter.py`.",
    )
    analysis = module.analyze_issue(issue, "Anvil", skip_enhance=True)
    calls: list[list[str]] = []

    def fake_runner(args, **kwargs):  # type: ignore[no-untyped-def]
        calls.append(args)
        if args[:3] == ["git", "rev-list", "--count"]:
            return module.subprocess.CompletedProcess(args, 0, "1\n", "")
        if args[:3] == ["git", "push", "-u"]:
            return module.subprocess.CompletedProcess(args, 0, "", "")
        if args[:3] == ["gh", "pr", "list"]:
            return module.subprocess.CompletedProcess(args, 0, "[]\n", "")
        if args[:3] == ["gh", "pr", "create"]:
            return module.subprocess.CompletedProcess(
                args,
                0,
                "https://github.com/Kewton/Anvil/pull/42\n",
                "",
            )
        return module.subprocess.CompletedProcess(args, 1, "", "unexpected")

    results = module.create_pull_requests(
        [analysis],
        run_id="run-1",
        dry_run=False,
        runner=fake_runner,
    )

    assert results[0].status == "created"
    assert results[0].pr_number == 42
    assert ["git", "push", "-u", "origin", analysis.branch_name] in calls
    assert calls.index(["git", "push", "-u", "origin", analysis.branch_name]) < next(
        index for index, call in enumerate(calls) if call[:3] == ["gh", "pr", "create"]
    )


def test_pr_numbers_for_merge_uses_created_and_existing_prs() -> None:
    module = load_script()
    results = [
        module.PullRequestResult(11, "feature/issue-11", "created", None, "https://x/pull/42", ""),
        module.PullRequestResult(12, "feature/issue-12", "existing", 43, "https://x/pull/43", ""),
        module.PullRequestResult(13, "feature/issue-13", "blocked", 44, "https://x/pull/44", ""),
    ]

    assert module.pr_numbers_for_merge(results) == [42, 43]


def test_merge_pull_requests_waits_for_ci_before_merge() -> None:
    module = load_script()
    calls: list[list[str]] = []

    def fake_runner(args, **kwargs):  # type: ignore[no-untyped-def]
        calls.append(args)
        if args[:3] == ["gh", "pr", "view"]:
            return module.subprocess.CompletedProcess(
                args,
                0,
                json.dumps({"isDraft": False, "mergeStateStatus": "CLEAN", "number": 42}),
                "",
            )
        if args[:3] == ["gh", "pr", "checks"]:
            return module.subprocess.CompletedProcess(args, 0, "checks passed\n", "")
        if args[:3] == ["gh", "pr", "merge"]:
            return module.subprocess.CompletedProcess(args, 0, "", "")
        if args[:3] == ["git", "pull", "--ff-only"]:
            return module.subprocess.CompletedProcess(args, 0, "", "")
        return module.subprocess.CompletedProcess(args, 1, "", "unexpected")

    results = module.merge_pull_requests(
        [42],
        dry_run=False,
        merge_method="squash",
        integration_checks=[],
        runner=fake_runner,
    )

    assert results[0].status == "merged"
    assert ["gh", "pr", "checks", "42", "--watch", "--interval", "10"] in calls
    assert calls.index(["gh", "pr", "checks", "42", "--watch", "--interval", "10"]) < calls.index(
        ["gh", "pr", "merge", "42", "--squash"]
    )
