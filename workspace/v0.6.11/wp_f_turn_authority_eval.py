#!/usr/bin/env python3
"""Small current-turn authority smoke matrix for WP-F.

Each case runs two Anvil oneshot turns against the same state dir and cwd.
Turn 1 intentionally leaves a different active_task in session memory; turn 2
then asks for a different task kind. The grader checks the turn-2 deliverable
only, so failures point at stale-context contamination or tool-call adherence.
"""

from __future__ import annotations

import argparse
import csv
import json
import os
import shutil
import subprocess
import textwrap
import time
from dataclasses import dataclass
from pathlib import Path


FIELDS = [
    "seq",
    "case_id",
    "first_kind",
    "second_kind",
    "pass",
    "high_quality",
    "verification_pass",
    "turn1_rc",
    "turn2_rc",
    "turn1_exit_reason",
    "turn2_exit_reason",
    "duration_sec",
    "changed_files",
    "notes",
]


@dataclass(frozen=True)
class SequenceCase:
    case_id: str
    first_kind: str
    second_kind: str
    first_prompt: str
    second_prompt: str
    setup: callable
    grade: callable


def write(path: Path, content: str) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(textwrap.dedent(content).lstrip(), encoding="utf-8")


def read(path: Path) -> str:
    return path.read_text(encoding="utf-8")


def snapshot(root: Path) -> dict[str, str]:
    out: dict[str, str] = {}
    for path in sorted(root.rglob("*")):
        if not path.is_file():
            continue
        rel = path.relative_to(root).as_posix()
        if rel.startswith((".anvil-state/", "target/", "node_modules/")):
            continue
        try:
            out[rel] = path.read_text(encoding="utf-8")
        except UnicodeDecodeError:
            out[rel] = "<binary>"
    return out


def changed(before: dict[str, str], after: dict[str, str]) -> list[str]:
    keys = set(before) | set(after)
    return sorted(k for k in keys if before.get(k) != after.get(k))


def bool_s(value: bool) -> str:
    return "true" if value else "false"


def setup_orders(root: Path) -> None:
    write(root / "input" / "orders.csv", "id,total\n1,10\n2,25\n")


def setup_empty(_: Path) -> None:
    return None


def grade_data_after_coding(root: Path) -> tuple[bool, bool, bool, str]:
    path = root / "output" / "summary.json"
    if not path.exists():
        return False, False, False, "missing output/summary.json"
    try:
        data = json.loads(read(path))
    except json.JSONDecodeError as err:
        return False, False, False, f"invalid json: {err}"
    passed = data.get("count") == 2 and data.get("total") == 35
    return passed, passed, passed, "" if passed else f"unexpected json: {data!r}"


def grade_docs_after_coding(root: Path) -> tuple[bool, bool, bool, str]:
    path = root / "docs" / "runbook.md"
    if not path.exists():
        return False, False, False, "missing docs/runbook.md"
    text = read(path).lower()
    required = ["overview", "run", "troubleshooting"]
    missing = [item for item in required if item not in text]
    passed = not missing
    return passed, passed, passed, "" if passed else "missing headings: " + ",".join(missing)


def grade_coding_after_docs(root: Path) -> tuple[bool, bool, bool, str]:
    source = root / "src" / "slugify.py"
    test = root / "tests" / "test_slugify.py"
    if not source.exists() or not test.exists():
        return False, False, False, "missing slugify source or test"
    cp = subprocess.run(
        ["python3", "-m", "pytest", "tests/test_slugify.py"],
        cwd=root,
        text=True,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        timeout=60,
        check=False,
    )
    passed = cp.returncode == 0
    return passed, passed, passed, "" if passed else (cp.stdout + cp.stderr)[-500:]


def grade_tdd_after_data(root: Path) -> tuple[bool, bool, bool, str]:
    source = root / "src" / "stats.py"
    test = root / "tests" / "test_stats.py"
    if not source.exists() or not test.exists():
        return False, False, False, "missing stats source or test"
    cp = subprocess.run(
        ["python3", "-m", "pytest", "tests/test_stats.py"],
        cwd=root,
        text=True,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        timeout=60,
        check=False,
    )
    passed = cp.returncode == 0
    return passed, passed, passed, "" if passed else (cp.stdout + cp.stderr)[-500:]


