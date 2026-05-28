#!/usr/bin/env python3
"""Planner and staged runner for Codex issue orchestration.

The default mode is safe planning. Mutating steps such as worktree creation,
CommandMate dispatch, PR creation, and merging are implemented as explicit
phases and remain inspectable through generated run artifacts.
"""

from __future__ import annotations

import argparse
import difflib
import json
import re
import subprocess
import urllib.error
import urllib.request
from dataclasses import dataclass
from datetime import datetime, timezone
from pathlib import Path
from typing import Protocol

REPO_ROOT = Path(__file__).resolve().parents[1]
DEFAULT_RUNS_DIR = REPO_ROOT / "workspace" / "management" / "runs"
DEFAULT_BASE = "origin/develop"
PHASE_ORDER = {
    "issue": 1,
    "plan": 2,
    "dev": 3,
    "pr": 4,
    "merge": 5,
    "uat": 6,
}


class Runner(Protocol):
    def __call__(
        self,
        args: list[str],
        *,
        cwd: Path | None = None,
        check: bool = False,
        capture_output: bool = True,
        text: bool = True,
    ) -> subprocess.CompletedProcess[str]: ...


@dataclass(frozen=True)
class Issue:
    number: int
    title: str
    body: str
    labels: tuple[str, ...] = ()


@dataclass(frozen=True)
class IssueAnalysis:
    issue: Issue
    objective: str
    acceptance_criteria: tuple[str, ...]
    suspected_files: tuple[str, ...]
    reference_files: tuple[str, ...]
    test_expectations: tuple[str, ...]
    enhancement_needed: bool
    questions: tuple[str, ...]
    branch_name: str
    worktree_path: str
    dependency_hints: tuple[str, ...] = ()


@dataclass(frozen=True)
class WorktreeResult:
    issue_number: int
    branch_name: str
    worktree_path: Path
    status: str
    message: str


@dataclass(frozen=True)
class WorkerSessionResult:
    issue_number: int
    worktree_id: str
    status: str
    processing: bool | None
    running: bool | None
    message: str
    commands: tuple[str, ...]


@dataclass(frozen=True)
class WorkerWaitResult:
    issue_number: int
    worktree_id: str
    status: str
    message: str


@dataclass(frozen=True)
class PullRequestResult:
    issue_number: int
    branch_name: str
    status: str
    pr_number: int | None
    url: str | None
    message: str


@dataclass(frozen=True)
class MergeResult:
    pr_number: int
    status: str
    message: str
    verification_status: str = "not-run"


@dataclass(frozen=True)
class IssueEnhancementResult:
    issue_number: int
    status: str
    message: str
    diff: str


@dataclass(frozen=True)
class UatFixWorktreeResult:
    issue_number: int
    branch_name: str
    worktree_path: Path
    status: str
    message: str


@dataclass(frozen=True)
class PhotonEventResult:
    event_kind: str
    status: str
    message: str


@dataclass(frozen=True)
class UatFailure:
    issue_number: int
    scenario: str
    expected: str
    actual: str
    evidence: str


def slugify(value: str, *, max_len: int = 48) -> str:
    lowered = value.lower()
    normalized = re.sub(r"[^a-z0-9]+", "-", lowered).strip("-")
    compact = re.sub(r"-{2,}", "-", normalized)
    if not compact:
        return "task"
    return compact[:max_len].strip("-") or "task"


def build_parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("issues", nargs="+", type=int, help="GitHub issue numbers")
    parser.add_argument("--dry-run", action="store_true", help="Only write planning artifacts")
    parser.add_argument("--max-parallel", type=int, default=3)
    parser.add_argument(
        "--phase",
        default="merge",
        choices=("issue", "plan", "dev", "pr", "merge", "uat"),
    )
    parser.add_argument("--merge-order", default="")
    parser.add_argument("--skip-enhance", action="store_true")
    parser.add_argument(
        "--issue-json", type=Path, help="Fixture JSON for tests or offline planning"
    )
    parser.add_argument("--run-id", help="Stable run id override")
    parser.add_argument("--runs-dir", type=Path, default=DEFAULT_RUNS_DIR)
    parser.add_argument("--create-worktrees", action="store_true")
    parser.add_argument("--dispatch-commandmate", action="store_true")
    parser.add_argument("--create-prs", action="store_true")
    parser.add_argument("--merge-prs", action="store_true")
    parser.add_argument("--write-uat", action="store_true")
    parser.add_argument("--apply-issue-enhancements", action="store_true")
    parser.add_argument("--create-uat-fix-worktrees", action="store_true")
    parser.add_argument("--poll-commandmate", action="store_true")
    parser.add_argument("--codex-agent-name", default="")
    parser.add_argument("--commandmate-duration", default="3h")
    parser.add_argument("--wait-commandmate-timeout", type=int, default=10800)
    parser.add_argument("--wait-commandmate-stall-timeout", type=int, default=0)
    parser.add_argument("--repo", default="")
    parser.add_argument(
        "--integration-check",
        action="append",
        default=[],
        help="Command to run after each merge. Can be specified multiple times.",
    )
    parser.add_argument("--merge-method", choices=("merge", "squash", "rebase"), default=None)
    parser.add_argument(
        "--pr-numbers", default="", help="Comma-separated PR numbers for merge phase"
    )
    parser.add_argument("--uat-failures-json", type=Path)
    parser.add_argument("--photon-url", default="", help="Optional PHOTON sidecar base URL")
    return parser


def parse_args() -> argparse.Namespace:
    return build_parser().parse_args()


def parse_args_from_list_for_test(values: list[str]) -> argparse.Namespace:
    return build_parser().parse_args(values)


def load_issues(numbers: list[int], fixture_path: Path | None, repo: str = "") -> list[Issue]:
    if fixture_path is not None:
        return load_issues_from_fixture(numbers, fixture_path)
    return [fetch_issue_with_gh(number, repo) for number in numbers]


def load_issues_from_fixture(numbers: list[int], fixture_path: Path) -> list[Issue]:
    raw = json.loads(fixture_path.read_text(encoding="utf-8"))
    items = raw["issues"] if isinstance(raw, dict) and "issues" in raw else raw
    if not isinstance(items, list):
        raise ValueError("--issue-json must contain a list or an object with an 'issues' list")

    by_number: dict[int, Issue] = {}
    for item in items:
        if not isinstance(item, dict):
            continue
        number = int(item["number"])
        labels_raw = item.get("labels", [])
        labels = tuple(str(label) for label in labels_raw) if isinstance(labels_raw, list) else ()
        by_number[number] = Issue(
            number=number,
            title=str(item.get("title", "")),
            body=str(item.get("body", "")),
            labels=labels,
        )
    missing = [number for number in numbers if number not in by_number]
    if missing:
        raise ValueError(f"fixture does not contain issues: {missing}")
    return [by_number[number] for number in numbers]


def fetch_issue_with_gh(number: int, repo: str = "") -> Issue:
    args = [
        "gh",
        "issue",
        "view",
        str(number),
        "--json",
        "number,title,body,labels",
    ]
    if repo:
        args.extend(["--repo", repo])
    completed = subprocess.run(
        args,
        cwd=REPO_ROOT,
        check=True,
        capture_output=True,
        text=True,
    )
    raw = json.loads(completed.stdout)
    labels = tuple(label["name"] for label in raw.get("labels", []) if isinstance(label, dict))
    return Issue(
        number=int(raw["number"]),
        title=str(raw.get("title", "")),
        body=str(raw.get("body", "")),
        labels=labels,
    )


def run_command(
    args: list[str],
    *,
    cwd: Path | None = None,
    check: bool = False,
    capture_output: bool = True,
    text: bool = True,
) -> subprocess.CompletedProcess[str]:
    return subprocess.run(
        args,
        cwd=cwd,
        check=check,
        capture_output=capture_output,
        text=text,
    )


