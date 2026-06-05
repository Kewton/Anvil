#!/usr/bin/env python3
"""analyze_run.py — compute standard metrics from a bench run-dir.

Usage:
    scripts/analyze_run.py <run-dir>

Reads:
    <run-dir>/session.json          required
    <run-dir>/meta.json             optional
    <run-dir>/logs/llm-io.jsonl     optional
    <run-dir>/workdir/src/app/page.tsx  optional

Writes:
    stdout: JSON (dict-sorted keys, schema_version=1)

Exit codes:
    0  success (partial failures fall back to null for individual metrics)
    1  argument error (missing run-dir)
    2  run-dir or session.json not found / not a regular file
    3  session.json parse error / schema invalid / size limit exceeded
"""

from __future__ import annotations

import json
import os
import stat
import sys
from pathlib import Path
from typing import Any

# ---------------------------------------------------------------------------
# constants
# ---------------------------------------------------------------------------

SCHEMA_VERSION = 1
KEYWORDS_VERSION = 1

WRITE_EDIT_TOOLS = ["Write", "Edit"]
DEFAULT_TASK_KIND = "coding"
KNOWN_TASK_KINDS = {
    "coding",
    "docs",
    "data",
    "research",
    "ops",
    # Issue #919 (P2): production TaskKind for artifact-producing prose
    # (translation / rewriting). `answer_only` is an eval-only label (Option B:
    # no production TaskKind::AnswerOnly) for pure-answer tasks.
    "authoring",
    "answer_only",
}
OBJECTIVE_KINDS_BY_TASK_KIND = {
    "coding": ("source_files", "test_run"),
    "docs": ("document_sections", "content_check"),
    "data": ("output_file", "schema_check"),
    "research": ("research_notes", "source_fetch_evidence"),
    "ops": ("command_observation", "safety_boundary_evidence"),
    "authoring": ("prose_artifact", "content_acceptance"),
    "answer_only": ("answer", "content_acceptance"),
}
GENERIC_TERMINAL_BY_FINAL_OUTCOME = {
    "completed": "completed",
    "done": "completed",
    "missing_deliverable": "missing_deliverable",
    "missing_repo_edits": "missing_deliverable",
    "missing_evidence": "missing_evidence",
    "missing_verification": "missing_evidence",
    "evidence_failed": "evidence_failed",
    "verifier_failed": "evidence_failed",
    "evidence_binding_failed": "evidence_binding_failed",
    "safe_stop_verifier_weak": "evidence_binding_failed",
    "evidence_runner_missing": "evidence_runner_missing",
    "safe_stop_verifier_missing": "evidence_runner_missing",
    "evidence_repair_exhausted": "evidence_repair_exhausted",
    "repair_exhausted": "evidence_repair_exhausted",
    "evidence_repair_safe_stop": "evidence_repair_safe_stop",
    "repair_safe_stop": "evidence_repair_safe_stop",
    "control_loop_exhausted": "control_loop_exhausted",
    "max_iterations": "control_loop_exhausted",
    "plan_incomplete": "control_loop_exhausted",
    "model_output_failure": "model_output_failure",
    "empty_responses": "model_output_failure",
    "no_tool_calls": "model_output_failure",
    "tool_call_format_error": "model_output_failure",
    "transport_failure": "transport_failure",
    "transport_error": "transport_failure",
    "interrupted": "interrupted",
}
KNOWN_GENERIC_TERMINAL_STATES = set(GENERIC_TERMINAL_BY_FINAL_OUTCOME.values()) | {
    "unknown",
}
KNOWN_LIFECYCLE_FAILURE_STAGES = {
    "classification",
    "deliverable",
    "evidence_authoring",
    "runner_binding",
    "diagnostic_classification",
    "repair",
    "rerun",
    "completed",
    "unknown",
}
RECOVERY_JOB_BY_GENERIC_TERMINAL = {
    "completed": "none",
    "missing_deliverable": "MissingDeliverableJob",
    "missing_evidence": "MissingEvidenceJob",
    "evidence_failed": "EvidenceFailedJob",
    # Issue #993 (parent #988, Issue E): a deliverable exists but its evidence
    # runner cannot bind. Kept aligned with the Rust
    # `RecoveryJobKind::EvidenceBindingFailedJob` projection.
    "evidence_binding_failed": "EvidenceBindingFailedJob",
    "evidence_runner_missing": "ToolFailureJob",
    "evidence_repair_exhausted": "EvidenceFailedJob",
    "evidence_repair_safe_stop": "EvidenceFailedJob",
    "control_loop_exhausted": "none",
    "model_output_failure": "ToolFailureJob",
    "transport_failure": "ToolFailureJob",
    "interrupted": "none",
    "unknown": "unknown",
}
# Issue #976 (parent #974, Issue B): FailureObservation replay classifier.
#
# `failure_class` collapses the generic terminal lifecycle state into the small
# transition vocabulary the v0.6.3 countermeasure analysis tracks. It lets the
# offline report count *transitions* (e.g. missing_evidence shrinking while
# recovery_exhausted grows) rather than only a single pass rate. The map is a
# refinement layer on top of `generic_terminal_state`; detectors below can
# override individual classes (e.g. a bound runner that failed under a
# `verifier_missing` terminal is reclassified to `evidence_failed`).
FAILURE_CLASS_BY_GENERIC_TERMINAL = {
    "completed": "none",
    "missing_deliverable": "missing_deliverable",
    "missing_evidence": "missing_evidence",
    "evidence_failed": "evidence_failed",
    "evidence_binding_failed": "evidence_failed",
    "evidence_runner_missing": "evidence_runner_missing",
    "evidence_repair_exhausted": "recovery_exhausted",
    "evidence_repair_safe_stop": "recovery_exhausted",
    "control_loop_exhausted": "control_loop_exhausted",
    "model_output_failure": "tool_protocol_failure",
    "transport_failure": "transport_failure",
    "interrupted": "interrupted",
    "unknown": "unknown",
}
KNOWN_FAILURE_CLASSES = set(FAILURE_CLASS_BY_GENERIC_TERMINAL.values())
# Terminal families that imply an evidence runner was actually bound + executed.
EVIDENCE_RAN_TERMINAL_STATES = {
    "evidence_failed",
    "evidence_binding_failed",
    "evidence_repair_exhausted",
    "evidence_repair_safe_stop",
}
# Terminal families where a repair loop ran against a failing diagnostic.
REPAIR_TERMINAL_STATES = {
    "evidence_repair_exhausted",
    "evidence_repair_safe_stop",
}
# Verifier-status labels that mean a runner ran (vs. was never available).
VERIFIER_STATUS_EXECUTED = {
    "failed",
    "ran",
    "executed",
    "nonzero_exit",
    "passed",
    "weak",
}
# Verifier-status labels that mean a runner ran and did NOT pass.
VERIFIER_STATUS_FAILED = {
    "failed",
    "nonzero_exit",
    "weak",
}
# Substrings (lowercased) in a failure signature/classification that point at a
# test- or setup-side cause rather than the implementation under repair.
TEST_OR_SETUP_FAILURE_MARKERS = (
    "import",
    "module",
    "modulenotfound",
    "no module named",
    "unresolved",
    "cannot find module",
    "no such module",
    "missing test",
    "no test",
    "no tests",
    "scripts.test",
    "test script",
    "package.json",
    "cargo.toml",
    "requirements",
    "dependency",
    "unresolved import",
)
# recovery_strategies labels (lowercased substrings) that mark a deterministic
# operator firing ahead of an LLM edit pass. Best-effort until an explicit
# `deterministic_operator_hit` signal is emitted by the eval log.
DETERMINISTIC_OPERATOR_MARKERS = (
    "deterministic",
    "operator",
    "scaffold",
)
KNOWN_TARGET_ROLES = {
    "implementation",
    "test",
    "setup",
    "test_or_setup",
    "deliverable",
    "evidence",
    "tool_protocol",
    "none",
    "unknown",
}

KNOWN_FAILURE_AUTHORITIES = {
    "contract_extraction",
    "artifact_classification",
    "verifier_setup",
    "generated_test_bug",
    "implementation_bug",
    "repair_routing",
    "success",
    "unknown",
}
WORKER_LIFECYCLE_STRING_FIELDS = {
    "worker_kind",
    "context_pack_kind",
    "diagnostic_class",
    "lifecycle_failure_stage",
}
WORKER_LIFECYCLE_BOOL_FIELDS = {
    "deliverable_created",
    "evidence_created",
    "runner_bound",
    "diagnostic_classified",
    "repair_applied",
    "rerun_passed",
}
WORKER_LIFECYCLE_INT_FIELDS = {
    "context_token_estimate",
    "context_entry_count",
}

GAME_KEYWORDS_V1 = [
    "game",
    "score",
    "lives",
    "bullet",
    "enemy",
    "ship",
    "invader",
    "player",
    "level",
    "wave",
]

COMPACT_SUMMARY_PREFIX = "[compact-summary]"
LLM_IO_ERROR_EVENT = "ollama.generate.error"
LLM_IO_ERROR_KIND = "status"
LLM_IO_REPLY_EVENTS: frozenset[str] = frozenset(
    {"ollama.generate.reply_final", "ollama.chat.reply_final"}
)

# size limits (bytes)
MAX_SESSION_JSON = 10 * 1024 * 1024  # 10 MiB
MAX_META_JSON = 256 * 1024  # 256 KiB
MAX_PAGE_TSX = 2 * 1024 * 1024  # 2 MiB
MAX_LLM_IO_JSONL = 500 * 1024 * 1024  # 500 MiB overall streaming cap
MAX_LLM_IO_LINE = 1 * 1024 * 1024  # 1 MiB per line
MAX_EVAL_JSONL = 100 * 1024 * 1024  # 100 MiB overall cap
MAX_EVAL_JSONL_LINE = 1 * 1024 * 1024  # 1 MiB per line