CASES = [
    SequenceCase(
        case_id="coding_to_data",
        first_kind="coding",
        second_kind="data",
        first_prompt=(
            "Create package.json and src/add.js exporting an add(a, b) function. "
            "Keep it minimal."
        ),
        second_prompt=(
            "Read input/orders.csv and create output/summary.json with exactly "
            "the numeric fields count and total."
        ),
        setup=setup_orders,
        grade=grade_data_after_coding,
    ),
    SequenceCase(
        case_id="coding_to_docs",
        first_kind="coding",
        second_kind="docs",
        first_prompt=(
            "Create a minimal Python module src/math_utils.py with a double(x) "
            "function and one pytest test."
        ),
        second_prompt=(
            "Create docs/runbook.md for this workspace with sections Overview, "
            "Run, and Troubleshooting. Do not add new source code."
        ),
        setup=setup_empty,
        grade=grade_docs_after_coding,
    ),
    SequenceCase(
        case_id="docs_to_coding",
        first_kind="docs",
        second_kind="coding",
        first_prompt="Create docs/notes.md with a short onboarding checklist.",
        second_prompt=(
            "Create src/slugify.py with slugify(text) and tests/test_slugify.py. "
            "The test should prove 'Hello, Local LLM!' becomes 'hello-local-llm'."
        ),
        setup=setup_empty,
        grade=grade_coding_after_docs,
    ),
    SequenceCase(
        case_id="data_to_tdd",
        first_kind="data",
        second_kind="coding_tdd",
        first_prompt=(
            "Create input/items.csv with two rows and output/items.json as a "
            "JSON array derived from it."
        ),
        second_prompt=(
            "Using TDD, create src/stats.py with mean(values) and "
            "tests/test_stats.py covering normal values and an empty list error."
        ),
        setup=setup_empty,
        grade=grade_tdd_after_data,
    ),
]


def load_llm_events(state_dir: Path) -> list[dict]:
    events: list[dict] = []
    for path in sorted(state_dir.glob("sessions/*/logs/llm-io.jsonl")):
        with path.open(encoding="utf-8") as fh:
            for line in fh:
                try:
                    event = json.loads(line)
                except json.JSONDecodeError:
                    continue
                if isinstance(event, dict):
                    events.append(event)
    return events


def extract_exit_reason(state_dir: Path, output: str) -> str:
    for event in reversed(load_llm_events(state_dir)):
        payload = event.get("payload")
        if not isinstance(payload, dict):
            continue
        if event.get("event") == "agent.milestone.turn_completed":
            reason = payload.get("exit_reason")
            if isinstance(reason, str):
                return reason
        reason = payload.get("outcome")
        if isinstance(reason, str) and ("repair" in reason or "missing" in reason or reason == "done"):
            return reason
    lower = output.lower()
    if "repair_exhausted" in lower:
        return "repair_exhausted"
    if "max iterations" in lower or "max_iterations" in lower:
        return "max_iterations"
    if "done" in lower:
        return "done"
    return ""


def run_turn(
    *,
    anvil_bin: str,
    model: str,
    sidecar_model: str,
    state_dir: Path,
    workdir: Path,
    prompt: str,
    fresh: bool,
    max_iterations: int,
    chat_timeout_secs: int,
    timeout_secs: int,
) -> subprocess.CompletedProcess[str]:
    cmd = [
        anvil_bin,
        "-m",
        model,
        "--sidecar-model",
        sidecar_model,
        "--chat-timeout-secs",
        str(chat_timeout_secs),
        "--max-iterations",
        str(max_iterations),
        "--oneshot",
        "-y",
        "--no-footer",
        "--state-dir",
        str(state_dir),
        "--cwd",
        str(workdir),
        "--prompt",
        prompt,
    ]
    if fresh:
        cmd.insert(cmd.index("--state-dir"), "--fresh-session")
    env = os.environ.copy()
    env.update(
        {
            "ANVIL_PHOTON_ENABLED": "false",
            "ANVIL_PHOTON_SHADOW_MODE": "true",
            "ANVIL_PHOTON_CANARY": "0",
        }
    )
    started = time.monotonic()
    try:
        cp = subprocess.run(
            cmd,
            cwd=workdir,
            env=env,
            text=True,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            timeout=timeout_secs,
            check=False,
        )
    except subprocess.TimeoutExpired as exc:
        stdout = exc.stdout if isinstance(exc.stdout, str) else (exc.stdout or b"").decode("utf-8", "replace")
        stderr = exc.stderr if isinstance(exc.stderr, str) else (exc.stderr or b"").decode("utf-8", "replace")
        return subprocess.CompletedProcess(cmd, 124, stdout, stderr + f"\nTIMEOUT after {timeout_secs}s\n")
    cp.duration = time.monotonic() - started  # type: ignore[attr-defined]
    return cp