def analyze_issue(issue: Issue, repo_name: str, *, skip_enhance: bool) -> IssueAnalysis:
    text = f"{issue.title}\n\n{issue.body}"
    objective = first_nonempty_line(issue.body) or issue.title
    acceptance = extract_acceptance_criteria(issue.body)
    path_candidates = extract_file_candidates(text)
    suspected_files, reference_files = classify_file_candidates(path_candidates)
    suspected_files = enrich_file_candidates_with_rg(text, suspected_files)
    tests = extract_test_expectations(text)
    dependency_hints = extract_dependency_hints(text)
    questions: list[str] = []

    if not acceptance and not skip_enhance:
        questions.append(
            "受入条件が明確ではありません。期待する完了条件を1-3点で補足してください。"
        )
    if not suspected_files and not skip_enhance:
        questions.append(
            "影響範囲を特定できません。想定している機能領域やファイルがあれば教えてください。"
        )

    slug = slugify(issue.title)
    branch = f"feature/issue-{issue.number}-{slug}"
    worktree = f"../{repo_name}-issue-{issue.number}-{slug}"
    return IssueAnalysis(
        issue=issue,
        objective=objective,
        acceptance_criteria=tuple(acceptance),
        suspected_files=tuple(suspected_files),
        reference_files=tuple(reference_files),
        test_expectations=tuple(tests),
        enhancement_needed=bool(questions),
        questions=tuple(questions[:3]),
        branch_name=branch,
        worktree_path=worktree,
        dependency_hints=tuple(dependency_hints),
    )


def first_nonempty_line(value: str) -> str:
    for line in value.splitlines():
        if line.lstrip().startswith("#"):
            continue
        stripped = line.strip(" -#\t")
        if stripped:
            return stripped
    return ""


def extract_acceptance_criteria(body: str) -> list[str]:
    lines = body.splitlines()
    out: list[str] = []
    in_section = False
    heading_re = re.compile(r"^#{1,6}\s+")
    trigger_re = re.compile(r"(acceptance|受入|受け入れ|完了条件|期待結果)", re.IGNORECASE)
    for line in lines:
        stripped = line.strip()
        if heading_re.match(stripped):
            in_section = bool(trigger_re.search(stripped))
            continue
        if in_section:
            if not stripped:
                continue
            if stripped.startswith(("-", "*", "1.", "2.", "3.", "4.", "5.")):
                out.append(stripped.lstrip("-* 0123456789."))
    return [item for item in out if item]


def extract_file_candidates(text: str) -> list[str]:
    patterns = [
        r"`([^`\s]+\.(?:py|md|toml|json|yaml|yml|rs|ts|tsx|js|jsx|sh))`",
        (
            r"\b((?:src|crates|workspace|scripts|tests|docs|configs|examples|benches|"
            r"\.github|\.agents|\.claude|\.codex)/[A-Za-z0-9_./-]+)\b"
        ),
        r"\b([A-Za-z0-9_.-]+/(?:[A-Za-z0-9_.-]+/)*[A-Za-z0-9_.-]+\.(?:py|md|toml|json|yaml|yml|rs|ts|tsx|js|jsx|sh))\b",
    ]
    seen: set[str] = set()
    out: list[str] = []
    for pattern in patterns:
        for match in re.finditer(pattern, text):
            candidate = match.group(1).strip()
            if match.start(1) > 0 and text[match.start(1) - 1] == "/":
                continue
            if ".." in candidate or candidate.startswith("/"):
                continue
            if candidate.split("/", 1)[0] in {"Users", "home", "tmp", "private", "var"}:
                continue
            if candidate not in seen:
                seen.add(candidate)
                out.append(candidate)
    return out


def classify_file_candidates(candidates: list[str]) -> tuple[list[str], list[str]]:
    suspected: list[str] = []
    references: list[str] = []
    seen_suspected: set[str] = set()
    seen_references: set[str] = set()
    for candidate in candidates:
        if is_external_reference(candidate):
            if candidate not in seen_references:
                references.append(candidate)
                seen_references.add(candidate)
            continue
        if candidate not in seen_suspected:
            suspected.append(candidate)
            seen_suspected.add(candidate)
    return suspected, references


def is_external_reference(candidate: str) -> bool:
    first = candidate.split("/", 1)[0]
    if first in {"Users", "home", "tmp", "private", "var"}:
        return True
    repo_name = commandmate_repository_name()
    if first.startswith(("photon-", "anvil-", "Anvil-")) and first not in {
        REPO_ROOT.name,
        repo_name,
    }:
        return True
    if candidate.startswith(f"{REPO_ROOT.name}/"):
        return True
    return False


def repo_path_exists(candidate: str) -> bool:
    return (REPO_ROOT / candidate).exists()


def enrich_file_candidates_with_rg(text: str, existing: list[str]) -> list[str]:
    """Add a few repository paths found by rg without making planning depend on it."""
    candidates = list(existing)
    seen = set(candidates)
    for phrase in extract_search_phrases(text):
        try:
            completed = run_command(["rg", "-l", "--fixed-strings", phrase], cwd=REPO_ROOT)
        except FileNotFoundError:
            return candidates
        if completed.returncode not in (0, 1):
            continue
        for line in completed.stdout.splitlines()[:5]:
            path = line.strip()
            if not path or is_external_reference(path) or is_planning_noise_path(path):
                continue
            if path not in seen:
                candidates.append(path)
                seen.add(path)
            if len(candidates) >= 8:
                return candidates
    return candidates


def is_planning_noise_path(path: str) -> bool:
    if path.startswith(("workspace/management/", "dev-reports/")):
        return True
    relevant_roots = (
        "src/",
        "crates/",
        "tests/",
        "scripts/",
        "docs/",
        ".github/",
        ".agents/",
        ".claude/",
        ".codex/",
        "Cargo.toml",
        "Cargo.lock",
        "README.md",
        "CHANGELOG.md",
    )
    if not path.startswith(relevant_roots):
        return True
    return path in {
        "scripts/codex_orchestrate.py",
        "tests/test_codex_orchestrate.py",
    }


def extract_search_phrases(text: str) -> list[str]:
    raw = re.findall(r"\b[A-Za-z_][A-Za-z0-9_]{4,}\b", text)
    stop = {
        "Issue",
        "Acceptance",
        "Criteria",
        "schema",
        "version",
        "Implement",
        "概要",
        "対象",
        "完了条件",
    }
    phrases: list[str] = []
    seen: set[str] = set()
    for item in raw:
        if item in stop or item.lower() in {"request", "response", "event", "tests"}:
            continue
        if item not in seen:
            phrases.append(item)
            seen.add(item)
        if len(phrases) >= 4:
            break
    return phrases


def extract_dependency_hints(text: str) -> list[str]:
    hints: list[str] = []
    lowered = text.lower()
    if any(word in lowered for word in ("schema", "contract", "record")):
        hints.append("contract")
    if any(word in lowered for word in ("sqlite", "storage", "migration", "local-first")):
        hints.append("storage")
    if any(word in lowered for word in ("sanitizer", "redact", "secret", "token")):
        hints.append("sanitizer")
    if any(word in lowered for word in ("fastapi", "endpoint", "client", "/v1/")):
        hints.append("api")
    return hints


def extract_test_expectations(text: str) -> list[str]:
    commands = []
    for command in (
        "cargo test",
        "cargo clippy",
        "cargo fmt",
        "cargo build",
        "pytest",
        "ruff",
        "mypy",
        "python -m build",
        "npm test",
    ):
        if command in text:
            commands.append(command)
    return commands


def classify_batches(
    analyses: list[IssueAnalysis], merge_order: str
) -> tuple[list[list[int]], list[int]]:
    if merge_order:
        order = [int(part.strip()) for part in merge_order.split(",") if part.strip()]
        batches = [[number] for number in order]
        return batches, order

    remaining = list(analyses)
    completed: set[int] = set()
    batches: list[list[int]] = []
    while remaining:
        ready = [
            analysis
            for analysis in remaining
            if all(dep.issue.number in completed for dep in direct_dependencies(analysis, analyses))
        ]
        if not ready:
            ready = [remaining[0]]
        batch: list[IssueAnalysis] = []
        for analysis in ready:
            if len(batch) >= 3:
                break
            if any(has_file_overlap(analysis, existing) for existing in batch):
                continue
            batch.append(analysis)
        if not batch:
            batch = [ready[0]]
        batches.append([item.issue.number for item in batch])
        completed.update(item.issue.number for item in batch)
        batch_numbers = {item.issue.number for item in batch}
        remaining = [item for item in remaining if item.issue.number not in batch_numbers]
    order = [number for batch in batches for number in batch]
    return batches, order


