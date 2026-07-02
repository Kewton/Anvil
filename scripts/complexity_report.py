#!/usr/bin/env python3
"""Report approximate Rust function complexity.

This is a lightweight trend tool, not a compiler-grade Rust parser. It is
intended to make large control-flow hotspots visible during refactoring.

Default scope is `src/**/*.rs`.
"""

from __future__ import annotations

import argparse
import json
import os
import re
import stat
import sys
from dataclasses import asdict, dataclass
from pathlib import Path
from typing import Iterable


FN_RE = re.compile(
    r"\b(?:pub(?:\([^)]*\))?\s+)?(?:async\s+)?(?:unsafe\s+)?"
    r"(?:extern\s+\"[^\"]+\"\s+)?fn\s+([A-Za-z_][A-Za-z0-9_]*)\b"
)

MAX_FILE_BYTES = 5 * 1024 * 1024


@dataclass
class FunctionMetric:
    path: str
    name: str
    line: int
    loc: int
    rough_cc: int


@dataclass
class FileMetric:
    path: str
    functions: int
    avg_rough_cc: float
    max_rough_cc: int
    rough_cc_ge_threshold: int
    rough_cc_ge_high_threshold: int
    function_loc: int


@dataclass
class MaskState:
    in_block_comment: bool = False
    raw_string_hashes: str | None = None
    in_string: bool = False
    escaped: bool = False


def _is_regular_file(path: Path) -> bool:
    try:
        st = path.lstat()
    except OSError:
        return False
    return stat.S_ISREG(st.st_mode) and not stat.S_ISLNK(st.st_mode)


def _safe_rs_file(path: Path, root: Path) -> Path | None:
    try:
        resolved = path.resolve(strict=True)
        root_resolved = root.resolve(strict=True)
    except OSError:
        return None
    if not resolved.is_relative_to(root_resolved):
        return None
    if not _is_regular_file(resolved):
        return None
    if resolved.suffix != ".rs":
        return None
    try:
        if resolved.stat().st_size > MAX_FILE_BYTES:
            return None
    except OSError:
        return None
    return resolved


def _iter_rs_files(root: Path, inputs: list[str]) -> list[Path]:
    paths: list[Path] = []
    candidates: Iterable[Path]
    if inputs:
        expanded: list[Path] = []
        for item in inputs:
            candidate = Path(item)
            if not candidate.is_absolute():
                candidate = root / candidate
            if candidate.is_dir():
                expanded.extend(candidate.rglob("*.rs"))
            else:
                expanded.append(candidate)
        candidates = expanded
    else:
        candidates = (root / "src").rglob("*.rs")

    seen: set[Path] = set()
    for candidate in candidates:
        safe = _safe_rs_file(candidate, root)
        if safe is None or safe in seen:
            continue
        seen.add(safe)
        paths.append(safe)
    return sorted(paths)


def _raw_string_start(line: str, index: int) -> tuple[int, str] | None:
    match = re.match(r'(?:b)?r(#+)?"', line[index:])
    if not match:
        return None
    hashes = match.group(1) or ""
    return len(match.group(0)), hashes


def _mask_comments_and_literals(line: str, state: MaskState) -> tuple[str, MaskState]:
    """Replace comments and string/char literals with spaces.

    This intentionally handles common Rust syntax only. The goal is stable
    trend reporting rather than exact parsing.
    """
    out: list[str] = []
    i = 0
    while i < len(line):
        ch = line[i]
        nxt = line[i + 1] if i + 1 < len(line) else ""

        if state.raw_string_hashes is not None:
            closing = '"' + state.raw_string_hashes
            end = line.find(closing, i)
            if end == -1:
                out.extend(" " * (len(line) - i))
                break
            out.extend(" " * (end + len(closing) - i))
            i = end + len(closing)
            state.raw_string_hashes = None
            continue

        if state.in_block_comment:
            if ch == "*" and nxt == "/":
                out.extend("  ")
                i += 2
                state.in_block_comment = False
            else:
                out.append(" ")
                i += 1
            continue

        if state.in_string:
            out.append(" ")
            if not state.escaped and ch == '"':
                state.in_string = False
            state.escaped = (not state.escaped and ch == "\\")
            if ch != "\\":
                state.escaped = False
            i += 1
            continue

        if ch == "/" and nxt == "/":
            out.extend(" " * (len(line) - i))
            break
        if ch == "/" and nxt == "*":
            out.extend("  ")
            i += 2
            state.in_block_comment = True
            continue
        raw_start = _raw_string_start(line, i)
        if raw_start is not None:
            start_len, hashes = raw_start
            out.extend(" " * start_len)
            i += start_len
            state.raw_string_hashes = hashes
            continue
        if ch == '"':
            out.append(" ")
            state.in_string = True
            state.escaped = False
            i += 1
            continue
        if ch == "'":
            char_match = re.match(r"'(?:\\.|[^\\'])'", line[i:])
            if char_match:
                out.extend(" " * len(char_match.group(0)))
                i += len(char_match.group(0))
                continue
            out.append(ch)
            i += 1
            continue

        out.append(ch)
        i += 1

    return "".join(out), state


def _rough_complexity_delta(masked_line: str) -> int:
    score = 0
    for keyword in ("if", "match", "for", "while", "loop"):
        score += len(re.findall(rf"\b{keyword}\b", masked_line))
    score += masked_line.count("&&")
    score += masked_line.count("||")
    score += masked_line.count("=>")
    score += masked_line.count("?")
    return score