def run_case(
    *,
    seq: int,
    case: SequenceCase,
    run_dir: Path,
    anvil_bin: str,
    model: str,
    sidecar_model: str,
    max_iterations: int,
    chat_timeout_secs: int,
    timeout_secs: int,
) -> dict[str, str]:
    workdir = run_dir / "workdirs" / f"{seq:03d}-{case.case_id}"
    state_dir = run_dir / "state" / f"{seq:03d}-{case.case_id}"
    raw_dir = run_dir / "raw" / f"{seq:03d}-{case.case_id}"
    shutil.rmtree(workdir, ignore_errors=True)
    shutil.rmtree(state_dir, ignore_errors=True)
    workdir.mkdir(parents=True)
    state_dir.mkdir(parents=True)
    raw_dir.mkdir(parents=True)
    case.setup(workdir)

    cp1 = run_turn(
        anvil_bin=anvil_bin,
        model=model,
        sidecar_model=sidecar_model,
        state_dir=state_dir,
        workdir=workdir,
        prompt=case.first_prompt,
        fresh=True,
        max_iterations=max_iterations,
        chat_timeout_secs=chat_timeout_secs,
        timeout_secs=timeout_secs,
    )
    before_turn2 = snapshot(workdir)
    cp2 = run_turn(
        anvil_bin=anvil_bin,
        model=model,
        sidecar_model=sidecar_model,
        state_dir=state_dir,
        workdir=workdir,
        prompt=case.second_prompt,
        fresh=False,
        max_iterations=max_iterations,
        chat_timeout_secs=chat_timeout_secs,
        timeout_secs=timeout_secs,
    )
    write(raw_dir / "turn1.stdout.log", cp1.stdout)
    write(raw_dir / "turn1.stderr.log", cp1.stderr)
    write(raw_dir / "turn2.stdout.log", cp2.stdout)
    write(raw_dir / "turn2.stderr.log", cp2.stderr)
    write(raw_dir / "prompts.json", json.dumps({"turn1": case.first_prompt, "turn2": case.second_prompt}, indent=2) + "\n")

    passed, high_quality, verification_pass, notes = case.grade(workdir)
    if cp1.returncode != 0 or cp2.returncode != 0:
        high_quality = False
        notes = (notes + "; " if notes else "") + f"rc turn1={cp1.returncode} turn2={cp2.returncode}"
    changed_files = changed(before_turn2, snapshot(workdir))
    return {
        "seq": str(seq),
        "case_id": case.case_id,
        "first_kind": case.first_kind,
        "second_kind": case.second_kind,
        "pass": bool_s(passed),
        "high_quality": bool_s(high_quality),
        "verification_pass": bool_s(verification_pass),
        "turn1_rc": str(cp1.returncode),
        "turn2_rc": str(cp2.returncode),
        "turn1_exit_reason": extract_exit_reason(state_dir, cp1.stdout + "\n" + cp1.stderr),
        "turn2_exit_reason": extract_exit_reason(state_dir, cp2.stdout + "\n" + cp2.stderr),
        "duration_sec": f"{getattr(cp1, 'duration', 0.0) + getattr(cp2, 'duration', 0.0):.1f}",
        "changed_files": ",".join(changed_files),
        "notes": notes,
    }


def write_summary(run_dir: Path, rows: list[dict[str, str]]) -> None:
    total = len(rows)
    passed = sum(1 for row in rows if row["pass"] == "true")
    high_quality = sum(1 for row in rows if row["high_quality"] == "true")
    verification = sum(1 for row in rows if row["verification_pass"] == "true")
    summary = {
        "total": total,
        "pass": passed,
        "high_quality": high_quality,
        "verification_pass": verification,
        "rows": rows,
    }
    write(run_dir / "summary.json", json.dumps(summary, indent=2, sort_keys=True) + "\n")
    lines = [
        "# WP-F Current-Turn Authority Summary",
        "",
        f"- total: {total}",
        f"- pass: {passed}/{total}",
        f"- high_quality: {high_quality}/{total}",
        f"- verification_pass: {verification}/{total}",
        "",
        "| case | turn kinds | pass | high_quality | notes |",
        "| --- | --- | ---: | ---: | --- |",
    ]
    for row in rows:
        lines.append(
            f"| {row['case_id']} | {row['first_kind']} -> {row['second_kind']} | "
            f"{row['pass']} | {row['high_quality']} | {row['notes']} |"
        )
    write(run_dir / "summary.md", "\n".join(lines) + "\n")


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--run-id", required=True)
    parser.add_argument("--out-root", default="workspace/v0.6.11/eval-runs")
    parser.add_argument("--anvil-bin", default="target/debug/anvil")
    parser.add_argument("--model", default="qwen3.6:27b-coding-nvfp4")
    parser.add_argument("--sidecar-model", default="qwen3-coder:30b")
    parser.add_argument("--max-iterations", type=int, default=16)
    parser.add_argument("--chat-timeout-secs", type=int, default=180)
    parser.add_argument("--timeout-secs", type=int, default=360)
    return parser.parse_args()


def main() -> int:
    args = parse_args()
    repo = Path(__file__).resolve().parents[2]
    run_dir = repo / args.out_root / args.run_id
    if run_dir.exists():
        raise SystemExit(f"run dir already exists: {run_dir}")
    run_dir.mkdir(parents=True)
    rows: list[dict[str, str]] = []
    for seq, case in enumerate(CASES, start=1):
        print(f"wp-f {seq}/{len(CASES)} {case.case_id}", flush=True)
        rows.append(
            run_case(
                seq=seq,
                case=case,
                run_dir=run_dir,
                anvil_bin=str((repo / args.anvil_bin).resolve()),
                model=args.model,
                sidecar_model=args.sidecar_model,
                max_iterations=args.max_iterations,
                chat_timeout_secs=args.chat_timeout_secs,
                timeout_secs=args.timeout_secs,
            )
        )
    with (run_dir / "results.csv").open("w", encoding="utf-8", newline="") as fh:
        writer = csv.DictWriter(fh, fieldnames=FIELDS)
        writer.writeheader()
        writer.writerows(rows)
    write_summary(run_dir, rows)
    print((run_dir / "summary.md").read_text(encoding="utf-8"), flush=True)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