CODE_EXTS = {
    ".c",
    ".cc",
    ".cpp",
    ".cs",
    ".go",
    ".h",
    ".hpp",
    ".java",
    ".js",
    ".jsx",
    ".kt",
    ".php",
    ".py",
    ".rb",
    ".rs",
    ".sh",
    ".swift",
    ".ts",
    ".tsx",
}
CONFIG_EXTS = {".lock", ".toml", ".yaml", ".yml"}
DOC_EXTS = {".md", ".mdx", ".rst", ".txt"}
DATA_EXTS = {".csv", ".json", ".jsonl", ".parquet", ".tsv", ".xlsx", ".yaml", ".yml"}
PROTECTED_EXACT_PATHS = {
    "meta.json",
    "session.json",
    "summary.tsv",
    "stdout.log",
    "stderr.log",
}
PROTECTED_PREFIXES = (
    ".anvil/",
    "logs/",
    "state/",
    "tmp-tests/metadata/",
)

FAILURE_AUTHORITY_ALIASES = {
    "contract extraction": "contract_extraction",
    "artifact classification": "artifact_classification",
    "verifier setup": "verifier_setup",
    "generated test bug": "generated_test_bug",
    "generated-test bug": "generated_test_bug",
    "generated_test_failure": "generated_test_bug",
    "implementation bug": "implementation_bug",
    "repair routing": "repair_routing",
    "model_output_failure": "contract_extraction",
    "verification_environment_failure": "verifier_setup",
    "verification_failure": "implementation_bug",
    "control_loop_failure": "repair_routing",
    "transport_failure": "repair_routing",
    "interrupted": "repair_routing",
    "none": "success",
}


# ---------------------------------------------------------------------------
# helpers
# ---------------------------------------------------------------------------


def _warn(msg: str) -> None:
    print(f"warning: {msg}", file=sys.stderr)


def _is_regular_file(p: Path) -> bool:
    """Return True iff p exists, is a regular file, and is not a symlink."""
    try:
        st = p.lstat()
    except OSError:
        return False
    if stat.S_ISLNK(st.st_mode):
        return False
    if not stat.S_ISREG(st.st_mode):
        return False
    return True


def _safe_regular_file_in(candidate: Path, parent: Path, limit: int) -> Path | None:
    """Canonicalize candidate and return it iff it is a regular file inside parent
    within the size limit, and the top-level path is NOT itself a symlink.

    Returns None on any violation.
    """
    try:
        if candidate.is_symlink():
            return None
        if not candidate.exists():
            return None
        resolved = candidate.resolve(strict=True)
        parent_resolved = parent.resolve(strict=True)
    except OSError:
        return None
    if not resolved.is_relative_to(parent_resolved):
        return None
    if not _is_regular_file(resolved):
        return None
    try:
        size = resolved.stat().st_size
    except OSError:
        return None
    if size > limit:
        _warn(f"file exceeds size limit, skipping: {candidate}")
        return None
    return resolved


# ---------------------------------------------------------------------------
# path normalization
# ---------------------------------------------------------------------------


def normalize_path(
    raw_path: str,
    workdir: Path | None,
    *,
    workdir_resolved: Path | None = None,
    workdir_name: str | None = None,
) -> str | None:
    """Normalize a tool_call path relative to workdir.

    workdir_resolved and workdir_name are pre-computed caches; pass them when
    calling inside a loop to avoid repeated resolve() syscalls.

    Returns None if the path is invalid, outside workdir, or contains traversal.
    """
    if not raw_path or "\x00" in raw_path:
        return None

    p = Path(raw_path)

    if p.is_absolute():
        if workdir is None:
            return None
        if workdir_resolved is None:
            try:
                workdir_resolved = workdir.resolve(strict=True)
            except OSError:
                return None
        try:
            resolved = p.resolve(strict=False)
            rel = resolved.relative_to(workdir_resolved)
        except (OSError, ValueError):
            return None
        return rel.as_posix()

    # relative path: strip duplicate project-name prefix (one segment)
    if workdir is not None:
        if workdir_name is None:
            try:
                workdir_name = (workdir_resolved or workdir.resolve(strict=True)).name
            except OSError:
                workdir_name = workdir.name
        parts = p.parts
        if len(parts) > 1 and parts[0] == workdir_name:
            p = Path(*parts[1:])

    # drop empty / "." segments
    cleaned = [part for part in p.parts if part not in ("", ".")]
    normalized = Path(*cleaned) if cleaned else Path(".")
    if any(part == ".." for part in normalized.parts):
        return None
    if not cleaned:
        return None
    return normalized.as_posix()


# ---------------------------------------------------------------------------
# session.json analysis
# ---------------------------------------------------------------------------


def _analyze_session(
    session: dict[str, Any], workdir: Path | None
) -> dict[str, Any]:
    messages = session.get("messages")
    if not isinstance(messages, list):
        raise ValueError("session.json: 'messages' must be a list")

    # pre-compute workdir resolve cache to avoid repeated syscalls in the loop
    wdir_resolved: Path | None = None
    wdir_name: str | None = None
    if workdir is not None:
        try:
            wdir_resolved = workdir.resolve(strict=True)
            wdir_name = wdir_resolved.name
        except OSError:
            wdir_name = workdir.name

    iter_count = 0
    compact_events = 0
    tool_calls: dict[str, int] = {}
    files_modified: list[str] = []
    seen: set[str] = set()
    # Issue #976: count every normalized Write/Edit target (pre-dedup) so the
    # FailureObservation can surface repeated repair targets.
    write_target_counts: dict[str, int] = {}

    for msg in messages:
        if not isinstance(msg, dict):
            continue
        role = msg.get("role")
        content = msg.get("content")

        if role == "system" and isinstance(content, str) and content.startswith(
            COMPACT_SUMMARY_PREFIX
        ):
            compact_events += 1

        if role != "assistant":
            continue

        iter_count += 1

        raw_calls = msg.get("tool_calls")
        if not isinstance(raw_calls, list):
            continue
        for call in raw_calls:
            if not isinstance(call, dict):
                continue
            name = call.get("name")
            if not isinstance(name, str) or not name:
                continue
            tool_calls[name] = tool_calls.get(name, 0) + 1

            if name not in WRITE_EDIT_TOOLS:
                continue
            args = call.get("arguments")
            if not isinstance(args, dict):
                continue
            raw_path = args.get("path")
            if not isinstance(raw_path, str):
                raw_path = args.get("file_path")
            if not isinstance(raw_path, str):
                continue
            normalized = normalize_path(
                raw_path, workdir,
                workdir_resolved=wdir_resolved, workdir_name=wdir_name
            )
            if normalized is None:
                continue
            write_target_counts[normalized] = write_target_counts.get(normalized, 0) + 1
            if normalized in seen:
                continue
            seen.add(normalized)
            files_modified.append(normalized)

    we_total = sum(tool_calls.get(t, 0) for t in WRITE_EDIT_TOOLS)

    run_id_raw = session.get("id")
    if isinstance(run_id_raw, str) and run_id_raw:
        run_id: str | None = run_id_raw
    else:
        run_id = None

    return {
        "run_id": run_id,
        "iter_count": iter_count,
        "tool_calls": dict(sorted(tool_calls.items())),
        "we_total": we_total,
        "compact_events": compact_events,
        "files_modified": files_modified,
        "write_target_counts": write_target_counts,
    }


# ---------------------------------------------------------------------------
# meta.json
# ---------------------------------------------------------------------------


def _safe_meta_string(data: dict[str, Any], key: str) -> str | None:
    raw = data.get(key)
    if not isinstance(raw, str):
        return None
    raw = raw.strip()
    if not raw:
        return None
    return raw[:128]


def _normalize_failure_authority(raw: str | None) -> str | None:
    if raw is None:
        return None
    normalized = raw.strip().lower().replace("-", "_").replace(" ", "_")
    normalized = FAILURE_AUTHORITY_ALIASES.get(raw.strip().lower(), normalized)
    return normalized if normalized in KNOWN_FAILURE_AUTHORITIES else None


def _safe_bool(data: dict[str, Any], key: str) -> bool | None:
    raw = data.get(key)
    return raw if isinstance(raw, bool) else None


