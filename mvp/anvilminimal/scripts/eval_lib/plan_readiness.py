from __future__ import annotations

import re
from pathlib import Path
from typing import Any

from .simple_yaml import load_yaml


READINESS_FIELDS = [
    "plan_run_readiness_score",
    "verify_policy_readiness_score",
    "contract_handoff_score",
    "declared_contract_completeness_score",
    "runner_handoff_integrity_score",
    "postcheck_contract_alignment_score",
    "dependency_ordering_score",
    "finalization_readiness_score",
    "readiness_blocking_issue_count",
    "readiness_warning_count",
    "readiness_cap_reason",
    "readiness_source",
    "plan_run_missed_predictive_signal",
    "missed_predictive_signal_reason",
    "readiness_false_positive_kind",
    "readiness_false_negative_kind",
    "ultra_phase_readiness_min_score",
    "ultra_phase_readiness_avg_score",
    "ultra_phase_readiness_failing_phase",
    "ultra_phase_readiness_cap_reason",
]

VERIFY_POLICY_VIOLATION_SCORES = {
    "empty": 0,
    "blocked": 0,
    "shell_control_syntax": 30,
    "setup_or_dev_server": 40,
    "workspace_escape": 0,
}

MANIFEST_NAMES = {
    "package.json",
    "Cargo.toml",
    "pyproject.toml",
    "requirements.txt",
    "go.mod",
    "pom.xml",
    "build.gradle",
}

VERIFY_KEYWORDS = {
    "build",
    "test",
    "check",
    "lint",
    "pytest",
    "cargo",
    "npm",
    "pnpm",
    "yarn",
    "tsc",
    "go test",
    "python -m",
}


def empty_plan_readiness_scores() -> dict[str, Any]:
    return {field: "" for field in READINESS_FIELDS}


def score_plan_readiness_file(
    path: str | Path,
    *,
    profile: str = "",
    prompt: str = "",
    handoff_events: list[dict[str, Any]] | None = None,
    source: str = "eval_derived",
) -> dict[str, Any]:
    try:
        data = load_yaml(path)
    except Exception as err:
        out = empty_plan_readiness_scores()
        out.update(
            {
                "plan_run_readiness_score": 0.0,
                "verify_policy_readiness_score": 0.0,
                "contract_handoff_score": 0.0,
                "declared_contract_completeness_score": 0.0,
                "postcheck_contract_alignment_score": 0.0,
                "dependency_ordering_score": 0.0,
                "finalization_readiness_score": 0.0,
                "readiness_blocking_issue_count": 1,
                "readiness_warning_count": 0,
                "readiness_cap_reason": "parse_failure",
                "readiness_source": source,
                "details": {"issues": [{"severity": "blocking", "kind": "parse_failure", "message": str(err)}]},
            }
        )
        return out
    out = score_plan_readiness(data, profile=profile, prompt=prompt, handoff_events=handoff_events, source=source)
    out["plan_path"] = str(path)
    return out


