from __future__ import annotations

import re
from pathlib import Path
from typing import Any

from .plan_capability_contract import (
    PLAN_CAPABILITY_ORACLE_VERSION,
    collect_plan_contract,
    score_plan_capability_contract,
    verify_capability_detected,
)


PLAN_VERIFY_ORACLE_VERSION = "plan-verify-v1"
VERIFY_SKIP_DIRS = {"node_modules", ".next", ".git", "target", "dist", "build"}
MAX_VERIFY_ARTIFACT_BYTES = 200_000


def score_plan_verify_coverage(
    *,
    scenario: dict[str, Any],
    mode: str,
    plan_paths: list[Path] | None = None,
    plan_data: dict[str, Any] | None = None,
    workdir: Path | None = None,
    postcheck_events: list[dict[str, Any]] | None = None,
    plan_capability_result: dict[str, Any] | None = None,
) -> dict[str, Any]:
    capability = plan_capability_result or score_plan_capability_contract(
        scenario=scenario,
        plan_paths=plan_paths or [],
        plan_data=plan_data,
    )
    required = [str(cap) for cap in capability.get("plan_required_capabilities", []) or []]
    if capability.get("capability_contract_details", {}).get("applicable") is False:
        return not_applicable(str(capability.get("prompt_plan_gap_kind", "no_plan_contract")))
    if not required:
        return not_applicable("no_plan_capability_contract")

    _, plan_items, parse_errors = collect_plan_contract(plan_paths or [], plan_data)
    commands = collect_verify_commands(plan_items)
    declared_text = "\n".join(commands).lower()
    declared = capability_hits(required, declared_text)
    command_classes = [classify_verify_command(command) for command in commands]
    declared_score = coverage_score(required, declared, command_classes, executed=False)

    artifact_reads: list[dict[str, Any]] = []
    executed_text = declared_text
    if mode != "step-plan" and workdir is not None:
        for command in commands:
            for rel_path in referenced_verify_artifact_paths(command):
                artifact = safe_read_verify_artifact(workdir, rel_path)
                artifact_reads.append(artifact)
                if artifact.get("content"):
                    executed_text += "\n" + str(artifact["content"]).lower()
        for event in postcheck_events or []:
            executed_text += "\n" + " ".join(str(value) for value in event.values()).lower()

    executed = capability_hits(required, executed_text) if mode != "step-plan" else declared
    executed_score: float | str = ""
    if mode != "step-plan":
        executed_score = coverage_score(required, executed, command_classes, executed=True)
    display_score: float | str = declared_score if mode == "step-plan" else executed_score
    if display_score == "":
        display_score = declared_score
    unverified = [cap for cap in required if cap not in executed]
    verified_for_count = declared if mode == "step-plan" else executed
    return {
        "plan_verify_declared_coverage_score": declared_score,
        "executed_verify_coverage_score": executed_score,
        "plan_verify_coverage_score": display_score,
        "plan_verified_capability_count": len(verified_for_count),
        "plan_unverified_capability_count": len(required) - len(verified_for_count),
        "plan_verify_gap_kind": plan_verify_gap_kind(
            required=required,
            verified=verified_for_count,
            commands=commands,
            command_classes=command_classes,
            artifact_reads=artifact_reads,
        ),
        "plan_verify_oracle_version": PLAN_VERIFY_ORACLE_VERSION,
        "plan_verify_details": {
            "applicable": True,
            "oracle_version": PLAN_VERIFY_ORACLE_VERSION,
            "plan_capability_oracle_version": PLAN_CAPABILITY_ORACLE_VERSION,
            "required_capabilities": required,
            "declared_verified_capabilities": sorted(declared),
            "executed_verified_capabilities": sorted(executed) if mode != "step-plan" else [],
            "unverified_capabilities": unverified,
            "commands": commands,
            "command_classes": command_classes,
            "artifact_reads": [
                {key: value for key, value in item.items() if key != "content"}
                for item in artifact_reads
            ],
            "parse_errors": parse_errors,
        },
    }


def not_applicable(reason: str) -> dict[str, Any]:
    return {
        "plan_verify_declared_coverage_score": "",
        "executed_verify_coverage_score": "",
        "plan_verify_coverage_score": "",
        "plan_verified_capability_count": "",
        "plan_unverified_capability_count": "",
        "plan_verify_gap_kind": reason,
        "plan_verify_oracle_version": PLAN_VERIFY_ORACLE_VERSION,
        "plan_verify_details": {
            "applicable": False,
            "reason": reason,
            "oracle_version": PLAN_VERIFY_ORACLE_VERSION,
        },
    }


def collect_verify_commands(plan_items: list[dict[str, Any]]) -> list[str]:
    commands: list[str] = []
    for item in plan_items:
        commands.extend(str(command).strip() for command in item.get("verify", []) or [])
    return [command for command in commands if command]