def direct_dependencies(
    analysis: IssueAnalysis, analyses: list[IssueAnalysis]
) -> list[IssueAnalysis]:
    hints = set(analysis.dependency_hints)
    dependencies: list[IssueAnalysis] = []
    for other in analyses:
        if other.issue.number == analysis.issue.number:
            continue
        other_hints = set(other.dependency_hints)
        storage_needs_sanitizer = "storage" in hints and "sanitizer" in other_hints
        storage_needs_contract = (
            "storage" in hints and "contract" in other_hints and "api" not in other_hints
        )
        if storage_needs_sanitizer or storage_needs_contract:
            dependencies.append(other)
        elif "api" in hints and {"contract", "storage"} & other_hints:
            dependencies.append(other)
    return dependencies


def dependency_reason(analysis: IssueAnalysis, analyses: list[IssueAnalysis]) -> str:
    deps = direct_dependencies(analysis, analyses)
    if deps:
        return "depends on " + ", ".join(f"#{item.issue.number}" for item in deps)
    if any(has_file_overlap(analysis, other) for other in analyses if other != analysis):
        return "shared implementation file risk"
    return "no direct dependency detected"


def classify_issue(analysis: IssueAnalysis, analyses: list[IssueAnalysis]) -> str:
    if direct_dependencies(analysis, analyses):
        return "strong-dependency"
    if any(has_file_overlap(analysis, other) for other in analyses if other != analysis):
        return "weak-conflict"
    return "independent"


def merge_order_from_batches(batches: list[list[int]]) -> list[int]:
    return [number for batch in batches for number in batch]


def legacy_classify_batches(
    analyses: list[IssueAnalysis], merge_order: str
) -> tuple[list[list[int]], list[int]]:
    if merge_order:
        order = [int(part.strip()) for part in merge_order.split(",") if part.strip()]
    else:
        order = [analysis.issue.number for analysis in analyses]

    batches: list[list[int]] = []
    current: list[IssueAnalysis] = []
    for analysis in analyses:
        if any(has_file_overlap(analysis, existing) for existing in current):
            if current:
                batches.append([item.issue.number for item in current])
            current = [analysis]
        else:
            current.append(analysis)
    if current:
        batches.append([item.issue.number for item in current])
    return batches, order


def has_file_overlap(left: IssueAnalysis, right: IssueAnalysis) -> bool:
    if {"contract"} & set(left.dependency_hints) and {"sanitizer"} & set(right.dependency_hints):
        return False
    if {"sanitizer"} & set(left.dependency_hints) and {"contract"} & set(right.dependency_hints):
        return False
    left_files = {path for path in left.suspected_files if is_implementation_path(path)}
    right_files = {path for path in right.suspected_files if is_implementation_path(path)}
    return bool(left_files & right_files)


def is_implementation_path(path: str) -> bool:
    return not path.startswith(("workspace/", "README", "docs/"))


def current_branch() -> str:
    return run_git(["branch", "--show-current"]) or "unknown"


def current_commit() -> str:
    return run_git(["rev-parse", "--short", "HEAD"]) or "unknown"


def run_git(args: list[str]) -> str:
    completed = run_command(["git", *args], cwd=REPO_ROOT)
    return completed.stdout.strip()


def render_manifest(
    *,
    run_id: str,
    created_at: str,
    issues: list[int],
    phase: str,
    max_parallel: int,
    dry_run: bool,
) -> str:
    return "\n".join(
        [
            "# Orchestration Manifest",
            "",
            f"- Run ID: `{run_id}`",
            f"- Created at: `{created_at}`",
            f"- Repository: `{commandmate_repository_name()}`",
            f"- Start branch: `{current_branch()}`",
            f"- Start commit: `{current_commit()}`",
            f"- Requested issues: `{', '.join(str(issue) for issue in issues)}`",
            f"- Phase: `{phase}`",
            f"- Max parallel: `{max_parallel}`",
            f"- Dry run: `{str(dry_run).lower()}`",
            f"- Develop base: `{DEFAULT_BASE}`",
            "- CommandMate Codex agent: default CommandMate CLI selection (`--agent` omitted)",
            "",
            "## Generated Artifacts",
            "",
            "- `issue-analysis.md`",
            "- `dependency-plan.md`",
            "",
            "## User Questions",
            "",
            "See `issue-analysis.md`.",
            "",
        ]
    )


def render_issue_analysis(analyses: list[IssueAnalysis]) -> str:
    lines = ["# Issue Analysis", ""]
    for analysis in analyses:
        issue = analysis.issue
        lines.extend(
            [
                f"## Issue #{issue.number}: {issue.title}",
                "",
                f"- 種別: `{', '.join(issue.labels) if issue.labels else 'unknown'}`",
                f"- 目的: {analysis.objective}",
                f"- 詳細化要否: `{'yes' if analysis.enhancement_needed else 'no'}`",
                "",
                "### 受入条件",
                "",
                *bullet_or_none(analysis.acceptance_criteria),
                "",
                "### 推定影響ファイル",
                "",
                *bullet_or_none(analysis.suspected_files),
                "",
                "### 参考情報",
                "",
                *bullet_or_none(analysis.reference_files),
                "",
                "### テスト期待値",
                "",
                *bullet_or_none(analysis.test_expectations),
                "",
                "### ユーザーへの質問",
                "",
                *bullet_or_none(analysis.questions),
                "",
                "### GitHub Issue 反映候補",
                "",
                "詳細化要否が `yes` の場合、ユーザー回答後に反映する。",
                "",
            ]
        )
    return "\n".join(lines)


def build_issue_body_with_orchestration_notes(analysis: IssueAnalysis) -> str:
    marker = "<!-- codex-orchestrate-notes -->"
    end_marker = "<!-- /codex-orchestrate-notes -->"
    notes = "\n".join(
        [
            marker,
            "## Orchestration Notes",
            "",
            f"- Objective: {analysis.objective}",
            "- Acceptance criteria:",
            *[f"  - {item}" for item in analysis.acceptance_criteria],
            "- Suspected files:",
            *[f"  - {item}" for item in analysis.suspected_files],
            "- References:",
            *[f"  - {item}" for item in analysis.reference_files],
            "- Test expectations:",
            *[f"  - {item}" for item in analysis.test_expectations],
            end_marker,
            "",
        ]
    )
    body = analysis.issue.body.rstrip()
    pattern = re.compile(
        rf"\n*{re.escape(marker)}.*?{re.escape(end_marker)}\n*",
        re.DOTALL,
    )
    if pattern.search(body):
        return pattern.sub(f"\n\n{notes}", body).rstrip() + "\n"
    return f"{body}\n\n{notes}" if body else notes


def apply_issue_enhancements(
    analyses: list[IssueAnalysis],
    *,
    dry_run: bool,
    runner: Runner = run_command,
) -> list[IssueEnhancementResult]:
    results: list[IssueEnhancementResult] = []
    for analysis in analyses:
        new_body = build_issue_body_with_orchestration_notes(analysis)
        if new_body == analysis.issue.body:
            results.append(
                IssueEnhancementResult(
                    analysis.issue.number,
                    "unchanged",
                    "Issue body already contains current orchestration notes",
                    "",
                )
            )
            continue
        diff = "\n".join(
            difflib.unified_diff(
                analysis.issue.body.splitlines(),
                new_body.splitlines(),
                fromfile=f"issue-{analysis.issue.number}-before.md",
                tofile=f"issue-{analysis.issue.number}-after.md",
                lineterm="",
            )
        )
        if dry_run:
            results.append(
                IssueEnhancementResult(
                    analysis.issue.number,
                    "planned",
                    "dry-run: GitHub Issue update skipped",
                    diff,
                )
            )
            continue
        runner(
            ["gh", "issue", "edit", str(analysis.issue.number), "--body", new_body],
            cwd=REPO_ROOT,
            check=True,
        )
        results.append(
            IssueEnhancementResult(
                analysis.issue.number,
                "updated",
                "GitHub Issue body updated",
                diff,
            )
        )
    return results


def render_issue_enhancement_report(results: list[IssueEnhancementResult]) -> str:
    lines = ["# Issue Enhancement Report", ""]
    if not results:
        return "# Issue Enhancement Report\n\nNot requested.\n"
    for result in results:
        lines.extend(
            [
                f"## Issue #{result.issue_number}",
                "",
                f"- Status: `{result.status}`",
                f"- Message: {result.message}",
                "",
            ]
        )
        if result.diff:
            lines.extend(["```diff", result.diff, "```", ""])
    return "\n".join(lines)