def score_plan_readiness(
    plan: dict[str, Any],
    *,
    profile: str = "",
    prompt: str = "",
    handoff_events: list[dict[str, Any]] | None = None,
    source: str = "eval_derived",
) -> dict[str, Any]:
    if "phases" in plan:
        return empty_plan_readiness_scores()
    steps = plan.get("steps") or []
    if not isinstance(steps, list) or not steps:
        return invalid_step_plan_readiness("missing_steps", source)
    if not str(plan.get("goal", "")).strip():
        return invalid_step_plan_readiness("missing_goal", source)

    issues: list[dict[str, Any]] = []
    verify_policy = score_verify_policy_readiness(steps, issues)
    declared = score_declared_contract_completeness(plan, profile=profile, prompt=prompt, issues=issues)
    runner = score_runner_handoff_integrity(handoff_events or [])
    contract_handoff = min(declared, runner) if isinstance(runner, (int, float)) else declared
    alignment = score_postcheck_contract_alignment(plan, issues)
    dependency = score_dependency_ordering(steps, issues)
    finalization = score_finalization_readiness(plan, issues)

    raw_score = (
        0.24 * verify_policy
        + 0.24 * contract_handoff
        + 0.18 * alignment
        + 0.18 * dependency
        + 0.16 * finalization
    )
    blocking = [issue for issue in issues if issue.get("severity") == "blocking"]
    warnings = [issue for issue in issues if issue.get("severity") != "blocking"]
    score = raw_score
    cap_reason = ""
    if blocking:
        score = min(score, 55.0)
        cap_reason = ";".join(sorted({str(issue.get("kind", "")) for issue in blocking if issue.get("kind")}))
    elif min(verify_policy, contract_handoff, alignment, dependency, finalization) < 60:
        score = min(score, 70.0)
        cap_reason = "weak_min_subscore"

    readiness_source = source
    if isinstance(runner, (int, float)):
        readiness_source = f"{source}+runtime_event"

    return {
        "plan_run_readiness_score": round(max(0.0, min(100.0, score)), 1),
        "verify_policy_readiness_score": round(verify_policy, 1),
        "contract_handoff_score": round(contract_handoff, 1),
        "declared_contract_completeness_score": round(declared, 1),
        "runner_handoff_integrity_score": round(runner, 1) if isinstance(runner, (int, float)) else "",
        "postcheck_contract_alignment_score": round(alignment, 1),
        "dependency_ordering_score": round(dependency, 1),
        "finalization_readiness_score": round(finalization, 1),
        "readiness_blocking_issue_count": len(blocking),
        "readiness_warning_count": len(warnings),
        "readiness_cap_reason": cap_reason,
        "readiness_source": readiness_source,
        "details": {"issues": issues},
    }


def invalid_step_plan_readiness(reason: str, source: str) -> dict[str, Any]:
    out = empty_plan_readiness_scores()
    out.update(
        {
            "plan_run_readiness_score": 0.0,
            "verify_policy_readiness_score": 0.0,
            "contract_handoff_score": 0.0,
            "declared_contract_completeness_score": 0.0,
            "postcheck_contract_alignment_score": 0.0,
            "dependency_ordering_score": 0.0,
            "finalization_readiness_score": 0.0,
            "readiness_blocking_issue_count": 1,
            "readiness_warning_count": 0,
            "readiness_cap_reason": reason,
            "readiness_source": source,
            "details": {"issues": [{"severity": "blocking", "kind": reason}]},
        }
    )
    return out


def score_verify_policy_readiness(steps: list[dict[str, Any]], issues: list[dict[str, Any]]) -> float:
    commands = collect_verify_commands(steps)
    if not commands:
        issues.append({"severity": "warning", "kind": "no_verify_commands"})
        return 60.0
    command_scores = []
    for command in commands:
        diagnosis = diagnose_verify_command(command)
        violation = diagnosis.get("violation", "")
        if violation:
            severity = "blocking" if violation in {"empty", "blocked", "workspace_escape"} else "warning"
            issues.append(
                {
                    "severity": severity,
                    "kind": f"verify_policy_{violation}",
                    "command": diagnosis.get("normalized", ""),
                    "reason": diagnosis.get("reason", ""),
                }
            )
            command_scores.append(float(VERIFY_POLICY_VIOLATION_SCORES.get(violation, 30)))
        else:
            command_scores.append(100.0)
    return min(command_scores) if command_scores else 60.0


def diagnose_verify_command(command: str) -> dict[str, str]:
    normalized = " ".join(str(command).split())
    if not normalized:
        return violation(normalized, "empty", "verify command is empty")
    if blocked_command(normalized):
        return violation(normalized, "blocked", "verify command is blocked")
    if contains_shell_control_syntax(normalized):
        return violation(normalized, "shell_control_syntax", "verify command may not use shell control syntax")
    lower = normalized.lower()
    if any(pattern in lower for pattern in ["npm install", "pnpm install", "yarn install", "cargo install", "next dev", "vite --host"]):
        return violation(normalized, "setup_or_dev_server", "verify command may not perform setup or start a dev server")
    manifest_path = manifest_path_arg(normalized)
    if manifest_path and workspace_escape(manifest_path):
        return violation(normalized, "workspace_escape", "verify command manifest path escapes workspace")
    return {"normalized": normalized, "violation": "", "reason": ""}


def violation(normalized: str, kind: str, reason: str) -> dict[str, str]:
    return {"normalized": normalized, "violation": kind, "reason": reason}