def _read_meta(run_dir: Path) -> dict[str, Any]:
    """Return selected meta.json fields, using None/defaults on failure."""
    fallback = {
        "case": "default",
        "elapsed_s": None,
        "failure_authority": None,
        "pam_variant": "default",
        "postcheck_reason": None,
        "postcheck_success": None,
        "rc": None,
        "task_kind": DEFAULT_TASK_KIND,
        "_task_kind_source": "default",
    }
    candidate = run_dir / "meta.json"
    safe = _safe_regular_file_in(candidate, run_dir, MAX_META_JSON)
    if safe is None:
        if candidate.exists() and not candidate.is_symlink():
            # silently missing is fine; only warn on unusable-but-present
            _warn(f"meta.json unusable: {candidate}")
        return fallback

    try:
        raw = safe.read_text(encoding="utf-8")
    except OSError as e:
        _warn(f"meta.json read error: {e}")
        return fallback

    try:
        data = json.loads(raw)
    except json.JSONDecodeError as e:
        _warn(f"meta.json parse error: {e}")
        return fallback

    if not isinstance(data, dict):
        _warn("meta.json is not an object")
        return fallback

    rc = data.get("rc")
    elapsed_s = data.get("elapsed_s")
    if not isinstance(rc, int) or isinstance(rc, bool):
        rc = None
    if isinstance(elapsed_s, bool):
        elapsed_s = None
    elif isinstance(elapsed_s, int):
        pass
    elif isinstance(elapsed_s, float):
        elapsed_s = int(elapsed_s)
    else:
        elapsed_s = None

    task_kind_source = "task_kind"
    task_kind = _safe_meta_string(data, "task_kind")
    if task_kind is None:
        task_kind_source = "category"
        task_kind = _safe_meta_string(data, "category")
    if task_kind is None or task_kind not in KNOWN_TASK_KINDS:
        task_kind_source = "default"
        task_kind = DEFAULT_TASK_KIND

    return {
        "case": _safe_meta_string(data, "case") or "default",
        "elapsed_s": elapsed_s,
        "failure_authority": _normalize_failure_authority(
            _safe_meta_string(data, "failure_authority")
        ),
        "pam_variant": _safe_meta_string(data, "pam_variant") or "default",
        "postcheck_reason": _safe_meta_string(data, "postcheck_reason"),
        "postcheck_success": _safe_bool(data, "postcheck_success"),
        "rc": rc,
        "task_kind": task_kind,
        "_task_kind_source": task_kind_source,
    }


# ---------------------------------------------------------------------------
# artifact-level postcheck
# ---------------------------------------------------------------------------


def _is_protected_artifact_path(path: str) -> bool:
    clean = path.strip().replace("\\", "/")
    if not clean or "\x00" in clean:
        return True
    while clean.startswith("./"):
        clean = clean[2:]
    if clean in PROTECTED_EXACT_PATHS:
        return True
    if clean.endswith(".log"):
        return True
    return any(clean.startswith(prefix) for prefix in PROTECTED_PREFIXES)


def _artifact_candidates(files_modified: list[str]) -> list[str]:
    out: list[str] = []
    seen: set[str] = set()
    for raw in files_modified:
        if not isinstance(raw, str):
            continue
        path = raw.strip().replace("\\", "/")
        while path.startswith("./"):
            path = path[2:]
        if not path or _is_protected_artifact_path(path) or path in seen:
            continue
        seen.add(path)
        out.append(path)
    return out


def _suffix(path: str) -> str:
    return Path(path).suffix.lower()


def _postcheck_success(task_kind: str, artifacts: list[str]) -> tuple[bool | None, str]:
    # Issue #919 (P2): a pure-answer task is artifact-optional, so it must
    # short-circuit BEFORE the `if not artifacts` failure below — a prose answer
    # with zero artifacts must not be failed.
    if task_kind == "answer_only":
        return None, "answer_only_no_artifact_required"

    if not artifacts:
        return False, "no_user_artifact"

    lowered = [p.lower() for p in artifacts]
    suffixes = {_suffix(p) for p in lowered}

    if task_kind == "coding":
        ok = any(ext in CODE_EXTS or ext in CONFIG_EXTS for ext in suffixes)
        return ok, "coding_artifact" if ok else "missing_coding_artifact"

    if task_kind == "docs":
        ok = any(ext in DOC_EXTS for ext in suffixes) or any(
            p.startswith("docs/") for p in lowered
        )
        return ok, "docs_artifact" if ok else "missing_docs_artifact"

    # Issue #919 (P2): authoring produces a docs-shaped artifact (mirror docs).
    if task_kind == "authoring":
        ok = any(ext in DOC_EXTS for ext in suffixes) or any(
            p.startswith("docs/") for p in lowered
        )
        return ok, "authoring_artifact" if ok else "missing_authoring_artifact"

    if task_kind == "data":
        ok = any(ext in DATA_EXTS for ext in suffixes) or any(
            ext in CODE_EXTS and ("script" in p or "data" in p or "transform" in p)
            for p in lowered
            for ext in [_suffix(p)]
        )
        return ok, "data_artifact" if ok else "missing_data_artifact"

    if task_kind == "research":
        ok = any(
            (ext in DOC_EXTS) and ("research" in p or "brief" in p or "report" in p)
            for p in lowered
            for ext in [_suffix(p)]
        )
        return ok, "research_artifact" if ok else "missing_research_artifact"

    if task_kind == "ops":
        ok = any(
            "runbook" in p
            or "/ops/" in p
            or p.startswith("ops/")
            or p.startswith("scripts/")
            or p.startswith(".github/workflows/")
            for p in lowered
        )
        return ok, "ops_artifact" if ok else "missing_ops_artifact"

    return None, "unknown_task_kind"


# ---------------------------------------------------------------------------
# llm-io.jsonl
# ---------------------------------------------------------------------------


def _read_error_500_count(run_dir: Path) -> int | None:
    logs_dir = run_dir / "logs"
    candidate = logs_dir / "llm-io.jsonl"
    # verify the logs dir itself is not a symlink pointing outside
    try:
        if logs_dir.is_symlink():
            return None
    except OSError:
        return None
    if not logs_dir.exists():
        return None
    safe = _safe_regular_file_in(candidate, run_dir, MAX_LLM_IO_JSONL)
    if safe is None:
        if candidate.exists() and not candidate.is_symlink():
            _warn(f"llm-io.jsonl unusable: {candidate}")
        return None

    count = 0
    try:
        with safe.open("rb") as fh:
            while True:
                raw_line = fh.readline(MAX_LLM_IO_LINE + 1)
                if not raw_line:
                    break
                if len(raw_line) > MAX_LLM_IO_LINE:
                    _warn("llm-io.jsonl: oversized line, skipping remainder")
                    return None
                try:
                    line = raw_line.decode("utf-8", errors="replace").strip()
                except Exception:
                    continue
                if not line:
                    continue
                try:
                    rec = json.loads(line)
                except json.JSONDecodeError:
                    continue
                if not isinstance(rec, dict):
                    continue
                if rec.get("event") != LLM_IO_ERROR_EVENT:
                    continue
                payload = rec.get("payload")
                if not isinstance(payload, dict):
                    continue
                if payload.get("kind") != LLM_IO_ERROR_KIND:
                    continue
                if payload.get("status") == 500:
                    count += 1
    except OSError as e:
        _warn(f"llm-io.jsonl read error: {e}")
        return None
    return count


# ---------------------------------------------------------------------------
# session.json — anvil_score / failure_kind
# ---------------------------------------------------------------------------


def _read_anvil_score(session_data: dict) -> dict | None:
    """Return last_anvil_score dict, or None if absent or not a dict (lossy recovery)."""
    raw = session_data.get("last_anvil_score")
    if not isinstance(raw, dict):
        return None
    return raw


def _read_failure_kind(session_data: dict) -> str | None:
    """Return last_feedback.kind string (snake_case serialised by Rust), or None."""
    last_feedback = session_data.get("last_feedback")
    if not isinstance(last_feedback, dict):
        return None
    kind = last_feedback.get("kind")
    if not isinstance(kind, str) or not kind:
        return None
    return kind


def _read_eval_taxonomy(run_dir: Path) -> dict[str, str] | None:
    """Return the last eval.jsonl evaluation_taxonomy object, if present."""
    logs_dir = run_dir / "logs"
    candidate = logs_dir / "eval.jsonl"
    try:
        if logs_dir.is_symlink():
            return None
    except OSError:
        return None
    if not logs_dir.exists():
        return None
    safe = _safe_regular_file_in(candidate, run_dir, MAX_EVAL_JSONL)
    if safe is None:
        if candidate.exists() and not candidate.is_symlink():
            _warn(f"eval.jsonl unusable: {candidate}")
        return None

    last: dict[str, str] | None = None
    try:
        with safe.open("rb") as fh:
            while True:
                raw_line = fh.readline(MAX_EVAL_JSONL_LINE + 1)
                if not raw_line:
                    break
                if len(raw_line) > MAX_EVAL_JSONL_LINE:
                    _warn("eval.jsonl: oversized line, skipping remainder")
                    return last
                try:
                    line = raw_line.decode("utf-8", errors="replace").strip()
                except Exception:
                    continue
                if not line:
                    continue
                try:
                    rec = json.loads(line)
                except json.JSONDecodeError:
                    continue
                if not isinstance(rec, dict):
                    continue
                taxonomy = rec.get("evaluation_taxonomy")
                if not isinstance(taxonomy, dict):
                    continue
                parsed: dict[str, str] = {}
                for key in (
                    "pam_variant",
                    "task_kind",
                    "anvil_terminal_class",
                    "outcome_agreement",
                    "failure_authority",
                ):
                    value = taxonomy.get(key)
                    if isinstance(value, str) and value.strip():
                        parsed[key] = value.strip()[:128]
                if parsed:
                    last = parsed
    except OSError as e:
        _warn(f"eval.jsonl read error: {e}")
        return None
    return last


def _bounded_string(value: Any, *, limit: int = 128) -> str | None:
    if not isinstance(value, str):
        return None
    value = value.strip()
    return value[:limit] if value else None


def _bounded_string_list(value: Any, *, limit: int = 8) -> list[str]:
    if not isinstance(value, list):
        return []
    out: list[str] = []
    for item in value:
        parsed = _bounded_string(item)
        if parsed is None:
            continue
        out.append(parsed)
        if len(out) >= limit:
            break
    return out


def _objective_kinds_for_task_kind(task_kind: str) -> tuple[str, str]:
    return OBJECTIVE_KINDS_BY_TASK_KIND.get(task_kind, ("unknown", "unknown"))