def render_dependency_plan(
    analyses: list[IssueAnalysis],
    batches: list[list[int]],
    merge_order: list[int],
) -> str:
    lines = [
        "# Dependency Plan",
        "",
        "## Parallel Batches",
        "",
    ]
    for index, batch in enumerate(batches, start=1):
        lines.append(f"- Batch {index}: {', '.join(f'#{number}' for number in batch)}")
    lines.extend(["", "## Merge Order", ""])
    lines.append(", ".join(f"#{number}" for number in merge_order))
    lines.extend(["", "## Issue Plans", ""])
    for analysis in analyses:
        classification = classify_issue(analysis, analyses)
        lines.extend(
            [
                f"### Issue #{analysis.issue.number}",
                "",
                f"- Classification: `{classification}`",
                f"- Dependency reason: {dependency_reason(analysis, analyses)}",
                f"- Branch: `{analysis.branch_name}`",
                f"- Worktree: `{analysis.worktree_path}`",
                f"- Suspected files: `{', '.join(analysis.suspected_files) or 'unknown'}`",
                f"- References: `{', '.join(analysis.reference_files) or 'none'}`",
                "",
            ]
        )
    lines.extend(["## Blocked Items", "", "None at dry-run planning time.", ""])
    return "\n".join(lines)


def bullet_or_none(items: tuple[str, ...]) -> list[str]:
    if not items:
        return ["- None"]
    return [f"- {item}" for item in items]


def write_artifacts(args: argparse.Namespace, analyses: list[IssueAnalysis]) -> Path:
    created_at = datetime.now(timezone.utc).replace(microsecond=0).isoformat()
    run_id = args.run_id or f"{datetime.now(timezone.utc).strftime('%Y%m%d-%H%M%S')}-orchestrate"
    run_dir = args.runs_dir / run_id
    run_dir.mkdir(parents=True, exist_ok=False)

    batches, merge_order = classify_batches(analyses, args.merge_order)
    (run_dir / "manifest.md").write_text(
        render_manifest(
            run_id=run_id,
            created_at=created_at,
            issues=args.issues,
            phase=args.phase,
            max_parallel=args.max_parallel,
            dry_run=args.dry_run,
        ),
        encoding="utf-8",
    )
    (run_dir / "issue-analysis.md").write_text(render_issue_analysis(analyses), encoding="utf-8")
    (run_dir / "dependency-plan.md").write_text(
        render_dependency_plan(analyses, batches, merge_order),
        encoding="utf-8",
    )
    (run_dir / "issue-enhancement-report.md").write_text(
        "# Issue Enhancement Report\n\nNot requested.\n", encoding="utf-8"
    )
    (run_dir / "worker-sessions.md").write_text(
        "# Worker Sessions\n\nNot started.\n", encoding="utf-8"
    )
    (run_dir / "merge-report.md").write_text("# Merge Report\n\nNot started.\n", encoding="utf-8")
    (run_dir / "uat-report.md").write_text("# UAT Report\n\nNot started.\n", encoding="utf-8")
    (run_dir / "uat-fix-worktrees.md").write_text(
        "# UAT Fix Worktrees\n\nNot requested.\n", encoding="utf-8"
    )
    (run_dir / "photon-events.md").write_text(
        "# PHOTON Events\n\nNot configured.\n", encoding="utf-8"
    )
    (run_dir / "final-report.md").write_text("# Final Report\n\nNot completed.\n", encoding="utf-8")
    return run_dir


def phase_at_least(current: str, target: str) -> bool:
    return PHASE_ORDER[current] >= PHASE_ORDER[target]


def resolve_worktree_path(raw_path: str) -> Path:
    path = Path(raw_path)
    if path.is_absolute():
        return path
    return (REPO_ROOT / path).resolve()


def branch_exists(branch_name: str, runner: Runner = run_command) -> bool:
    completed = runner(
        ["git", "show-ref", "--verify", "--quiet", f"refs/heads/{branch_name}"],
        cwd=REPO_ROOT,
    )
    return completed.returncode == 0


def worktree_is_dirty(path: Path, runner: Runner = run_command) -> bool:
    completed = runner(["git", "status", "--porcelain"], cwd=path)
    return bool(completed.stdout.strip())


def create_or_reuse_worktrees(
    analyses: list[IssueAnalysis],
    *,
    dry_run: bool,
    runner: Runner = run_command,
) -> list[WorktreeResult]:
    results: list[WorktreeResult] = []
    if dry_run:
        return [
            WorktreeResult(
                issue_number=analysis.issue.number,
                branch_name=analysis.branch_name,
                worktree_path=resolve_worktree_path(analysis.worktree_path),
                status="planned",
                message="dry-run: worktree creation skipped",
            )
            for analysis in analyses
        ]

    runner(["git", "fetch", "origin", "develop"], cwd=REPO_ROOT, check=True)
    for analysis in analyses:
        path = resolve_worktree_path(analysis.worktree_path)
        if path.exists():
            if worktree_is_dirty(path, runner):
                results.append(
                    WorktreeResult(
                        issue_number=analysis.issue.number,
                        branch_name=analysis.branch_name,
                        worktree_path=path,
                        status="blocked",
                        message="existing worktree has uncommitted changes",
                    )
                )
            else:
                results.append(
                    WorktreeResult(
                        issue_number=analysis.issue.number,
                        branch_name=analysis.branch_name,
                        worktree_path=path,
                        status="reused",
                        message="existing clean worktree reused",
                    )
                )
            continue

        if branch_exists(analysis.branch_name, runner):
            cmd = ["git", "worktree", "add", str(path), analysis.branch_name]
        else:
            cmd = [
                "git",
                "worktree",
                "add",
                "-b",
                analysis.branch_name,
                str(path),
                DEFAULT_BASE,
            ]
        runner(cmd, cwd=REPO_ROOT, check=True)
        results.append(
            WorktreeResult(
                issue_number=analysis.issue.number,
                branch_name=analysis.branch_name,
                worktree_path=path,
                status="created",
                message="worktree created",
            )
        )
    return results


def render_worker_sessions(
    results: list[WorktreeResult], dispatch_results: list[WorkerSessionResult]
) -> str:
    lines = ["# Worker Sessions", ""]
    for result in results:
        session = next(
            (item for item in dispatch_results if item.issue_number == result.issue_number),
            None,
        )
        lines.extend(
            [
                f"## Issue #{result.issue_number}",
                "",
                f"- Branch: `{result.branch_name}`",
                f"- Worktree: `{result.worktree_path}`",
                f"- Status: `{result.status}`",
                f"- Message: {result.message}",
                f"- Worker status: `{session.status if session else 'not-dispatched'}`",
                f"- Running: `{session.running if session else 'unknown'}`",
                f"- Processing: `{session.processing if session else 'unknown'}`",
                f"- Worker message: {session.message if session else 'not dispatched'}",
                "",
            ]
        )
    commands = [command for result in dispatch_results for command in result.commands]
    if commands:
        lines.extend(["## CommandMate Dispatch", "", *[f"- `{line}`" for line in commands], ""])
    return "\n".join(lines)


def build_worker_prompt(analysis: IssueAnalysis) -> str:
    criteria = "\n".join(f"- {item}" for item in analysis.acceptance_criteria) or "- 未整理"
    suspected = "\n".join(f"- {item}" for item in analysis.suspected_files) or "- 未特定"
    references = "\n".join(f"- {item}" for item in analysis.reference_files) or "- なし"
    return "\n".join(
        [
            f"Codex issue worker task for Issue #{analysis.issue.number}",
            "",
            "If `/codex-issue-worker` is available in this worktree, follow that skill.",
            "If it is not available, treat this message as the full worker instruction.",
            "",
            "## Required Workflow",
            "",
            "1. Read the Issue summary, acceptance criteria, suspected files, and references.",
            "2. Write a short design note before editing.",
            "3. Implement the smallest coherent change that satisfies the Issue.",
            "4. Add or update focused tests where appropriate.",
            "5. Run focused verification, and broader checks if shared contracts are touched.",
            "6. Write `dev-reports/issue-<number>/design.md`, "
            "`implementation-summary.md`, and `verification.md`.",
            "7. Commit the work with a clear Issue-scoped commit message.",
            "8. Report blockers only if implementation cannot safely proceed.",
            "",
            "## Issue Summary",
            "",
            f"- Title: {analysis.issue.title}",
            f"- Objective: {analysis.objective}",
            "",
            "## Acceptance Criteria",
            "",
            criteria,
            "",
            "## Suspected Files",
            "",
            suspected,
            "",
            "## References",
            "",
            references,
            "",
            "## Orchestration Notes",
            "",
            f"- Branch: {analysis.branch_name}",
            f"- Worktree: {analysis.worktree_path}",
            "- Keep review lightweight and ask only blocking questions.",
        ]
    )