def blocked_command(command: str) -> bool:
    lower = command.lower().strip()
    blocked_prefixes = (
        "rm ",
        "rmdir ",
        "sudo ",
        "chmod ",
        "chown ",
        "mkfs",
        "shutdown",
        "reboot",
    )
    return lower in {"rm", "sudo", "shutdown", "reboot"} or lower.startswith(blocked_prefixes)


def contains_shell_control_syntax(command: str) -> bool:
    return any(token in command for token in ["&&", "||", "|", ";", "`", "$("])


def manifest_path_arg(command: str) -> str:
    parts = command.split()
    for index, part in enumerate(parts[:-1]):
        if part == "--manifest-path":
            return parts[index + 1]
    return ""


def workspace_escape(path: str) -> bool:
    return path.startswith("/") or path == ".." or path.startswith("../") or "/../" in path


def score_declared_contract_completeness(
    plan: dict[str, Any],
    *,
    profile: str,
    prompt: str,
    issues: list[dict[str, Any]],
) -> float:
    steps = plan.get("steps") or []
    checks = {
        "goal": bool(str(plan.get("goal", "")).strip()),
        "steps": bool(steps),
        "expected_result": all(str(step.get("expected_result", "")).strip() for step in steps),
        "artifact_owner": any(step_expected_paths(step) for step in steps),
        "verify": bool(collect_verify_commands(steps)),
        "profile_contract": profile_contract_declared(plan, profile=profile, prompt=prompt),
    }
    if not checks["expected_result"]:
        issues.append({"severity": "warning", "kind": "missing_expected_result"})
    if not checks["artifact_owner"]:
        issues.append({"severity": "warning", "kind": "missing_expected_paths"})
    if not checks["verify"]:
        issues.append({"severity": "warning", "kind": "missing_verify"})
    score = 100.0 * sum(1 for ok in checks.values() if ok) / len(checks)
    return score


def profile_contract_declared(plan: dict[str, Any], *, profile: str, prompt: str) -> bool:
    profile = str(profile or "").strip().lower()
    if profile in {"", "generic", "default", "none"}:
        return True
    text = plan_text(plan).lower()
    prompt_text = str(prompt or "").lower()
    generic_hits = 0
    for group in [
        ["manifest", "package", "dependency", "dependencies", "cargo", "pyproject", "requirements", "go.mod"],
        ["build", "test", "verify", "check", "smoke"],
        ["config", "runtime", "port", "server"],
    ]:
        if any(word in text for word in group):
            generic_hits += 1
    if "port" in prompt_text and not re.search(r"\bport\b|\b-p\s+\d+\b|:\d{2,5}\b", text):
        return False
    return generic_hits >= 2


def score_runner_handoff_integrity(events: list[dict[str, Any]]) -> float | str:
    contract_events = [event for event in events if event.get("event") == "step_prompt_contract"]
    if not contract_events:
        return ""
    scores = []
    for event in contract_events:
        checks = [
            bool(event.get("has_overall_goal")),
            bool(event.get("has_required_final_artifacts")),
            bool(event.get("has_expected_paths")),
            bool(event.get("has_verify_commands")),
            bool(event.get("has_expected_result")),
            bool(event.get("has_bounded_repair_policy")),
        ]
        if event.get("prior_artifact_context_applicable"):
            checks.append(bool(event.get("has_prior_artifact_context")))
        scores.append(100.0 * sum(1 for ok in checks if ok) / len(checks))
    return sum(scores) / len(scores)


def score_postcheck_contract_alignment(plan: dict[str, Any], issues: list[dict[str, Any]]) -> float:
    steps = plan.get("steps") or []
    paths = declared_paths(steps)
    verify_commands = collect_verify_commands(steps)
    score = 100.0
    if paths and not verify_commands:
        issues.append({"severity": "warning", "kind": "declared_paths_without_verify"})
        score -= 30.0
    if verify_commands and not paths and any(implementation_like_step(step) for step in steps):
        issues.append({"severity": "warning", "kind": "verify_without_declared_artifacts"})
        score -= 25.0
    if duplicate_paths(paths):
        issues.append({"severity": "blocking", "kind": "duplicate_declared_path_ownership"})
        score -= 35.0
    if nested_paths_without_owner(paths):
        issues.append({"severity": "warning", "kind": "nested_paths_without_parent_context"})
        score -= 10.0
    if terminal_report_only(plan) and (paths or verify_commands):
        issues.append({"severity": "warning", "kind": "terminal_report_after_artifact_contract"})
        score -= 15.0
    return max(0.0, score)