def classify_verify_command(command: str) -> dict[str, Any]:
    lowered = " ".join(command.lower().split())
    kind = "other"
    strength = 20
    if not lowered:
        kind = "empty"
        strength = 0
    elif lowered.startswith(("test -f ", "test -s ", "ls ")):
        kind = "file_existence"
        strength = 8
    elif lowered.startswith("cat ") or ("readfilesync" in lowered and "assert" not in lowered) or "existssync" in lowered:
        kind = "contentless_read"
        strength = 10
    elif any(token in lowered for token in ["playwright", "cypress", "puppeteer", "browser", "interaction"]):
        kind = "browser_declaration"
        strength = 85
    elif any(token in lowered for token in ["npm test", "vitest", "jest", "pytest", "unittest", "cargo test", "go test"]):
        kind = "test_runner"
        strength = 65
    elif any(token in lowered for token in ["npm run build", "next build", "cargo build", "tsc"]):
        kind = "build_compile"
        strength = 25
    elif re.search(r"\bgrep\b|\bassert\b|smoke[-_]?check|semantic|capability", lowered):
        kind = "source_assertion"
        strength = 55
    if referenced_verify_artifact_paths(command):
        kind = "dedicated_smoke_artifact" if kind == "other" else kind
        strength = max(strength, 60)
    return {"command": command, "kind": kind, "strength": strength}


def capability_hits(required: list[str], text: str) -> set[str]:
    return {capability for capability in required if verify_capability_detected(capability, text)}


def coverage_score(
    required: list[str],
    verified: set[str],
    command_classes: list[dict[str, Any]],
    *,
    executed: bool,
) -> float:
    if not required:
        return 100.0
    semantic = 100.0 * len(verified) / max(1, len(required))
    if verified:
        return round(semantic, 1)
    if not command_classes:
        return 0.0
    max_strength = max(float(item.get("strength", 0)) for item in command_classes)
    if any(item.get("kind") in {"file_existence", "contentless_read"} for item in command_classes):
        max_strength = min(max_strength, 12.0)
    if any(item.get("kind") == "build_compile" for item in command_classes):
        max_strength = min(max_strength, 25.0)
    if any(item.get("kind") == "test_runner" for item in command_classes):
        max_strength = min(max_strength, 35.0)
    if executed and any(item.get("kind") == "dedicated_smoke_artifact" for item in command_classes):
        max_strength = min(max_strength, 45.0)
    return round(max_strength, 1)


def plan_verify_gap_kind(
    *,
    required: list[str],
    verified: set[str],
    commands: list[str],
    command_classes: list[dict[str, Any]],
    artifact_reads: list[dict[str, Any]],
) -> str:
    if not required:
        return "no_plan_capability_contract"
    if len(verified) == len(required):
        return ""
    if not commands:
        return "semantic_capability_unverified"
    kinds = {str(item.get("kind", "")) for item in command_classes}
    if kinds and kinds.issubset({"build_compile"}):
        return "build_only_verify_for_behavior_contract"
    if kinds and kinds.issubset({"file_existence", "contentless_read"}):
        return "contentless_verify_for_capability_contract"
    if any(item.get("status") == "missing" for item in artifact_reads):
        return "missing_verify_artifact"
    if any(item.get("status") in {"outside_workspace", "skipped_dir", "too_large", "binary"} for item in artifact_reads):
        return "verify_artifact_not_referenced"
    if "browser_declaration" not in kinds and any(cap in required for cap in ["keyboard_or_player_control", "render_loop_or_canvas"]):
        return "browser_required_but_not_declared"
    return "semantic_capability_unverified"


def referenced_verify_artifact_paths(command: str) -> list[str]:
    paths: list[str] = []
    patterns = [
        r"\bnode\s+([^\s;&|]+)",
        r"\bpython3?\s+([^\s;&|]+)",
        r"\bpytest\s+([^\s;&|]+)",
    ]
    for pattern in patterns:
        for match in re.finditer(pattern, command):
            candidate = match.group(1).strip("'\"")
            if candidate.startswith("-"):
                continue
            if Path(candidate).suffix.lower() in {".js", ".mjs", ".cjs", ".ts", ".tsx", ".py"}:
                paths.append(candidate)
    for match in re.finditer(r"['\"]([^'\"]*(?:smoke|check|test)[^'\"]*\.(?:js|ts|py))['\"]", command, re.IGNORECASE):
        paths.append(match.group(1))
    return sorted(dict.fromkeys(paths))


def safe_read_verify_artifact(workdir: Path, rel_path: str) -> dict[str, Any]:
    candidate = (workdir / rel_path).resolve()
    root = workdir.resolve()
    try:
        candidate.relative_to(root)
    except ValueError:
        return {"path": rel_path, "status": "outside_workspace"}
    if any(part in VERIFY_SKIP_DIRS for part in candidate.parts):
        return {"path": rel_path, "status": "skipped_dir"}
    if not candidate.exists():
        return {"path": rel_path, "status": "missing"}
    if not candidate.is_file():
        return {"path": rel_path, "status": "not_file"}
    try:
        size = candidate.stat().st_size
    except OSError as err:
        return {"path": rel_path, "status": "stat_error", "error": str(err)}
    if size > MAX_VERIFY_ARTIFACT_BYTES:
        return {"path": rel_path, "status": "too_large", "bytes": size}
    try:
        raw = candidate.read_bytes()
    except OSError as err:
        return {"path": rel_path, "status": "read_error", "error": str(err)}
    if b"\x00" in raw[:4096]:
        return {"path": rel_path, "status": "binary", "bytes": size}
    return {
        "path": rel_path,
        "status": "read",
        "bytes": size,
        "content": raw.decode("utf-8", errors="replace"),
    }