def build_commandmate_send_command(
    worktree_id: str,
    prompt: str,
    *,
    duration: str,
    codex_agent_name: str,
) -> list[str]:
    cmd = ["commandmatedev", "send", worktree_id, prompt]
    if codex_agent_name:
        cmd.extend(["--agent", codex_agent_name])
    cmd.extend(["--auto-yes", "--duration", duration])
    return cmd


def build_commandmate_ls_command(
    *, branch_prefix: str | None = None, json_output: bool = True
) -> list[str]:
    cmd = ["commandmatedev", "ls"]
    if branch_prefix:
        cmd.extend(["--branch", branch_prefix])
    if json_output:
        cmd.append("--json")
    return cmd


def commandmate_worktree_id(branch_name: str) -> str:
    return f"{commandmate_repository_name()}-{branch_name.replace('/', '-')}"


def commandmate_repository_name() -> str:
    name = REPO_ROOT.name
    match = re.match(r"(.+)-issue-\d+-", name)
    if match:
        return match.group(1)
    if name.endswith("-develop"):
        base = name.removesuffix("-develop")
        return base or name
    return name


def dispatch_commandmate(
    analyses: list[IssueAnalysis],
    worktree_results: list[WorktreeResult],
    *,
    dry_run: bool,
    duration: str,
    codex_agent_name: str,
    poll: bool = False,
    runner: Runner = run_command,
) -> list[WorkerSessionResult]:
    results: list[WorkerSessionResult] = []
    by_issue = {result.issue_number: result for result in worktree_results}
    for analysis in analyses:
        result = by_issue.get(analysis.issue.number)
        if result is None or result.status == "blocked":
            continue
        worktree_id = commandmate_worktree_id(result.branch_name)
        task = build_commandmate_send_command(
            worktree_id,
            build_worker_prompt(analysis),
            duration=duration,
            codex_agent_name=codex_agent_name,
        )
        commands = (" ".join(task),)
        if not dry_run:
            try:
                runner(task, cwd=REPO_ROOT, check=True)
            except subprocess.CalledProcessError as exc:
                message = str(exc)
                if exc.stderr:
                    message = exc.stderr.strip()
                elif exc.stdout:
                    message = exc.stdout.strip()
                results.append(
                    WorkerSessionResult(
                        issue_number=analysis.issue.number,
                        worktree_id=worktree_id,
                        status="blocked",
                        processing=None,
                        running=None,
                        message=message,
                        commands=commands,
                    )
                )
                continue
        status = WorkerSessionResult(
            issue_number=analysis.issue.number,
            worktree_id=worktree_id,
            status="planned" if dry_run else "sent",
            processing=None,
            running=None,
            message="dry-run: CommandMate dispatch skipped" if dry_run else "task sent",
            commands=commands,
        )
        if not dry_run and poll:
            status = poll_worker_startup(
                analysis.issue.number,
                worktree_id,
                codex_agent_name=codex_agent_name,
                commands=commands,
                runner=runner,
            )
        results.append(status)
    return results


def poll_worker_startup(
    issue_number: int,
    worktree_id: str,
    *,
    codex_agent_name: str,
    commands: tuple[str, ...],
    runner: Runner = run_command,
) -> WorkerSessionResult:
    state = get_commandmate_state(worktree_id, codex_agent_name=codex_agent_name, runner=runner)
    if state["processing"] is True:
        return WorkerSessionResult(
            issue_number,
            worktree_id,
            "processing",
            True,
            state["running"],
            "worker is processing",
            commands,
        )
    if state["running"] is True and state["processing"] is False:
        return WorkerSessionResult(
            issue_number,
            worktree_id,
            "started-but-idle",
            False,
            True,
            "worker session is running but not processing",
            commands,
        )
    if state["found"] is False:
        message = str(state.get("message") or "worktree session not found in CommandMate")
    else:
        message = "worker did not enter processing state"
    return WorkerSessionResult(
        issue_number,
        worktree_id,
        "blocked",
        state["processing"],
        state["running"],
        message,
        commands,
    )


def get_commandmate_state(
    worktree_id: str, *, codex_agent_name: str, runner: Runner = run_command
) -> dict[str, object]:
    completed = runner(build_commandmate_ls_command(), cwd=REPO_ROOT)
    if completed.returncode != 0:
        return {
            "found": False,
            "running": None,
            "processing": None,
            "status": "unreachable",
            "message": classify_commandmate_failure(completed),
        }
    if not completed.stdout.strip():
        return {
            "found": False,
            "running": None,
            "processing": None,
            "status": "empty-response",
            "message": "commandmatedev ls returned no output",
        }
    try:
        raw = json.loads(completed.stdout)
    except json.JSONDecodeError:
        return {
            "found": False,
            "running": None,
            "processing": None,
            "status": "invalid-json",
            "message": "commandmatedev ls returned invalid JSON",
        }
    items = raw if isinstance(raw, list) else raw.get("worktrees", [])
    for item in items:
        if not isinstance(item, dict):
            continue
        item_id = str(item.get("id") or item.get("name") or "")
        if item_id != worktree_id:
            continue
        session_status = item.get("sessionStatusByCli", {})
        cli_status = {}
        if isinstance(session_status, dict):
            cli_status = (
                (
                    session_status.get(codex_agent_name)
                    if codex_agent_name
                    else session_status.get("codex")
                )
                or session_status.get("default")
                or next(iter(session_status.values()), {})
            )
        running = bool(
            item.get("isSessionRunning")
            or item.get("isRunning")
            or str(item.get("status") or item.get("state") or "").lower() in {"running", "ready"}
        )
        processing = item.get("isProcessing")
        if processing is None and isinstance(cli_status, dict):
            processing = cli_status.get("isProcessing")
        return {
            "found": True,
            "running": running,
            "processing": bool(processing) if processing is not None else None,
            "status": "found",
            "message": "worktree session found",
        }
    return {
        "found": False,
        "running": None,
        "processing": None,
        "status": "not-found",
        "message": "worktree session not found in CommandMate",
    }


def classify_commandmate_failure(completed: subprocess.CompletedProcess[str]) -> str:
    output = "\n".join(part for part in (completed.stderr, completed.stdout) if part).strip()
    lowered = output.lower()
    if (
        "server is not running" in lowered
        or "fetch failed" in lowered
        or "econnrefused" in lowered
        or "couldn't connect to server" in lowered
    ):
        return (
            "commandmate-unreachable: local CommandMate HTTP endpoint could not be reached. "
            "This can be a sandbox localhost access issue; do not infer server shutdown or start "
            "CommandMate automatically."
        )
    return output or "commandmate command failed"


def wait_for_commandmate_workers(
    results: list[WorkerSessionResult],
    *,
    timeout_seconds: int,
    stall_timeout_seconds: int,
    runner: Runner = run_command,
) -> list[WorkerWaitResult]:
    wait_results: list[WorkerWaitResult] = []
    for result in results:
        if result.status not in {"sent", "processing", "started-but-idle"}:
            continue
        cmd = ["commandmatedev", "wait", result.worktree_id, "--timeout", str(timeout_seconds)]
        if stall_timeout_seconds > 0:
            cmd.extend(["--stall-timeout", str(stall_timeout_seconds)])
        completed = runner(cmd, cwd=REPO_ROOT)
        if completed.returncode == 0:
            wait_results.append(
                WorkerWaitResult(
                    result.issue_number,
                    result.worktree_id,
                    "completed",
                    completed.stdout.strip() or "worker completed",
                )
            )
        else:
            wait_results.append(
                WorkerWaitResult(
                    result.issue_number,
                    result.worktree_id,
                    "blocked",
                    classify_commandmate_failure(completed),
                )
            )
    return wait_results