def score_dependency_ordering(steps: list[dict[str, Any]], issues: list[dict[str, Any]]) -> float:
    manifest_step = first_step_index_with_manifest(steps)
    verify_step = first_step_index_with_build_or_test_verify(steps)
    artifact_step = first_step_index_with_artifact(steps)
    score = 100.0
    if verify_step is not None and artifact_step is None:
        issues.append({"severity": "warning", "kind": "verify_before_artifact_contract"})
        score -= 25.0
    if verify_step is not None and artifact_step is not None and verify_step < artifact_step:
        issues.append({"severity": "blocking", "kind": "verify_before_artifact_owner"})
        score -= 40.0
    if verify_step is not None and manifest_step is None and looks_project_build_or_test(collect_verify_commands(steps)):
        issues.append({"severity": "warning", "kind": "project_verify_without_manifest_contract"})
        score -= 20.0
    if verify_step is not None and manifest_step is not None and verify_step < manifest_step:
        issues.append({"severity": "blocking", "kind": "verify_before_manifest_owner"})
        score -= 40.0
    for command in collect_verify_commands(steps):
        if diagnose_verify_command(command).get("violation") == "setup_or_dev_server":
            score -= 20.0
            break
    return max(0.0, score)


def score_finalization_readiness(plan: dict[str, Any], issues: list[dict[str, Any]]) -> float:
    steps = plan.get("steps") or []
    paths = declared_paths(steps)
    verify_commands = collect_verify_commands(steps)
    score = 100.0
    if not paths and not verify_commands:
        issues.append({"severity": "warning", "kind": "no_declared_completion_contract"})
        score -= 35.0
    if terminal_report_only(plan):
        issues.append({"severity": "warning", "kind": "report_only_final_step"})
        score -= 25.0
    if not any(str(step.get("expected_result", "")).strip() for step in steps):
        score -= 15.0
    if verify_commands and not any(verify_attached_to_terminal_or_verify_step(step) for step in steps):
        issues.append({"severity": "warning", "kind": "verify_not_attached_to_completion_step"})
        score -= 10.0
    return max(0.0, score)


def aggregate_ultra_phase_readiness(scores: list[dict[str, Any]]) -> dict[str, Any]:
    values = [
        float(score["plan_run_readiness_score"])
        for score in scores
        if score.get("plan_run_readiness_score") not in {"", None}
    ]
    out = {
        "ultra_phase_readiness_min_score": "",
        "ultra_phase_readiness_avg_score": "",
        "ultra_phase_readiness_failing_phase": "",
        "ultra_phase_readiness_cap_reason": "",
    }
    if not values:
        return out
    min_index, min_value = min(
        ((index, float(score["plan_run_readiness_score"])) for index, score in enumerate(scores) if score.get("plan_run_readiness_score") not in {"", None}),
        key=lambda item: item[1],
    )
    weakest = scores[min_index]
    out.update(
        {
            "ultra_phase_readiness_min_score": round(min_value, 1),
            "ultra_phase_readiness_avg_score": round(sum(values) / len(values), 1),
            "ultra_phase_readiness_failing_phase": Path(str(weakest.get("plan_path", f"phase-{min_index + 1}"))).name,
            "ultra_phase_readiness_cap_reason": weakest.get("readiness_cap_reason", ""),
        }
    )
    return out


def classify_readiness_outcome(
    readiness_score: object,
    *,
    success: bool,
    failure_kind: str = "",
) -> dict[str, str]:
    try:
        score = float(readiness_score)
    except (TypeError, ValueError):
        return {
            "plan_run_missed_predictive_signal": "",
            "missed_predictive_signal_reason": "",
            "readiness_false_positive_kind": "",
            "readiness_false_negative_kind": "",
        }
    if score >= 80.0 and not success:
        reason = missed_signal_reason(failure_kind)
        return {
            "plan_run_missed_predictive_signal": "true",
            "missed_predictive_signal_reason": reason,
            "readiness_false_positive_kind": failure_kind or "unknown_failure",
            "readiness_false_negative_kind": "",
        }
    if score < 70.0 and success:
        return {
            "plan_run_missed_predictive_signal": "true",
            "missed_predictive_signal_reason": "low_readiness_but_success",
            "readiness_false_positive_kind": "",
            "readiness_false_negative_kind": "low_readiness_but_success",
        }
    return {
        "plan_run_missed_predictive_signal": "",
        "missed_predictive_signal_reason": "",
        "readiness_false_positive_kind": "",
        "readiness_false_negative_kind": "",
    }