def _generic_terminal_state(final_outcome: str | None, rc: Any) -> str:
    if final_outcome in GENERIC_TERMINAL_BY_FINAL_OUTCOME:
        return GENERIC_TERMINAL_BY_FINAL_OUTCOME[final_outcome]
    terminal_success = _anvil_terminal_success(rc)
    if terminal_success is True:
        return "completed"
    return "unknown"


def _normalize_generic_terminal_state_from_worker_lifecycle(
    generic_terminal_state: str, projection: dict[str, Any]
) -> str:
    """Use deterministic worker observations to refine legacy terminal labels.

    Older replay logs can end with ``missing_evidence`` even after a runner was
    bound and a rerun failed. Keep the legacy label readable, but route the
    generic lifecycle through evidence failure so recovery analysis does not
    schedule a missing-evidence job for a failed runner.

    Issue #993 (parent #988, Issue E): the inverse case is a *binding-order*
    failure — the evidence deliverable was created (a Node test file, a docs
    document, a data output, research notes) but no evidence runner could be
    bound to it (`package.json`/`scripts.test` missing, no target document, no
    output file, no source notes). That is ``evidence_binding_failed``, not a
    generic ``missing_evidence`` (legacy ``missing_verification``) terminal, so
    recovery materializes the binding and reruns instead of asking for more
    evidence. The ``evidence_created is False`` case (the test author has not
    produced the deliverable yet) is left untouched as ``missing_evidence``.
    """
    if generic_terminal_state != "missing_evidence":
        return generic_terminal_state
    if projection.get("runner_bound") is True:
        if projection.get("evidence_created") is True or projection.get("rerun_passed") is False:
            return "evidence_failed"
        return generic_terminal_state
    if (
        projection.get("runner_bound") is False
        and projection.get("evidence_created") is True
    ):
        return "evidence_binding_failed"
    return generic_terminal_state


def _recovery_job_kind_for_terminal(generic_terminal_state: str) -> str:
    return RECOVERY_JOB_BY_GENERIC_TERMINAL.get(generic_terminal_state, "unknown")


def _read_eval_objective_projection(run_dir: Path) -> dict[str, Any]:
    """Return the last eval.jsonl objective/terminal projection, if present.

    This reader is report-only and best-effort. It intentionally differs from
    `_read_classified_task_kind`, which fails closed for the R5 routing gate.
    """
    logs_dir = run_dir / "logs"
    candidate = logs_dir / "eval.jsonl"
    try:
        if logs_dir.is_symlink():
            return {}
    except OSError:
        return {}
    if not logs_dir.exists():
        return {}
    safe = _safe_regular_file_in(candidate, run_dir, MAX_EVAL_JSONL)
    if safe is None:
        if candidate.exists() and not candidate.is_symlink():
            _warn(f"eval.jsonl unusable: {candidate}")
        return {}

    last: dict[str, Any] = {}
    try:
        with safe.open("rb") as fh:
            while True:
                raw_line = fh.readline(MAX_EVAL_JSONL_LINE + 1)
                if not raw_line:
                    break
                if len(raw_line) > MAX_EVAL_JSONL_LINE:
                    _warn("eval.jsonl: oversized line, skipping objective projection remainder")
                    return last
                try:
                    line = raw_line.decode("utf-8", errors="replace").strip()
                except Exception:
                    continue
                if not line:
                    continue
                try:
                    rec = json.loads(line)
                except json.JSONDecodeError:
                    continue
                if not isinstance(rec, dict):
                    continue

                parsed: dict[str, Any] = {}
                final_outcome = _bounded_string(rec.get("final_outcome"))
                diagnostics = rec.get("terminal_diagnostics")
                if final_outcome is None and isinstance(diagnostics, dict):
                    final_outcome = _bounded_string(diagnostics.get("outcome"))
                if final_outcome is not None:
                    parsed["final_outcome"] = final_outcome

                objective = rec.get("objective_evaluation")
                if isinstance(objective, dict):
                    for key in (
                        "deliverable_kind",
                        "evidence_kind",
                        "generic_terminal_state",
                        "recovery_job_kind",
                    ):
                        value = _bounded_string(objective.get(key))
                        if value is not None:
                            parsed[key] = value

                recovery_strategy_count = rec.get("recovery_strategy_count")
                if isinstance(recovery_strategy_count, int) and not isinstance(
                    recovery_strategy_count, bool
                ):
                    parsed["recovery_strategy_count"] = max(0, recovery_strategy_count)
                recovery_strategies = _bounded_string_list(rec.get("recovery_strategies"))
                if recovery_strategies:
                    parsed["recovery_strategies"] = recovery_strategies

                worker_lifecycle = rec.get("worker_lifecycle")
                if isinstance(worker_lifecycle, dict):
                    for key in WORKER_LIFECYCLE_STRING_FIELDS:
                        value = _bounded_string(worker_lifecycle.get(key))
                        if value is not None:
                            parsed[key] = value
                    for key in WORKER_LIFECYCLE_BOOL_FIELDS:
                        value = worker_lifecycle.get(key)
                        if isinstance(value, bool):
                            parsed[key] = value
                    for key in WORKER_LIFECYCLE_INT_FIELDS:
                        value = worker_lifecycle.get(key)
                        if isinstance(value, int) and not isinstance(value, bool):
                            parsed[key] = max(0, value)

                if parsed:
                    last = parsed
    except OSError as e:
        _warn(f"eval.jsonl read error: {e}")
        return {}
    return last


def _read_failure_inputs(run_dir: Path) -> dict[str, Any]:
    """Return failure-observation signals from the last eval.jsonl record.

    Report-only and best-effort (mirrors :func:`_read_eval_objective_projection`,
    NOT the fail-closed :func:`_read_classified_task_kind`). Pulls from the
    production ``terminal_diagnostics`` block (``verifier_status``,
    ``last_failure_signature``, ``classification``, ``missing_obligations`` and
    per-obligation ``failure_domain``) plus the top-level ``verify_commands``
    list. An optional ``failure_observation`` block carries fields the eval log
    does not derive itself yet (``evidence_exit_code``, stdout/stderr excerpts,
    ``invalid_proposal_count``, ``repair_count``, ``deterministic_operator_hit``);
    these are read defensively and override the derived defaults when present.
    Absent/unreadable inputs yield ``{}`` so the classifier degrades cleanly.
    """
    logs_dir = run_dir / "logs"
    candidate = logs_dir / "eval.jsonl"
    try:
        if logs_dir.is_symlink():
            return {}
    except OSError:
        return {}
    if not logs_dir.exists():
        return {}
    safe = _safe_regular_file_in(candidate, run_dir, MAX_EVAL_JSONL)
    if safe is None:
        return {}

    last: dict[str, Any] = {}
    try:
        with safe.open("rb") as fh:
            while True:
                raw_line = fh.readline(MAX_EVAL_JSONL_LINE + 1)
                if not raw_line:
                    break
                if len(raw_line) > MAX_EVAL_JSONL_LINE:
                    _warn("eval.jsonl: oversized line, skipping failure inputs remainder")
                    return last
                try:
                    line = raw_line.decode("utf-8", errors="replace").strip()
                except Exception:
                    continue
                if not line:
                    continue
                try:
                    rec = json.loads(line)
                except json.JSONDecodeError:
                    continue
                if not isinstance(rec, dict):
                    continue

                parsed: dict[str, Any] = {}
                diagnostics = rec.get("terminal_diagnostics")
                if isinstance(diagnostics, dict):
                    for src, dst in (
                        ("verifier_status", "verifier_status"),
                        ("last_failure_signature", "last_failure_signature"),
                        ("classification", "terminal_classification"),
                    ):
                        value = _bounded_string(diagnostics.get(src))
                        if value is not None:
                            parsed[dst] = value
                    missing = _bounded_string_list(diagnostics.get("missing_obligations"))
                    if missing:
                        parsed["missing_obligations"] = missing
                    obligations = diagnostics.get("obligations")
                    if isinstance(obligations, list):
                        domains: list[str] = []
                        for ob in obligations:
                            if not isinstance(ob, dict):
                                continue
                            domain = _bounded_string(ob.get("failure_domain"))
                            if domain is not None and domain not in domains:
                                domains.append(domain)
                            if len(domains) >= 8:
                                break
                        if domains:
                            parsed["obligation_failure_domains"] = domains

                verify_commands = _bounded_string_list(rec.get("verify_commands"))
                if verify_commands:
                    parsed["verify_commands"] = verify_commands

                override = rec.get("failure_observation")
                if isinstance(override, dict):
                    for key in ("evidence_command", "stdout_excerpt", "stderr_excerpt"):
                        value = _bounded_string(override.get(key), limit=256)
                        if value is not None:
                            parsed[key] = value
                    for key in (
                        "evidence_exit_code",
                        "invalid_proposal_count",
                        "repair_count",
                    ):
                        value = override.get(key)
                        if isinstance(value, int) and not isinstance(value, bool):
                            parsed[key] = value
                    hit = override.get("deterministic_operator_hit")
                    if isinstance(hit, bool):
                        parsed["deterministic_operator_hit"] = hit

                if parsed:
                    last = parsed
    except OSError as e:
        _warn(f"eval.jsonl failure inputs read error: {e}")
        return {}
    return last


def _has_worker_lifecycle_projection(projection: dict[str, Any]) -> bool:
    return any(
        key in projection
        for key in (
            *WORKER_LIFECYCLE_STRING_FIELDS,
            *WORKER_LIFECYCLE_BOOL_FIELDS,
            *WORKER_LIFECYCLE_INT_FIELDS,
        )
    )