def render_worker_wait_report(results: list[WorkerWaitResult]) -> str:
    lines = ["# CommandMate Wait Report", ""]
    if not results:
        return "# CommandMate Wait Report\n\nNo worker wait requested.\n"
    for result in results:
        lines.extend(
            [
                f"## Issue #{result.issue_number}",
                "",
                f"- Worktree ID: `{result.worktree_id}`",
                f"- Status: `{result.status}`",
                f"- Message: {result.message}",
                "",
            ]
        )
    return "\n".join(lines)


def render_pr_body(analysis: IssueAnalysis, run_id: str) -> str:
    files = "\n".join(f"- `{item}`" for item in analysis.suspected_files) or "- 未特定"
    tests = "\n".join(f"- `{item}`" for item in analysis.test_expectations) or "- 未実行"
    return "\n".join(
        [
            f"Closes #{analysis.issue.number}",
            "",
            "## Summary",
            "",
            f"- {analysis.objective}",
            "",
            "## Changed Files",
            "",
            files,
            "",
            "## Tests Run",
            "",
            tests,
            "",
            "## Known Risks",
            "",
            "- None recorded by orchestration planner.",
            "",
            "## Orchestration",
            "",
            f"- Run ID: `{run_id}`",
            "",
        ]
    )


def find_existing_pr(branch_name: str, *, runner: Runner = run_command) -> PullRequestResult | None:
    completed = runner(
        [
            "gh",
            "pr",
            "list",
            "--head",
            branch_name,
            "--base",
            "develop",
            "--json",
            "number,url,state",
        ],
        cwd=REPO_ROOT,
    )
    if completed.returncode != 0 or not completed.stdout.strip():
        return None
    raw = json.loads(completed.stdout)
    if not raw:
        return None
    first = raw[0]
    return PullRequestResult(
        issue_number=0,
        branch_name=branch_name,
        status="existing",
        pr_number=int(first["number"]),
        url=str(first.get("url") or ""),
        message=f"existing PR state: {first.get('state', 'unknown')}",
    )


def create_pull_requests(
    analyses: list[IssueAnalysis],
    *,
    run_id: str,
    dry_run: bool,
    runner: Runner = run_command,
) -> list[PullRequestResult]:
    results: list[PullRequestResult] = []
    for analysis in analyses:
        if not dry_run and not branch_has_commits(analysis.branch_name, runner=runner):
            results.append(
                PullRequestResult(
                    issue_number=analysis.issue.number,
                    branch_name=analysis.branch_name,
                    status="blocked",
                    pr_number=None,
                    url=None,
                    message="branch has no commits ahead of origin/develop",
                )
            )
            continue
        if not dry_run:
            push = push_branch_to_origin(analysis.branch_name, runner=runner)
            if push.returncode != 0:
                results.append(
                    PullRequestResult(
                        issue_number=analysis.issue.number,
                        branch_name=analysis.branch_name,
                        status="blocked",
                        pr_number=None,
                        url=None,
                        message=push.stderr.strip() or "branch push failed",
                    )
                )
                continue
        existing = None if dry_run else find_existing_pr(analysis.branch_name, runner=runner)
        if existing is not None:
            results.append(
                PullRequestResult(
                    issue_number=analysis.issue.number,
                    branch_name=analysis.branch_name,
                    status=existing.status,
                    pr_number=existing.pr_number,
                    url=existing.url,
                    message=existing.message,
                )
            )
            continue
        title = f"#{analysis.issue.number} {analysis.issue.title}"
        body = render_pr_body(analysis, run_id)
        cmd = [
            "gh",
            "pr",
            "create",
            "--base",
            "develop",
            "--head",
            analysis.branch_name,
            "--title",
            title,
            "--body",
            body,
        ]
        if dry_run:
            results.append(
                PullRequestResult(
                    issue_number=analysis.issue.number,
                    branch_name=analysis.branch_name,
                    status="planned",
                    pr_number=None,
                    url=None,
                    message="dry-run: PR creation skipped",
                )
            )
            continue
        completed = runner(cmd, cwd=REPO_ROOT, check=True)
        url = completed.stdout.strip() or None
        results.append(
            PullRequestResult(
                issue_number=analysis.issue.number,
                branch_name=analysis.branch_name,
                status="created",
                pr_number=parse_pr_number(url or ""),
                url=url,
                message="branch pushed and PR created",
            )
        )
    return results


def push_branch_to_origin(
    branch_name: str, *, runner: Runner = run_command
) -> subprocess.CompletedProcess[str]:
    return runner(["git", "push", "-u", "origin", branch_name], cwd=REPO_ROOT)


def parse_pr_number(value: str) -> int | None:
    match = re.search(r"/pull/(\d+)(?:\D*$|$)", value)
    if not match:
        return None
    return int(match.group(1))


def pr_numbers_for_merge(results: list[PullRequestResult]) -> list[int]:
    numbers: list[int] = []
    for result in results:
        if result.status not in {"created", "existing"}:
            continue
        if result.pr_number is not None:
            numbers.append(result.pr_number)
            continue
        parsed = parse_pr_number(result.url or "")
        if parsed is not None:
            numbers.append(parsed)
    return numbers


def branch_has_commits(branch_name: str, *, runner: Runner = run_command) -> bool:
    completed = runner(
        ["git", "rev-list", "--count", f"{DEFAULT_BASE}..{branch_name}"],
        cwd=REPO_ROOT,
    )
    if completed.returncode != 0:
        return False
    try:
        return int(completed.stdout.strip() or "0") > 0
    except ValueError:
        return False


def render_pr_report(results: list[PullRequestResult]) -> str:
    lines = ["# PR Report", ""]
    for result in results:
        lines.extend(
            [
                f"## Issue #{result.issue_number}",
                "",
                f"- Branch: `{result.branch_name}`",
                f"- Status: `{result.status}`",
                f"- PR: `{result.pr_number or result.url or 'pending'}`",
                f"- Message: {result.message}",
                "",
            ]
        )
    return "\n".join(lines)


def merge_pull_requests(
    pr_numbers: list[int],
    *,
    dry_run: bool,
    merge_method: str | None,
    integration_checks: list[str],
    runner: Runner = run_command,
) -> list[MergeResult]:
    results: list[MergeResult] = []
    for pr_number in pr_numbers:
        if dry_run:
            results.append(MergeResult(pr_number, "planned", "dry-run: merge skipped"))
            continue
        mergeable = check_pr_mergeability(pr_number, runner=runner)
        if mergeable.status != "mergeable":
            results.append(mergeable)
            break
        checks = wait_for_pr_checks(pr_number, runner=runner)
        if checks.status != "passed":
            results.append(MergeResult(pr_number, "blocked", checks.message))
            break
        cmd = ["gh", "pr", "merge", str(pr_number)]
        if merge_method is not None:
            cmd.append(f"--{merge_method}")
        merge = runner(cmd, cwd=REPO_ROOT)
        if merge.returncode != 0:
            results.append(
                MergeResult(pr_number, "blocked", merge.stderr.strip() or "merge failed")
            )
            break
        runner(["git", "pull", "--ff-only", "origin", "develop"], cwd=REPO_ROOT, check=True)
        verification = run_integration_checks(integration_checks, runner=runner)
        if verification.startswith("failed:"):
            results.append(
                MergeResult(
                    pr_number,
                    "blocked",
                    "merged, but integration verification failed",
                    verification,
                )
            )
            break
        results.append(MergeResult(pr_number, "merged", "merged and develop updated", verification))
    return results


def wait_for_pr_checks(pr_number: int, *, runner: Runner = run_command) -> MergeResult:
    checks = runner(
        ["gh", "pr", "checks", str(pr_number), "--watch", "--interval", "10"],
        cwd=REPO_ROOT,
    )
    if checks.returncode != 0:
        return MergeResult(
            pr_number,
            "blocked",
            checks.stderr.strip() or "CI checks failed or unavailable",
        )
    return MergeResult(pr_number, "passed", "CI checks passed")