def missed_signal_reason(failure_kind: str) -> str:
    mapping = {
        "verify_command_policy_error": "verify_policy_not_reflected_in_readiness",
        "postcheck_failure": "postcheck_contract_not_reflected_in_readiness",
        "max_iterations": "finalization_readiness_not_reflected",
        "plan_final_contract_failure": "contract_handoff_not_reflected_in_readiness",
        "tool_validation_error": "tool_policy_not_reflected_in_readiness",
        "tool_execution_error": "tool_policy_not_reflected_in_readiness",
        "planner_lint_error": "lint_policy_not_reflected_in_readiness",
    }
    return mapping.get(failure_kind, "runtime_failure_not_reflected_in_readiness")


def collect_verify_commands(steps: list[dict[str, Any]]) -> list[str]:
    commands = []
    for step in steps:
        for command in step.get("verify", []) or []:
            text = str(command).strip()
            if text:
                commands.append(text)
    return commands


def step_expected_paths(step: dict[str, Any]) -> list[str]:
    return [str(path) for path in step.get("expected_paths", []) or [] if str(path).strip()]


def declared_paths(steps: list[dict[str, Any]]) -> list[str]:
    return [path for step in steps for path in step_expected_paths(step)]


def duplicate_paths(paths: list[str]) -> bool:
    return len(paths) != len(set(paths))


def nested_paths_without_owner(paths: list[str]) -> bool:
    path_set = set(paths)
    for path in paths:
        if "/" not in path.strip("/"):
            continue
        parent = path.rsplit("/", 1)[0]
        if parent and parent in path_set:
            return False
    return False


def implementation_like_step(step: dict[str, Any]) -> bool:
    kind = str(step.get("kind", "")).lower()
    instruction = str(step.get("instruction", "")).lower()
    return kind in {"implement", "setup", "work"} or any(word in instruction for word in ["create", "write", "implement", "add", "update"])


def terminal_report_only(plan: dict[str, Any]) -> bool:
    steps = plan.get("steps") or []
    if not steps:
        return False
    last = steps[-1]
    return str(last.get("kind", "")).lower() == "report" and not step_expected_paths(last) and not (last.get("verify") or [])


def first_step_index_with_manifest(steps: list[dict[str, Any]]) -> int | None:
    for index, step in enumerate(steps):
        if any(Path(path).name in MANIFEST_NAMES for path in step_expected_paths(step)):
            return index
    return None


def first_step_index_with_build_or_test_verify(steps: list[dict[str, Any]]) -> int | None:
    for index, step in enumerate(steps):
        commands = " ".join(str(command).lower() for command in step.get("verify", []) or [])
        if any(keyword in commands for keyword in VERIFY_KEYWORDS):
            return index
    return None


def first_step_index_with_artifact(steps: list[dict[str, Any]]) -> int | None:
    for index, step in enumerate(steps):
        if step_expected_paths(step):
            return index
    return None


def looks_project_build_or_test(commands: list[str]) -> bool:
    text = " ".join(command.lower() for command in commands)
    return any(keyword in text for keyword in ["npm ", "pnpm ", "yarn ", "cargo ", "go test", "pytest", "mvn ", "gradle"])


def verify_attached_to_terminal_or_verify_step(step: dict[str, Any]) -> bool:
    if not (step.get("verify") or []):
        return False
    kind = str(step.get("kind", "")).lower()
    return kind in {"verify", "implement", "setup"}


def plan_text(plan: dict[str, Any]) -> str:
    parts: list[str] = []
    parts.append(str(plan.get("goal", "")))
    for step in plan.get("steps", []) or []:
        parts.append(str(step.get("id", "")))
        parts.append(str(step.get("kind", "")))
        parts.append(str(step.get("instruction", "")))
        parts.append(str(step.get("expected_result", "")))
        parts.extend(str(path) for path in step.get("expected_paths", []) or [])
        parts.extend(str(command) for command in step.get("verify", []) or [])
    return "\n".join(parts)