def _derive_lifecycle_failure_stage(
    *,
    task_kind_misroute: bool,
    projection: dict[str, Any],
    generic_terminal_state: str,
    postcheck_success: bool | None,
) -> str:
    explicit = projection.get("lifecycle_failure_stage")
    if isinstance(explicit, str) and explicit in KNOWN_LIFECYCLE_FAILURE_STAGES:
        return explicit
    if task_kind_misroute:
        return "classification"
    if projection.get("deliverable_created") is False:
        return "deliverable"
    if projection.get("evidence_created") is False:
        return "evidence_authoring"
    if projection.get("runner_bound") is False:
        return "runner_binding"
    if projection.get("diagnostic_classified") is False:
        return "diagnostic_classification"
    if generic_terminal_state == "missing_deliverable":
        return "deliverable"
    if generic_terminal_state == "missing_evidence":
        return "evidence_authoring"
    if generic_terminal_state in {"evidence_runner_missing", "evidence_binding_failed"}:
        return "runner_binding"
    if generic_terminal_state in {
        "evidence_repair_exhausted",
        "evidence_repair_safe_stop",
    }:
        return "repair"
    if projection.get("rerun_passed") is False:
        return "rerun"
    if generic_terminal_state == "evidence_failed":
        return "rerun"
    if generic_terminal_state == "completed" and postcheck_success is True:
        return "completed"
    return "unknown"


def _read_classified_task_kind(run_dir: Path) -> str | None:
    """Return the last eval.jsonl top-level ``classified_task_kind`` (Issue #925).

    This is the agent's CLASSIFIED task_kind, surfaced from the agent layer and
    kept DISTINCT from the eval-heuristic ``evaluation_taxonomy.task_kind``; the
    existing :func:`_read_eval_taxonomy` (the 5-key allowlist) is intentionally
    left untouched.

    Fail-closed (DR4-001/DR4-003): only one of the known 5 kinds is accepted.
    Anything else — absent, wrong type, unknown/over-long string, malformed JSON,
    oversized line, symlinked or unreadable file — yields ``None`` so the R5 gate
    treats it as a *missing* classification rather than silently passing. This
    function never raises and never fails open.
    """
    logs_dir = run_dir / "logs"
    candidate = logs_dir / "eval.jsonl"
    try:
        if logs_dir.is_symlink():
            return None
    except OSError:
        return None
    if not logs_dir.exists():
        return None
    safe = _safe_regular_file_in(candidate, run_dir, MAX_EVAL_JSONL)
    if safe is None:
        return None

    # The last record carrying the field is authoritative (turn-level log).
    # Fail-closed (CB-001): ANY structural corruption — oversized line, decode
    # failure, malformed JSON, or a non-object record — abandons the whole file
    # and returns None, rather than trusting a stale value read before the
    # corruption. A truncated/DoS/tampered tail must NOT let an earlier valid
    # classification slip through as a pass (DR4-003).
    last_raw: Any = None
    try:
        with safe.open("rb") as fh:
            while True:
                raw_line = fh.readline(MAX_EVAL_JSONL_LINE + 1)
                if not raw_line:
                    break
                if len(raw_line) > MAX_EVAL_JSONL_LINE:
                    _warn("eval.jsonl: oversized line, failing classified read closed")
                    return None
                try:
                    line = raw_line.decode("utf-8", errors="strict").strip()
                except UnicodeDecodeError:
                    _warn("eval.jsonl: undecodable line, failing classified read closed")
                    return None
                if not line:
                    continue
                try:
                    rec = json.loads(line)
                except json.JSONDecodeError:
                    _warn("eval.jsonl: malformed JSON line, failing classified read closed")
                    return None
                if not isinstance(rec, dict):
                    _warn("eval.jsonl: non-object record, failing classified read closed")
                    return None
                if "classified_task_kind" in rec:
                    last_raw = rec.get("classified_task_kind")
    except OSError as e:
        _warn(f"eval.jsonl read error: {e}")
        return None

    if isinstance(last_raw, str):
        stripped = last_raw.strip()
        # Membership in the 5-kind set is the strict validator (it also bounds
        # length: the longest known kind is "research"). Unknown => None.
        if stripped in KNOWN_TASK_KINDS:
            return stripped
    return None


def _anvil_terminal_success(rc: Any) -> bool | None:
    if isinstance(rc, bool) or not isinstance(rc, int):
        return None
    return rc == 0


def _anvil_terminal_class(rc: Any) -> str:
    terminal_success = _anvil_terminal_success(rc)
    if terminal_success is None:
        return "unknown"
    return "success" if terminal_success else "non_success"


def _outcome_agreement(rc: Any, postcheck_success: Any) -> str:
    terminal_success = _anvil_terminal_success(rc)
    if terminal_success is None or not isinstance(postcheck_success, bool):
        return "unknown"
    if terminal_success and postcheck_success:
        return "true_positive"
    if terminal_success and not postcheck_success:
        return "false_positive"
    if not terminal_success and postcheck_success:
        return "false_negative"
    return "true_negative"


def _score_int(score: dict | None, key: str) -> int | None:
    if not isinstance(score, dict):
        return None
    raw = score.get(key)
    if isinstance(raw, bool) or not isinstance(raw, int):
        return None
    return raw if raw >= 0 else None


def _looks_like_test_path(path: str) -> bool:
    lower = path.lower().replace("\\", "/")
    name = Path(lower).name
    return (
        lower.startswith("tests/")
        or "/tests/" in lower
        or lower.startswith("test/")
        or "/test/" in lower
        or lower.startswith("tmp-tests/")
        or name.startswith("test_")
        or name.endswith("_test.py")
        or name.endswith("_test.rs")
        or name.endswith(".test.js")
        or name.endswith(".test.ts")
        or name.endswith(".test.tsx")
        or name.endswith(".spec.js")
        or name.endswith(".spec.ts")
        or name.endswith(".spec.tsx")
    )


_SETUP_BASENAMES = {
    "package.json",
    "package-lock.json",
    "pnpm-lock.yaml",
    "yarn.lock",
    "cargo.toml",
    "cargo.lock",
    "requirements.txt",
    "pyproject.toml",
    "setup.py",
    "setup.cfg",
    "go.mod",
    "go.sum",
    "tsconfig.json",
    "pom.xml",
    "build.gradle",
    "makefile",
    "dockerfile",
}


def _looks_like_setup_path(path: str) -> bool:
    """True for dependency/build/config manifests (setup-side artifacts)."""
    lower = path.lower().replace("\\", "/")
    name = Path(lower).name
    if name in _SETUP_BASENAMES:
        return True
    return Path(lower).suffix in CONFIG_EXTS


def _looks_like_impl_path(path: str) -> bool:
    """True for a source file that is neither a test nor a setup manifest."""
    if _looks_like_test_path(path) or _looks_like_setup_path(path):
        return False
    return Path(path.lower()).suffix in CODE_EXTS


def _generated_test_bug_evidence(
    failure_kind: str | None,
    anvil_score: dict | None,
    artifact_files: list[str],
) -> bool:
    test_changed = _score_int(anvil_score, "test_files_changed")
    impl_changed = _score_int(anvil_score, "implementation_files_changed")
    setup_changed = _score_int(anvil_score, "setup_files_changed")
    test_failures = _score_int(anvil_score, "test_failure_count")
    tests_passed = anvil_score.get("tests_passed") if isinstance(anvil_score, dict) else None
    score_points_at_tests = (
        (test_changed or 0) > 0
        and (impl_changed or 0) == 0
        and (setup_changed or 0) == 0
        and (test_failures is None or test_failures > 0)
        and (tests_passed is None or tests_passed is False)
    )
    paths_point_at_tests = bool(artifact_files) and all(
        _looks_like_test_path(path) for path in artifact_files
    )
    return failure_kind == "test_failure" and (score_points_at_tests or paths_point_at_tests)


def _classify_failure_authority(
    *,
    meta_authority: str | None,
    eval_taxonomy: dict[str, str] | None,
    rc: Any,
    postcheck_success: bool | None,
    postcheck_reason: str,
    failure_kind: str | None,
    anvil_score: dict | None,
    artifact_files: list[str],
) -> str:
    if meta_authority is not None:
        return meta_authority
    if _generated_test_bug_evidence(failure_kind, anvil_score, artifact_files):
        return "generated_test_bug"

    eval_authority = _normalize_failure_authority(
        eval_taxonomy.get("failure_authority") if eval_taxonomy else None
    )
    if eval_authority is not None:
        return eval_authority

    agreement = _outcome_agreement(rc, postcheck_success)
    if agreement == "true_positive":
        return "success"

    if failure_kind == "no_verifier_available":
        return "verifier_setup"
    if failure_kind in {"tool_protocol_failure", "no_tool_call", "no_repo_progress"}:
        return "contract_extraction"
    if failure_kind in {"edit_failure", "timeout", "unsafe_command_blocked"}:
        return "repair_routing"
    if failure_kind in {"compile_error", "type_error", "lint_failure", "test_failure"}:
        return "implementation_bug"

    if postcheck_reason == "no_user_artifact":
        return "contract_extraction"
    if postcheck_reason.startswith("missing_") and postcheck_reason.endswith("_artifact"):
        return "artifact_classification"
    if agreement in {"false_positive", "false_negative", "true_negative"}:
        return "implementation_bug"
    return "unknown"


# ---------------------------------------------------------------------------
# FailureObservation replay classifier (Issue #976 / parent #974, Issue B)
# ---------------------------------------------------------------------------


def _signature_points_at_test_or_setup(
    signature: str | None, classification: str | None
) -> bool:
    """True when a failure signature/classification names a test- or setup-side
    cause (import error, missing manifest, missing test, ...)."""
    for text in (signature, classification):
        if not isinstance(text, str) or not text:
            continue
        low = text.lower()
        if any(marker in low for marker in TEST_OR_SETUP_FAILURE_MARKERS):
            return True
    return False