def analyze_file(path: Path, root: Path) -> list[FunctionMetric]:
    rel = path.relative_to(root).as_posix()
    try:
        text = path.read_text(encoding="utf-8")
    except UnicodeDecodeError:
        text = path.read_text(encoding="utf-8", errors="replace")

    functions: list[FunctionMetric] = []
    mask_state = MaskState()
    active: dict[str, int | str] | None = None
    pending: dict[str, int | str] | None = None
    brace_depth = 0

    for line_no, raw_line in enumerate(text.splitlines(), start=1):
        masked, mask_state = _mask_comments_and_literals(raw_line, mask_state)

        if active is None and pending is None:
            match = FN_RE.search(masked)
            if match:
                pending = {"name": match.group(1), "line": line_no, "score": 1}

        if pending is not None and active is None:
            pending["score"] = int(pending["score"]) + _rough_complexity_delta(masked)
            open_count = masked.count("{")
            close_count = masked.count("}")
            if open_count:
                active = pending
                pending = None
                brace_depth = open_count - close_count
                if brace_depth <= 0:
                    start_line = int(active["line"])
                    functions.append(
                        FunctionMetric(
                            path=rel,
                            name=str(active["name"]),
                            line=start_line,
                            loc=line_no - start_line + 1,
                            rough_cc=int(active["score"]),
                        )
                    )
                    active = None
                    brace_depth = 0
            continue

        if active is not None:
            active["score"] = int(active["score"]) + _rough_complexity_delta(masked)
            brace_depth += masked.count("{")
            brace_depth -= masked.count("}")
            if brace_depth <= 0:
                start_line = int(active["line"])
                functions.append(
                    FunctionMetric(
                        path=rel,
                        name=str(active["name"]),
                        line=start_line,
                        loc=line_no - start_line + 1,
                        rough_cc=int(active["score"]),
                    )
                )
                active = None
                brace_depth = 0

    return functions


def file_metrics(
    functions: list[FunctionMetric], threshold: int, high_threshold: int
) -> list[FileMetric]:
    grouped: dict[str, list[FunctionMetric]] = {}
    for function in functions:
        grouped.setdefault(function.path, []).append(function)

    metrics: list[FileMetric] = []
    for path, items in sorted(grouped.items()):
        total = sum(item.rough_cc for item in items)
        metrics.append(
            FileMetric(
                path=path,
                functions=len(items),
                avg_rough_cc=round(total / len(items), 2) if items else 0.0,
                max_rough_cc=max((item.rough_cc for item in items), default=0),
                rough_cc_ge_threshold=sum(
                    1 for item in items if item.rough_cc >= threshold
                ),
                rough_cc_ge_high_threshold=sum(
                    1 for item in items if item.rough_cc >= high_threshold
                ),
                function_loc=sum(item.loc for item in items),
            )
        )
    return metrics


def _print_text(
    files: list[FileMetric], functions: list[FunctionMetric], top: int
) -> None:
    print("File metrics:")
    print("functions avg_cc max_cc cc>=threshold cc>=high fn_loc path")
    for item in sorted(files, key=lambda x: (-x.max_rough_cc, x.path)):
        print(
            f"{item.functions:9d} {item.avg_rough_cc:6.2f} {item.max_rough_cc:6d} "
            f"{item.rough_cc_ge_threshold:13d} "
            f"{item.rough_cc_ge_high_threshold:8d} {item.function_loc:6d} "
            f"{item.path}"
        )

    print()
    print(f"Top {top} functions:")
    print("rough_cc loc line path::function")
    for item in sorted(functions, key=lambda x: (-x.rough_cc, -x.loc, x.path, x.line))[
        :top
    ]:
        print(
            f"{item.rough_cc:8d} {item.loc:3d} {item.line:5d} "
            f"{item.path}::{item.name}"
        )


def main(argv: list[str]) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("paths", nargs="*", help="Rust files or directories")
    parser.add_argument("--root", default=".", help="repository root")
    parser.add_argument("--json", action="store_true", help="emit JSON")
    parser.add_argument("--top", type=int, default=20, help="number of functions to show")
    parser.add_argument("--threshold", type=int, default=15)
    parser.add_argument("--high-threshold", type=int, default=50)
    args = parser.parse_args(argv)

    root = Path(args.root)
    try:
        root = root.resolve(strict=True)
    except OSError as exc:
        print(f"error: invalid root: {exc}", file=sys.stderr)
        return 2
    if not root.is_dir():
        print("error: root is not a directory", file=sys.stderr)
        return 2

    files = _iter_rs_files(root, args.paths)
    functions: list[FunctionMetric] = []
    for path in files:
        functions.extend(analyze_file(path, root))

    file_items = file_metrics(functions, args.threshold, args.high_threshold)
    payload = {
        "schema_version": 1,
        "root": os.fspath(root),
        "threshold": args.threshold,
        "high_threshold": args.high_threshold,
        "files_analyzed": len(files),
        "functions_analyzed": len(functions),
        "file_metrics": [asdict(item) for item in file_items],
        "top_functions": [
            asdict(item)
            for item in sorted(
                functions, key=lambda x: (-x.rough_cc, -x.loc, x.path, x.line)
            )[: args.top]
        ],
    }

    if args.json:
        print(json.dumps(payload, ensure_ascii=False, indent=2, sort_keys=True))
    else:
        _print_text(file_items, functions, args.top)
    return 0


if __name__ == "__main__":
    raise SystemExit(main(sys.argv[1:]))