def check_pr_mergeability(pr_number: int, *, runner: Runner = run_command) -> MergeResult:
    completed = runner(
        [
            "gh",
            "pr",
            "view",
            str(pr_number),
            "--json",
            "isDraft,mergeStateStatus,number",
        ],
        cwd=REPO_ROOT,
    )
    if completed.returncode != 0:
        return MergeResult(pr_number, "blocked", "could not read PR mergeability")
    raw = json.loads(completed.stdout)
    if raw.get("isDraft"):
        return MergeResult(pr_number, "blocked", "PR is draft")
    merge_state = str(raw.get("mergeStateStatus") or "UNKNOWN")
    if merge_state not in {"CLEAN", "HAS_HOOKS", "UNSTABLE", "UNKNOWN"}:
        return MergeResult(pr_number, "blocked", f"mergeStateStatus={merge_state}")
    return MergeResult(pr_number, "mergeable", f"mergeStateStatus={merge_state}")


def run_integration_checks(checks: list[str], *, runner: Runner = run_command) -> str:
    if not checks:
        return "not-configured"
    for check in checks:
        completed = runner(["sh", "-lc", check], cwd=REPO_ROOT)
        if completed.returncode != 0:
            return f"failed: {check}"
    return "passed"


def render_merge_report(results: list[MergeResult]) -> str:
    lines = ["# Merge Report", ""]
    if not results:
        return "# Merge Report\n\nNo PRs merged.\n"
    for result in results:
        lines.extend(
            [
                f"## PR #{result.pr_number}",
                "",
                f"- Status: `{result.status}`",
                f"- Message: {result.message}",
                f"- Verification: `{result.verification_status}`",
                "",
            ]
        )
    return "\n".join(lines)


def render_uat_report(analyses: list[IssueAnalysis]) -> str:
    lines = [
        "# UAT Report",
        "",
        "## Automated Checks",
        "",
        "- Not run by planner. Fill this section after develop verification.",
        "",
        "## Manual GUI / Real-device Checks",
        "",
    ]
    for analysis in analyses:
        criteria = analysis.acceptance_criteria or ("Issue の期待動作を満たすこと",)
        lines.extend([f"### Issue #{analysis.issue.number}: {analysis.issue.title}", ""])
        for index, criterion in enumerate(criteria, start=1):
            lines.extend(
                [
                    f"#### Scenario {index}",
                    "",
                    "- 前提: develop 反映後の最新版を使用する。",
                    f"- 操作: `{criterion}` を確認できる画面または実機操作を行う。",
                    f"- 期待結果: {criterion}",
                    "- Evidence: screenshot / relevant logs / 操作メモ / device or browser version",
                    "- Result: unchecked",
                    "",
                ]
            )
    lines.extend(
        [
            "## Fix Loop",
            "",
            "UAT が fail した場合は、該当 Issue / PR / file に mapping する。",
            "そのうえで focused failure prompt から follow-up worktree を作成する。",
            "Retry limit: 3",
            "",
        ]
    )
    return "\n".join(lines)


def load_uat_failures(path: Path | None) -> list[UatFailure]:
    if path is None:
        return []
    raw = json.loads(path.read_text(encoding="utf-8"))
    items = raw["failures"] if isinstance(raw, dict) and "failures" in raw else raw
    if not isinstance(items, list):
        raise ValueError("UAT failures JSON must be a list or an object with a 'failures' list")
    failures: list[UatFailure] = []
    for item in items:
        if not isinstance(item, dict):
            continue
        failures.append(
            UatFailure(
                issue_number=int(item["issue_number"]),
                scenario=str(item.get("scenario", "")),
                expected=str(item.get("expected", "")),
                actual=str(item.get("actual", "")),
                evidence=str(item.get("evidence", "")),
            )
        )
    return failures


def render_uat_fix_prompts(failures: list[UatFailure], analyses: list[IssueAnalysis]) -> str:
    by_issue = {analysis.issue.number: analysis for analysis in analyses}
    lines = ["# UAT Fix Prompts", ""]
    if not failures:
        lines.extend(["No UAT failures recorded.", ""])
        return "\n".join(lines)

    for failure in failures:
        analysis = by_issue.get(failure.issue_number)
        title = analysis.issue.title if analysis else "Unknown issue"
        branch = analysis.branch_name if analysis else f"fix/issue-{failure.issue_number}-uat"
        lines.extend(
            [
                f"## Issue #{failure.issue_number}: {title}",
                "",
                "```text",
                f"/codex-issue-worker UAT failure fix for Issue #{failure.issue_number}",
                "",
                "UAT で以下の scenario が fail しました。原因を特定し、最小修正を行ってください。",
                "",
                f"- Scenario: {failure.scenario or 'unspecified'}",
                f"- Expected: {failure.expected or 'unspecified'}",
                f"- Actual: {failure.actual or 'unspecified'}",
                f"- Evidence: {failure.evidence or 'not provided'}",
                f"- Suggested branch/worktree context: {branch}",
                "",
                "実施内容:",
                "1. 失敗を再現または evidence から原因を特定する。",
                "2. focused fix を行う。",
                "3. focused verification を実行する。",
                "4. UAT scenario の再確認手順を更新する。",
                "5. follow-up PR を作成できる状態にする。",
                "```",
                "",
            ]
        )
    return "\n".join(lines)


def create_uat_fix_worktrees(
    failures: list[UatFailure],
    *,
    dry_run: bool,
    runner: Runner = run_command,
) -> list[UatFixWorktreeResult]:
    results: list[UatFixWorktreeResult] = []
    seen: set[int] = set()
    for failure in failures:
        if failure.issue_number in seen:
            continue
        seen.add(failure.issue_number)
        slug = slugify(failure.scenario or "uat-failure", max_len=32)
        branch = f"fix/issue-{failure.issue_number}-uat-{slug}"
        path = (
            REPO_ROOT / f"../{REPO_ROOT.name}-fix-issue-{failure.issue_number}-uat-{slug}"
        ).resolve()
        if dry_run:
            results.append(
                UatFixWorktreeResult(
                    failure.issue_number,
                    branch,
                    path,
                    "planned",
                    "dry-run: UAT fix worktree creation skipped",
                )
            )
            continue
        if path.exists():
            if worktree_is_dirty(path, runner):
                results.append(
                    UatFixWorktreeResult(
                        failure.issue_number,
                        branch,
                        path,
                        "blocked",
                        "existing UAT fix worktree has uncommitted changes",
                    )
                )
            else:
                results.append(
                    UatFixWorktreeResult(
                        failure.issue_number,
                        branch,
                        path,
                        "reused",
                        "existing clean UAT fix worktree reused",
                    )
                )
            continue
        if branch_exists(branch, runner):
            cmd = ["git", "worktree", "add", str(path), branch]
        else:
            cmd = ["git", "worktree", "add", "-b", branch, str(path), DEFAULT_BASE]
        runner(cmd, cwd=REPO_ROOT, check=True)
        results.append(
            UatFixWorktreeResult(
                failure.issue_number,
                branch,
                path,
                "created",
                "UAT fix worktree created",
            )
        )
    return results


def render_uat_fix_worktree_report(results: list[UatFixWorktreeResult]) -> str:
    lines = ["# UAT Fix Worktrees", ""]
    if not results:
        return "# UAT Fix Worktrees\n\nNo UAT fix worktrees requested.\n"
    for result in results:
        lines.extend(
            [
                f"## Issue #{result.issue_number}",
                "",
                f"- Branch: `{result.branch_name}`",
                f"- Worktree: `{result.worktree_path}`",
                f"- Status: `{result.status}`",
                f"- Message: {result.message}",
                "",
            ]
        )
    return "\n".join(lines)


def write_final_report(run_dir: Path, analyses: list[IssueAnalysis]) -> None:
    lines = ["# Final Report", "", "## Issues", ""]
    for analysis in analyses:
        lines.append(f"- Issue #{analysis.issue.number}: {analysis.issue.title}")
    lines.extend(
        [
            "",
            "## Status",
            "",
            "Generated by current orchestration slice. Review phase-specific reports for details.",
            "",
        ]
    )
    (run_dir / "final-report.md").write_text("\n".join(lines), encoding="utf-8")


def emit_photon_event(
    base_url: str,
    *,
    event_kind: str,
    run_id: str,
    payload: dict[str, object],
) -> PhotonEventResult:
    if not base_url:
        return PhotonEventResult(event_kind, "skipped", "PHOTON URL not configured")
    body = json.dumps(
        {
            "schema_version": "codex-orchestrate.v1",
            "event_kind": event_kind,
            "run_id": run_id,
            "payload": sanitize_event_payload(payload),
        }
    ).encode("utf-8")
    request = urllib.request.Request(
        base_url.rstrip("/") + "/v1/events",
        data=body,
        headers={"Content-Type": "application/json"},
        method="POST",
    )
    try:
        with urllib.request.urlopen(request, timeout=2) as response:
            status = getattr(response, "status", 0)
        return PhotonEventResult(event_kind, "sent", f"HTTP {status}")
    except (OSError, urllib.error.URLError, TimeoutError) as exc:
        return PhotonEventResult(event_kind, "warning", f"PHOTON event failed: {exc}")