def _test_or_setup_cause(
    *,
    signature: str | None,
    classification: str | None,
    failure_kind: str | None,
    anvil_score: dict | None,
) -> bool:
    """Whether the failing diagnostic points at test/setup rather than impl."""
    if _signature_points_at_test_or_setup(signature, classification):
        return True
    test_failures = _score_int(anvil_score, "test_failure_count")
    test_changed = _score_int(anvil_score, "test_files_changed")
    if (test_failures or 0) > 0 and (test_changed or 0) == 0:
        return True
    if failure_kind == "test_failure" and (test_changed or 0) == 0:
        return True
    return False


def _impl_only_repaired(
    anvil_score: dict | None, repeated_targets: list[str]
) -> bool:
    """Whether repair effort only touched implementation files."""
    impl_changed = _score_int(anvil_score, "implementation_files_changed")
    test_changed = _score_int(anvil_score, "test_files_changed")
    setup_changed = _score_int(anvil_score, "setup_files_changed")
    score_impl_only = (
        (impl_changed or 0) > 0
        and (test_changed or 0) == 0
        and (setup_changed or 0) == 0
    )
    repeated_impl = any(_looks_like_impl_path(p) for p in repeated_targets)
    repeated_other = any(
        _looks_like_test_path(p) or _looks_like_setup_path(p) for p in repeated_targets
    )
    path_impl_only = repeated_impl and not repeated_other
    return score_impl_only or path_impl_only


def _evidence_runner_executed(
    *,
    generic_terminal_state: str,
    projection: dict[str, Any],
    failure_inputs: dict[str, Any],
    anvil_score: dict | None,
) -> bool:
    """Whether an evidence runner (test/build/command) actually executed."""
    if generic_terminal_state in EVIDENCE_RAN_TERMINAL_STATES:
        return True
    if projection.get("runner_bound") is True:
        return True
    if failure_inputs.get("verify_commands"):
        return True
    status = failure_inputs.get("verifier_status")
    if isinstance(status, str) and status.strip().lower() in VERIFIER_STATUS_EXECUTED:
        return True
    if isinstance(anvil_score, dict):
        if isinstance(anvil_score.get("build_passed"), bool):
            return True
        if isinstance(anvil_score.get("tests_passed"), bool):
            return True
    return False


def _runner_present_but_failed(
    *,
    generic_terminal_state: str,
    projection: dict[str, Any],
    failure_inputs: dict[str, Any],
    anvil_score: dict | None,
) -> bool:
    """Detector (Issue #976 acceptance): a ``verifier_missing`` terminal that
    actually had a runner bound which ran and failed. These runs must route to
    evidence-failed recovery, not a missing-evidence/runner job."""
    if generic_terminal_state != "evidence_runner_missing":
        return False
    status = failure_inputs.get("verifier_status")
    if isinstance(status, str) and status.strip().lower() in VERIFIER_STATUS_FAILED:
        return True
    if failure_inputs.get("verify_commands"):
        return True
    if (
        projection.get("runner_bound") is True
        and projection.get("rerun_passed") is False
    ):
        return True
    if isinstance(anvil_score, dict):
        if anvil_score.get("build_passed") is False or anvil_score.get("tests_passed") is False:
            return True
        if (_score_int(anvil_score, "compile_error_count") or 0) > 0:
            return True
        if (_score_int(anvil_score, "test_failure_count") or 0) > 0:
            return True
    return False


def _tool_protocol_error(
    *, generic_terminal_state: str, legacy_terminal_state: str, failure_kind: str | None
) -> bool:
    """Whether the run failed on tool-call protocol (format/no-tool) rather than
    on evidence."""
    if generic_terminal_state == "model_output_failure":
        return True
    if failure_kind in {"tool_protocol_failure", "tool_call_format_error", "no_tool_call"}:
        return True
    if legacy_terminal_state in {
        "tool_call_format_error",
        "no_tool_calls",
        "empty_responses",
    }:
        return True
    return False


def _deterministic_operator_hit(
    failure_inputs: dict[str, Any], recovery_strategies: list[str]
) -> bool:
    explicit = failure_inputs.get("deterministic_operator_hit")
    if isinstance(explicit, bool):
        return explicit
    for strategy in recovery_strategies:
        if not isinstance(strategy, str):
            continue
        low = strategy.lower()
        if any(marker in low for marker in DETERMINISTIC_OPERATOR_MARKERS):
            return True
    return False


def _failure_class_for(
    generic_terminal_state: str, runner_present_but_failed: bool
) -> str:
    base = FAILURE_CLASS_BY_GENERIC_TERMINAL.get(generic_terminal_state, "unknown")
    if base == "evidence_runner_missing" and runner_present_but_failed:
        return "evidence_failed"
    return base


def _target_role_for(
    *,
    failure_class: str,
    repair_should_target_test_or_setup: bool,
    wrong_target_repair: bool,
    test_or_setup_cause: bool,
    obligation_failure_domains: list[str],
) -> str:
    if failure_class == "none":
        return "none"
    if failure_class == "missing_deliverable":
        return "deliverable"
    if failure_class == "missing_evidence":
        return "evidence"
    if failure_class == "tool_protocol_failure":
        return "tool_protocol"
    if repair_should_target_test_or_setup or wrong_target_repair:
        return "test_or_setup"
    if failure_class in {"evidence_failed", "recovery_exhausted"}:
        # The failure markers (import/module/dependency/missing-test) do not
        # disambiguate a test-side from a setup-side cause, so stay honest with
        # the combined role rather than over-claiming "test".
        return "test_or_setup" if test_or_setup_cause else "implementation"
    for domain in obligation_failure_domains:
        low = domain.lower()
        if "test" in low:
            return "test"
        if "setup" in low or "manifest" in low or "dependency" in low:
            return "setup"
        if "deliverable" in low or "artifact" in low:
            return "deliverable"
    return "unknown"


def _build_failure_observation(
    *,
    generic_terminal_state: str,
    legacy_terminal_state: str,
    postcheck_success: bool | None,
    artifact_files: list[str],
    write_target_counts: dict[str, int],
    evidence_kind: str,
    failure_kind: str | None,
    anvil_score: dict | None,
    recovery_strategy_count: int,
    recovery_strategies: list[str],
    projection: dict[str, Any],
    failure_inputs: dict[str, Any],
) -> dict[str, Any]:
    """Structure a single run's failure for replay classification.

    Pure projection over already-parsed inputs (no I/O). Successful runs get
    ``failure_class == "none"`` with the transition flags all False, so the
    aggregate report can use every analyzed run as a rate denominator.
    """
    repeated_targets = sorted(
        path for path, count in write_target_counts.items() if count >= 2
    )[:8]

    signature = failure_inputs.get("last_failure_signature")
    if not isinstance(signature, str) or not signature:
        diag_class = projection.get("diagnostic_class")
        signature = diag_class if isinstance(diag_class, str) and diag_class else None
    classification = failure_inputs.get("terminal_classification")
    first_failing_diagnostic = signature or (
        classification if isinstance(classification, str) else None
    )

    verify_commands = failure_inputs.get("verify_commands")
    evidence_command = failure_inputs.get("evidence_command")
    if not isinstance(evidence_command, str) and isinstance(verify_commands, list) and verify_commands:
        evidence_command = verify_commands[0]
    if not isinstance(evidence_command, str):
        evidence_command = None

    evidence_exit_code = failure_inputs.get("evidence_exit_code")
    if not isinstance(evidence_exit_code, int) or isinstance(evidence_exit_code, bool):
        evidence_exit_code = None
    stdout_excerpt = failure_inputs.get("stdout_excerpt")
    stdout_excerpt = stdout_excerpt if isinstance(stdout_excerpt, str) else None
    stderr_excerpt = failure_inputs.get("stderr_excerpt")
    stderr_excerpt = stderr_excerpt if isinstance(stderr_excerpt, str) else None

    repair_count_override = failure_inputs.get("repair_count")
    repair_count = (
        repair_count_override
        if isinstance(repair_count_override, int)
        and not isinstance(repair_count_override, bool)
        and repair_count_override >= 0
        else recovery_strategy_count
    )
    invalid_proposal_count = failure_inputs.get("invalid_proposal_count")
    if (
        not isinstance(invalid_proposal_count, int)
        or isinstance(invalid_proposal_count, bool)
        or invalid_proposal_count < 0
    ):
        invalid_proposal_count = 0

    missing_obligations = failure_inputs.get("missing_obligations")
    missing_obligations = (
        missing_obligations if isinstance(missing_obligations, list) else []
    )
    obligation_failure_domains = failure_inputs.get("obligation_failure_domains")
    obligation_failure_domains = (
        obligation_failure_domains
        if isinstance(obligation_failure_domains, list)
        else []
    )

    tool_protocol_error = _tool_protocol_error(
        generic_terminal_state=generic_terminal_state,
        legacy_terminal_state=legacy_terminal_state,
        failure_kind=failure_kind,
    )
    runner_present_but_failed = _runner_present_but_failed(
        generic_terminal_state=generic_terminal_state,
        projection=projection,
        failure_inputs=failure_inputs,
        anvil_score=anvil_score,
    )
    failure_class = _failure_class_for(generic_terminal_state, runner_present_but_failed)
    if failure_class not in KNOWN_FAILURE_CLASSES:
        failure_class = "unknown"

    test_or_setup_cause = _test_or_setup_cause(
        signature=signature,
        classification=classification,
        failure_kind=failure_kind,
        anvil_score=anvil_score,
    )
    impl_only_repaired = _impl_only_repaired(anvil_score, repeated_targets)
    repaired_wrong_role = impl_only_repaired and test_or_setup_cause
    repair_should_target_test_or_setup = (
        repaired_wrong_role and generic_terminal_state in REPAIR_TERMINAL_STATES
    )
    wrong_target_repair = (
        repaired_wrong_role and generic_terminal_state in EVIDENCE_RAN_TERMINAL_STATES
    )

    evidence_runner_executed = _evidence_runner_executed(
        generic_terminal_state=generic_terminal_state,
        projection=projection,
        failure_inputs=failure_inputs,
        anvil_score=anvil_score,
    )
    deterministic_operator_hit = _deterministic_operator_hit(
        failure_inputs, recovery_strategies
    )

    consecutive_no_progress = _score_int(anvil_score, "consecutive_no_progress_turns")
    same_diagnostic_repeated = (
        first_failing_diagnostic is not None
        and generic_terminal_state
        in (EVIDENCE_RAN_TERMINAL_STATES | {"evidence_runner_missing"})
        and (
            (consecutive_no_progress or 0) >= 2
            or repair_count >= 2
            or bool(repeated_targets)
        )
    )

    target_role = _target_role_for(
        failure_class=failure_class,
        repair_should_target_test_or_setup=repair_should_target_test_or_setup,
        wrong_target_repair=wrong_target_repair,
        test_or_setup_cause=test_or_setup_cause,
        obligation_failure_domains=obligation_failure_domains,
    )
    if target_role not in KNOWN_TARGET_ROLES:
        target_role = "unknown"

    return {
        "failure_class": failure_class,
        "target_role": target_role,
        "terminal_state": generic_terminal_state,
        "legacy_terminal_state": legacy_terminal_state,
        "postcheck_success": postcheck_success,
        "generated_file_count": len(artifact_files),
        "repair_count": repair_count,
        "invalid_proposal_count": invalid_proposal_count,
        "first_failing_diagnostic": first_failing_diagnostic,
        "evidence_kind": evidence_kind,
        "evidence_command": evidence_command,
        "evidence_exit_code": evidence_exit_code,
        "stdout_excerpt": stdout_excerpt,
        "stderr_excerpt": stderr_excerpt,
        "missing_deliverable": failure_class == "missing_deliverable",
        "missing_evidence": failure_class == "missing_evidence",
        "missing_obligations": missing_obligations,
        "repeated_targets": repeated_targets,
        "tool_protocol_error": tool_protocol_error,
        "runner_present_but_failed": runner_present_but_failed,
        "repair_should_target_test_or_setup": repair_should_target_test_or_setup,
        "wrong_target_repair": wrong_target_repair,
        "same_diagnostic_repeated": same_diagnostic_repeated,
        "evidence_runner_executed": evidence_runner_executed,
        "deterministic_operator_hit": deterministic_operator_hit,
    }


