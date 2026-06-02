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
KNOWN_TASK_KINDS = {"coding", "docs", "data", "research", "ops"}

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


def _read_meta(run_dir: Path) -> dict[str, Any]:
    """Return selected meta.json fields, using None/defaults on failure."""
    fallback = {
        "case": "default",
        "elapsed_s": None,
        "pam_variant": "default",
        "rc": None,
        "task_kind": DEFAULT_TASK_KIND,
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

    task_kind = _safe_meta_string(data, "task_kind")
    if task_kind is None:
        task_kind = _safe_meta_string(data, "category")
    if task_kind is None or task_kind not in KNOWN_TASK_KINDS:
        task_kind = DEFAULT_TASK_KIND

    return {
        "case": _safe_meta_string(data, "case") or "default",
        "elapsed_s": elapsed_s,
        "pam_variant": _safe_meta_string(data, "pam_variant") or "default",
        "rc": rc,
        "task_kind": task_kind,
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
    token_prompt, token_completion = _read_token_usage(run_dir)

    files_modified = session_metrics["files_modified"]
    artifact_files = _artifact_candidates(files_modified)
    postcheck_success, postcheck_reason = _postcheck_success(
        meta["task_kind"], artifact_files
    )
    page_tsx_touched = "src/app/page.tsx" in files_modified
    tool_calls = session_metrics["tool_calls"]
    tool_call_total = sum(tool_calls.values())

    out: dict[str, Any] = {
        "anvil_score": anvil_score,
        "artifact_file_count": len(artifact_files),
        "artifact_files": artifact_files,
        "case": meta["case"],
        "compact_events": session_metrics["compact_events"],
        "elapsed_s": meta["elapsed_s"],
        "error_500_count": error_500_count,
        "failure_kind": failure_kind,
        "files_modified": files_modified,
        "iter_count": session_metrics["iter_count"],
        "keywords_version": KEYWORDS_VERSION,
        "page_tsx_has_game_keywords": page_tsx_has_keywords,
        "page_tsx_touched": page_tsx_touched,
        "pam_variant": meta["pam_variant"],
        "postcheck_reason": postcheck_reason,
        "postcheck_success": postcheck_success,
        "rc": meta["rc"],
        "run_id": session_metrics["run_id"],
        "schema_version": SCHEMA_VERSION,
        "task_kind": meta["task_kind"],
        "token_completion": token_completion,
        "token_prompt": token_prompt,
        "tool_call_total": tool_call_total,
        "tool_calls": tool_calls,
        "we_total": session_metrics["we_total"],
        "xml_parser_errors": None,
    }

    json.dump(out, sys.stdout, sort_keys=True, ensure_ascii=False)
    sys.stdout.write("\n")
    return 0


if __name__ == "__main__":
    # Respect umask; no sensitive files are written by this script.
    os.umask(0o077)
    sys.exit(main(sys.argv))