def sanitize_event_payload(value: object) -> object:
    if isinstance(value, dict):
        return {str(key): sanitize_event_payload(item) for key, item in value.items()}
    if isinstance(value, (list, tuple)):
        return [sanitize_event_payload(item) for item in value]
    if isinstance(value, str):
        return redact_paths(value)
    return value


def redact_paths(value: str) -> str:
    return re.sub(r"/(?:Users|home|tmp|private|var)/[^\s`'\"]+", "[REDACTED_PATH]", value)


def render_photon_events(results: list[PhotonEventResult]) -> str:
    lines = ["# PHOTON Events", ""]
    if not results:
        return "# PHOTON Events\n\nNot configured.\n"
    for result in results:
        lines.extend(
            [
                f"- `{result.event_kind}`: `{result.status}` - {result.message}",
            ]
        )
    lines.append("")
    return "\n".join(lines)


def main() -> int:
    args = parse_args()
    repo_name = commandmate_repository_name()
    issues = load_issues(args.issues, args.issue_json, args.repo)
    analyses = [analyze_issue(issue, repo_name, skip_enhance=args.skip_enhance) for issue in issues]
    run_dir = write_artifacts(args, analyses)
    dry_run = bool(args.dry_run)
    photon_results: list[PhotonEventResult] = [
        emit_photon_event(
            args.photon_url,
            event_kind="orchestrate.started",
            run_id=run_dir.name,
            payload={"issues": args.issues, "dry_run": dry_run, "phase": args.phase},
        ),
        emit_photon_event(
            args.photon_url,
            event_kind="issue.analysis.completed",
            run_id=run_dir.name,
            payload={
                "issues": [
                    {
                        "number": analysis.issue.number,
                        "enhancement_needed": analysis.enhancement_needed,
                        "suspected_files": list(analysis.suspected_files),
                    }
                    for analysis in analyses
                ]
            },
        ),
    ]

    if args.apply_issue_enhancements:
        issue_results = apply_issue_enhancements(analyses, dry_run=dry_run)
        (run_dir / "issue-enhancement-report.md").write_text(
            render_issue_enhancement_report(issue_results),
            encoding="utf-8",
        )

    worktree_results: list[WorktreeResult] = []
    dispatch_results: list[WorkerSessionResult] = []
    wait_results: list[WorkerWaitResult] = []
    publish_requested = (
        args.create_prs or args.merge_prs or (not dry_run and phase_at_least(args.phase, "pr"))
    )
    if args.create_worktrees or (not dry_run and phase_at_least(args.phase, "dev")):
        worktree_results = create_or_reuse_worktrees(analyses, dry_run=dry_run)
        dispatch_results = dispatch_commandmate(
            analyses,
            worktree_results,
            dry_run=dry_run or not args.dispatch_commandmate,
            duration=args.commandmate_duration,
            codex_agent_name=args.codex_agent_name,
            poll=args.poll_commandmate,
        )
        (run_dir / "worker-sessions.md").write_text(
            render_worker_sessions(worktree_results, dispatch_results),
            encoding="utf-8",
        )
        for result in dispatch_results:
            if result.status == "blocked":
                photon_results.append(
                    emit_photon_event(
                        args.photon_url,
                        event_kind="worker.blocked",
                        run_id=run_dir.name,
                        payload={
                            "issue_number": result.issue_number,
                            "worktree_id": result.worktree_id,
                            "message": result.message,
                        },
                    )
                )
            elif result.status in {"sent", "processing", "planned", "started-but-idle"}:
                photon_results.append(
                    emit_photon_event(
                        args.photon_url,
                        event_kind="worker.started",
                        run_id=run_dir.name,
                        payload={
                            "issue_number": result.issue_number,
                            "worktree_id": result.worktree_id,
                            "status": result.status,
                        },
                    )
                )

    can_publish = True
    if publish_requested and dispatch_results:
        can_publish = all(result.status != "blocked" for result in dispatch_results)
    if publish_requested and not dry_run and args.dispatch_commandmate and dispatch_results:
        wait_results = wait_for_commandmate_workers(
            dispatch_results,
            timeout_seconds=args.wait_commandmate_timeout,
            stall_timeout_seconds=args.wait_commandmate_stall_timeout,
        )
        (run_dir / "commandmate-wait-report.md").write_text(
            render_worker_wait_report(wait_results),
            encoding="utf-8",
        )
        can_publish = (
            can_publish
            and bool(wait_results)
            and all(result.status == "completed" for result in wait_results)
        )

    pr_results: list[PullRequestResult] = []
    if publish_requested and can_publish:
        pr_results = create_pull_requests(analyses, run_id=run_dir.name, dry_run=dry_run)
        (run_dir / "pr-report.md").write_text(render_pr_report(pr_results), encoding="utf-8")
    elif publish_requested:
        (run_dir / "pr-report.md").write_text(
            "# PR Report\n\nSkipped because one or more CommandMate workers did not complete.\n",
            encoding="utf-8",
        )

    merge_requested = args.merge_prs or (not dry_run and phase_at_least(args.phase, "merge"))
    if merge_requested and can_publish:
        pr_numbers = [int(part.strip()) for part in args.pr_numbers.split(",") if part.strip()]
        if not pr_numbers:
            pr_numbers = (
                [analysis.issue.number for analysis in analyses]
                if dry_run
                else pr_numbers_for_merge(pr_results)
            )
        if not pr_numbers:
            raise ValueError("merge phase requires created/existing PR numbers")
        merge_results = merge_pull_requests(
            pr_numbers,
            dry_run=dry_run,
            merge_method=args.merge_method,
            integration_checks=args.integration_check,
        )
        (run_dir / "merge-report.md").write_text(
            render_merge_report(merge_results), encoding="utf-8"
        )
        for result in merge_results:
            if result.status not in {"merged", "blocked"}:
                continue
            photon_results.append(
                emit_photon_event(
                    args.photon_url,
                    event_kind="pr.merged" if result.status == "merged" else "verification.failed",
                    run_id=run_dir.name,
                    payload={
                        "pr_number": result.pr_number,
                        "status": result.status,
                        "message": result.message,
                    },
                )
            )
    elif merge_requested:
        (run_dir / "merge-report.md").write_text(
            "# Merge Report\n\nSkipped because one or more CommandMate workers did not complete.\n",
            encoding="utf-8",
        )

    if args.write_uat or phase_at_least(args.phase, "uat"):
        (run_dir / "uat-report.md").write_text(render_uat_report(analyses), encoding="utf-8")
        failures = load_uat_failures(args.uat_failures_json)
        (run_dir / "uat-fix-prompts.md").write_text(
            render_uat_fix_prompts(failures, analyses),
            encoding="utf-8",
        )
        if args.create_uat_fix_worktrees:
            fix_results = create_uat_fix_worktrees(failures, dry_run=dry_run)
            (run_dir / "uat-fix-worktrees.md").write_text(
                render_uat_fix_worktree_report(fix_results),
                encoding="utf-8",
            )
        for failure in failures:
            photon_results.append(
                emit_photon_event(
                    args.photon_url,
                    event_kind="uat.failed",
                    run_id=run_dir.name,
                    payload={
                        "issue_number": failure.issue_number,
                        "scenario": failure.scenario,
                        "expected": failure.expected,
                        "actual": failure.actual,
                    },
                )
            )
        if not failures:
            photon_results.append(
                emit_photon_event(
                    args.photon_url,
                    event_kind="uat.passed",
                    run_id=run_dir.name,
                    payload={"issues": args.issues, "status": "no failures recorded"},
                )
            )

    write_final_report(run_dir, analyses)
    photon_results.append(
        emit_photon_event(
            args.photon_url,
            event_kind="orchestrate.completed",
            run_id=run_dir.name,
            payload={"issues": args.issues},
        )
    )
    (run_dir / "photon-events.md").write_text(
        render_photon_events(photon_results), encoding="utf-8"
    )
    print(run_dir)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