# ---------------------------------------------------------------------------
# llm-io.jsonl — token usage
# ---------------------------------------------------------------------------


def _read_token_usage(run_dir: Path) -> tuple[int | None, int | None]:
    """Return (prompt_tokens_total, completion_tokens_total) from llm-io.jsonl.

    Accumulates across all ollama.generate.reply_final / ollama.chat.reply_final
    events. Returns (None, None) when the log is absent, unreadable, or contains
    no token fields.
    """
    logs_dir = run_dir / "logs"
    candidate = logs_dir / "llm-io.jsonl"
    try:
        if logs_dir.is_symlink():
            return None, None
    except OSError:
        return None, None
    if not logs_dir.exists():
        return None, None
    safe = _safe_regular_file_in(candidate, run_dir, MAX_LLM_IO_JSONL)
    if safe is None:
        if candidate.exists() and not candidate.is_symlink():
            _warn(f"llm-io.jsonl unusable for token usage: {candidate}")
        return None, None

    prompt_total: int | None = None
    completion_total: int | None = None
    try:
        with safe.open("rb") as fh:
            while True:
                raw_line = fh.readline(MAX_LLM_IO_LINE + 1)
                if not raw_line:
                    break
                if len(raw_line) > MAX_LLM_IO_LINE:
                    _warn("llm-io.jsonl: oversized line, skipping remainder (token usage)")
                    return None, None
                try:
                    line = raw_line.decode("utf-8", errors="replace").strip()
                except Exception:
                    continue
                if not line:
                    continue
                try:
                    rec = json.loads(line)
                except json.JSONDecodeError:
                    continue
                if not isinstance(rec, dict):
                    continue
                if rec.get("event") not in LLM_IO_REPLY_EVENTS:
                    continue
                payload = rec.get("payload")
                if not isinstance(payload, dict):
                    continue
                p_tok = payload.get("prompt_tokens")
                c_tok = payload.get("completion_tokens")
                if isinstance(p_tok, int) and not isinstance(p_tok, bool) and p_tok >= 0:
                    prompt_total = (prompt_total or 0) + p_tok
                if isinstance(c_tok, int) and not isinstance(c_tok, bool) and c_tok >= 0:
                    completion_total = (completion_total or 0) + c_tok
    except OSError as e:
        _warn(f"llm-io.jsonl read error (token usage): {e}")
        return None, None
    return prompt_total, completion_total


# ---------------------------------------------------------------------------
# page.tsx
# ---------------------------------------------------------------------------


def _read_page_tsx_keywords(run_dir: Path, workdir: Path | None) -> bool | None:
    if workdir is None:
        return None
    candidate = workdir / "src" / "app" / "page.tsx"
    # Per spec: if the file itself is a symlink, treat as missing.
    try:
        if candidate.is_symlink():
            _warn(f"page.tsx is a symlink, skipping: {candidate}")
            return None
    except OSError:
        return None
    safe = _safe_regular_file_in(candidate, run_dir, MAX_PAGE_TSX)
    if safe is None:
        if candidate.exists():
            _warn(f"page.tsx unusable: {candidate}")
        return None
    try:
        text = safe.read_text(encoding="utf-8", errors="replace")
    except OSError as e:
        _warn(f"page.tsx read error: {e}")
        return None
    lower = text.lower()
    for kw in GAME_KEYWORDS_V1:
        if kw in lower:
            return True
    return False


# ---------------------------------------------------------------------------
# main
# ---------------------------------------------------------------------------


def _resolve_run_dir(raw: str) -> Path:
    """Canonicalize raw run-dir argument. Exits on failure per spec."""
    path = Path(raw)
    if path.is_symlink():
        print(f"error: run-dir must not be a symlink: {raw}", file=sys.stderr)
        sys.exit(2)
    try:
        resolved = path.resolve(strict=True)
    except (OSError, RuntimeError):
        print(f"error: run-dir not found: {raw}", file=sys.stderr)
        sys.exit(2)
    if not resolved.is_dir():
        print(f"error: run-dir is not a directory: {raw}", file=sys.stderr)
        sys.exit(2)
    return resolved


def _resolve_workdir(run_dir: Path) -> Path | None:
    candidate = run_dir / "workdir"
    try:
        if not candidate.exists():
            return None
        resolved = candidate.resolve(strict=True)
    except (OSError, RuntimeError):
        return None
    if not resolved.is_relative_to(run_dir):
        return None
    # must be a directory (regular or via symlink whose target is a dir)
    try:
        st = resolved.stat()
    except OSError:
        return None
    if not stat.S_ISDIR(st.st_mode):
        return None
    return resolved


def _load_session(run_dir: Path) -> dict[str, Any]:
    candidate = run_dir / "session.json"
    if candidate.is_symlink():
        print(
            f"error: session.json must not be a symlink: {candidate}",
            file=sys.stderr,
        )
        sys.exit(2)
    if not candidate.exists():
        print(f"error: session.json not found: {candidate}", file=sys.stderr)
        sys.exit(2)
    try:
        resolved = candidate.resolve(strict=True)
    except (OSError, RuntimeError):
        print(f"error: session.json cannot be resolved: {candidate}", file=sys.stderr)
        sys.exit(2)
    if not resolved.is_relative_to(run_dir):
        print(
            f"error: session.json resolves outside run-dir: {resolved}",
            file=sys.stderr,
        )
        sys.exit(2)
    if not _is_regular_file(resolved):
        print(
            f"error: session.json is not a regular file: {resolved}",
            file=sys.stderr,
        )
        sys.exit(2)
    try:
        size = resolved.stat().st_size
    except OSError as e:
        print(f"error: session.json stat failed: {e}", file=sys.stderr)
        sys.exit(3)
    if size > MAX_SESSION_JSON:
        print(
            f"error: session.json exceeds size limit ({size} > {MAX_SESSION_JSON})",
            file=sys.stderr,
        )
        sys.exit(3)
    try:
        raw = resolved.read_text(encoding="utf-8")
    except (OSError, UnicodeDecodeError) as e:
        print(f"error: session.json read failed: {e}", file=sys.stderr)
        sys.exit(3)
    try:
        data = json.loads(raw)
    except json.JSONDecodeError as e:
        print(f"error: session.json parse error: {e}", file=sys.stderr)
        sys.exit(3)
    if not isinstance(data, dict):
        print("error: session.json must be a JSON object", file=sys.stderr)
        sys.exit(3)
    if "messages" not in data or not isinstance(data["messages"], list):
        print("error: session.json must contain 'messages' list", file=sys.stderr)
        sys.exit(3)
    return data


def main(argv: list[str]) -> int:
    if len(argv) < 2:
        print("usage: analyze_run.py <run-dir>", file=sys.stderr)
        return 1

    run_dir = _resolve_run_dir(argv[1])
    session = _load_session(run_dir)
    workdir = _resolve_workdir(run_dir)

    try:
        session_metrics = _analyze_session(session, workdir)
    except ValueError as e:
        print(f"error: {e}", file=sys.stderr)
        return 3

    meta = _read_meta(run_dir)
    error_500_count = _read_error_500_count(run_dir)
    page_tsx_has_keywords = _read_page_tsx_keywords(run_dir, workdir)
    anvil_score = _read_anvil_score(session)
    failure_kind = _read_failure_kind(session)
    eval_log_taxonomy = _read_eval_taxonomy(run_dir)
    eval_objective_projection = _read_eval_objective_projection(run_dir)
    failure_inputs = _read_failure_inputs(run_dir)
    token_prompt, token_completion = _read_token_usage(run_dir)

    files_modified = session_metrics["files_modified"]
    artifact_files = _artifact_candidates(files_modified)
    task_kind = meta["task_kind"]
    if meta.get("_task_kind_source") == "default" and eval_log_taxonomy:
        taxonomy_task_kind = eval_log_taxonomy.get("task_kind")
        if taxonomy_task_kind in KNOWN_TASK_KINDS:
            task_kind = taxonomy_task_kind
    pam_variant = meta["pam_variant"]
    if pam_variant == "default" and eval_log_taxonomy:
        taxonomy_pam_variant = eval_log_taxonomy.get("pam_variant")
        if taxonomy_pam_variant in {"pam_on", "pam_off"}:
            pam_variant = taxonomy_pam_variant
    deliverable_kind, evidence_kind = _objective_kinds_for_task_kind(task_kind)
    deliverable_kind = eval_objective_projection.get(
        "deliverable_kind", deliverable_kind
    )
    evidence_kind = eval_objective_projection.get("evidence_kind", evidence_kind)
    final_outcome = eval_objective_projection.get("final_outcome")
    legacy_terminal_state = (
        final_outcome
        if isinstance(final_outcome, str) and final_outcome
        else ("done" if _anvil_terminal_success(meta["rc"]) is True else "unknown")
    )
    raw_generic_terminal_state = eval_objective_projection.get(
        "generic_terminal_state",
        _generic_terminal_state(final_outcome, meta["rc"]),
    )
    generic_terminal_state = _normalize_generic_terminal_state_from_worker_lifecycle(
        raw_generic_terminal_state, eval_objective_projection
    )
    if generic_terminal_state not in KNOWN_GENERIC_TERMINAL_STATES:
        generic_terminal_state = "unknown"
    if generic_terminal_state != raw_generic_terminal_state:
        recovery_job_kind = _recovery_job_kind_for_terminal(generic_terminal_state)
    else:
        recovery_job_kind = eval_objective_projection.get(
            "recovery_job_kind",
            _recovery_job_kind_for_terminal(generic_terminal_state),
        )
    recovery_strategy_count = eval_objective_projection.get("recovery_strategy_count", 0)
    if not isinstance(recovery_strategy_count, int) or isinstance(
        recovery_strategy_count, bool
    ):
        recovery_strategy_count = 0
    recovery_strategies = eval_objective_projection.get("recovery_strategies", [])
    if not isinstance(recovery_strategies, list):
        recovery_strategies = []

    if isinstance(meta.get("postcheck_success"), bool):
        postcheck_success = meta["postcheck_success"]
        postcheck_reason = meta["postcheck_reason"] or "meta_postcheck"
    else:
        postcheck_success, postcheck_reason = _postcheck_success(task_kind, artifact_files)

    # Issue #925 (P8) — R5 misroute fail-closed gate.
    # Compare the agent's CLASSIFIED kind against the EXPECTED kind (the raw meta
    # `category`/`task_kind`, NOT the inferred-merged `task_kind` variable above /
    # DR1-003). Only non-coding cases with a real expected kind are gated (coding
    # bypasses R5). A misroute — classified differs, or is missing/unknown —
    # forces `postcheck_success=False` (fail-closed; default-to-pass prohibited).
    classified_task_kind = _read_classified_task_kind(run_dir)
    expected_kind = (
        meta["task_kind"]
        if meta.get("_task_kind_source") in ("task_kind", "category")
        else None
    )
    task_kind_misroute = False
    if expected_kind is not None and expected_kind != "coding":
        if classified_task_kind is None:
            task_kind_misroute = True
            r5_reason = "missing_classification"
        elif classified_task_kind != expected_kind:
            task_kind_misroute = True
            r5_reason = "task_kind_misroute"
        if task_kind_misroute:
            postcheck_success = False
            postcheck_reason = r5_reason

    anvil_terminal_success = _anvil_terminal_success(meta["rc"])
    anvil_terminal_class = _anvil_terminal_class(meta["rc"])
    outcome_agreement = _outcome_agreement(meta["rc"], postcheck_success)
    failure_authority = _classify_failure_authority(
        meta_authority=meta["failure_authority"],
        eval_taxonomy=eval_log_taxonomy,
        rc=meta["rc"],
        postcheck_success=postcheck_success,
        postcheck_reason=postcheck_reason,
        failure_kind=failure_kind,
        anvil_score=anvil_score,
        artifact_files=artifact_files,
    )
    page_tsx_touched = "src/app/page.tsx" in files_modified
    tool_calls = session_metrics["tool_calls"]
    tool_call_total = sum(tool_calls.values())
    worker_lifecycle_available = _has_worker_lifecycle_projection(
        eval_objective_projection
    )
    worker_lifecycle_fields = {
        key: eval_objective_projection[key]
        for key in (
            *WORKER_LIFECYCLE_STRING_FIELDS,
            *WORKER_LIFECYCLE_BOOL_FIELDS,
            *WORKER_LIFECYCLE_INT_FIELDS,
        )
        if key in eval_objective_projection
    }
    if worker_lifecycle_available or task_kind_misroute:
        worker_lifecycle_fields["lifecycle_failure_stage"] = (
            _derive_lifecycle_failure_stage(
                task_kind_misroute=task_kind_misroute,
                projection=eval_objective_projection,
                generic_terminal_state=generic_terminal_state,
                postcheck_success=postcheck_success,
            )
        )

    # Issue #976 (parent #974, Issue B): structure the failure for replay
    # classification + transition metrics. Emitted for every run (success ->
    # failure_class "none") so report.py can use all analyzed runs as a rate
    # denominator.
    failure_observation = _build_failure_observation(
        generic_terminal_state=generic_terminal_state,
        legacy_terminal_state=legacy_terminal_state,
        postcheck_success=postcheck_success,
        artifact_files=artifact_files,
        write_target_counts=session_metrics["write_target_counts"],
        evidence_kind=evidence_kind,
        failure_kind=failure_kind,
        anvil_score=anvil_score,
        recovery_strategy_count=recovery_strategy_count,
        recovery_strategies=recovery_strategies,
        projection=eval_objective_projection,
        failure_inputs=failure_inputs,
    )

    out: dict[str, Any] = {
        "anvil_score": anvil_score,
        "anvil_terminal_class": anvil_terminal_class,
        "anvil_terminal_success": anvil_terminal_success,
        "artifact_file_count": len(artifact_files),
        "artifact_files": artifact_files,
        "case": meta["case"],
        "compact_events": session_metrics["compact_events"],
        # Issue #925: surface the agent's classified kind + R5 verdict. Emitted
        # conditionally (omit when absent / not-a-misroute) so pre-#925 records
        # and coding cases keep their existing output shape verbatim.
        **(
            {"classified_task_kind": classified_task_kind}
            if classified_task_kind is not None
            else {}
        ),
        **({"task_kind_misroute": True} if task_kind_misroute else {}),
        "deliverable_kind": deliverable_kind,
        "elapsed_s": meta["elapsed_s"],
        "evidence_kind": evidence_kind,
        "error_500_count": error_500_count,
        "evaluation_taxonomy": {
            "anvil_terminal_class": anvil_terminal_class,
            "failure_authority": failure_authority,
            "outcome_agreement": outcome_agreement,
            "pam_variant": pam_variant,
            "task_kind": task_kind,
        },
        "failure_authority": failure_authority,
        "failure_kind": failure_kind,
        "failure_observation": failure_observation,
        "files_modified": files_modified,
        "generic_terminal_state": generic_terminal_state,
        "iter_count": session_metrics["iter_count"],
        "keywords_version": KEYWORDS_VERSION,
        "legacy_terminal_state": legacy_terminal_state,
        "outcome_agreement": outcome_agreement,
        "page_tsx_has_game_keywords": page_tsx_has_keywords,
        "page_tsx_touched": page_tsx_touched,
        "pam_variant": pam_variant,
        "postcheck_reason": postcheck_reason,
        "postcheck_success": postcheck_success,
        "rc": meta["rc"],
        "recovery_job_kind": recovery_job_kind,
        "recovery_strategies": recovery_strategies,
        "recovery_strategy_count": recovery_strategy_count,
        "run_id": session_metrics["run_id"],
        "schema_version": SCHEMA_VERSION,
        "task_kind": task_kind,
        "token_completion": token_completion,
        "token_prompt": token_prompt,
        "tool_call_total": tool_call_total,
        "tool_calls": tool_calls,
        "we_total": session_metrics["we_total"],
        **worker_lifecycle_fields,
        "xml_parser_errors": None,
    }

    json.dump(out, sys.stdout, sort_keys=True, ensure_ascii=False)
    sys.stdout.write("\n")
    return 0


if __name__ == "__main__":
    # Respect umask; no sensitive files are written by this script.
    os.umask(0o077)
    sys.exit(main(sys.argv))
