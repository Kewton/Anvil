#!/usr/bin/env python3
"""Run WP10/WP11 local-LLM evaluation matrices.

The script is intentionally small and self-contained for the v0.6.11
architecture validation work. It creates deterministic workspaces, invokes the
local Anvil binary, grades observable artifacts with local commands, and writes
CSV/Markdown summaries. Raw run directories are kept under workspace/v0.6.11.
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
from typing import Callable


RESULT_FIELDS = [
    "suite",
    "run_id",
    "seq",
    "variant",
    "case_id",
    "task_kind",
    "pass",
    "high_quality",
    "verification_pass",
    "anvil_rc",
    "exit_reason",
    "duration_sec",
    "changed_files",
    "unexpected_changes",
    "false_done",
    "false_missing",
    "repair_exhausted",
    "max_iterations",
    "pam_availability",
    "pam_injected_count",
    "pam_unused_reason",
    "pam_failure_phase",
    "shadow_terminal_class",
    "shadow_terminal_conflict",
    "shadow_missing_evidence",
    "shadow_failed_evidence",
    "notes",
]


@dataclass(frozen=True)
class Case:
    case_id: str
    task_kind: str
    prompt: str
    setup: Callable[[Path], None]
    grade: Callable[[Path], tuple[bool, bool, bool, str]]
    allowed_prefixes: tuple[str, ...]


def write(path: Path, content: str) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(textwrap.dedent(content).lstrip(), encoding="utf-8")


def read(path: Path) -> str:
    return path.read_text(encoding="utf-8")


def run_cmd(cmd: list[str], cwd: Path, timeout: int = 60) -> subprocess.CompletedProcess[str]:
    return subprocess.run(
        cmd,
        cwd=cwd,
        text=True,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        timeout=timeout,
        check=False,
    )


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


def allowed_changes(paths: list[str], prefixes: tuple[str, ...]) -> list[str]:
    return [p for p in paths if not any(p == prefix or p.startswith(prefix) for prefix in prefixes)]


def bool_s(value: bool) -> str:
    return "true" if value else "false"


def load_llm_events(state_dir: Path) -> list[dict]:
    events: list[dict] = []
    for path in sorted(state_dir.glob("sessions/*/logs/llm-io.jsonl")):
        with path.open(encoding="utf-8") as fh:
            for line in fh:
                line = line.strip()
                if not line:
                    continue
                try:
                    value = json.loads(line)
                except json.JSONDecodeError:
                    continue
                if isinstance(value, dict):
                    events.append(value)
    return events


def extract_exit_reason(state_dir: Path, output: str) -> str:
    events = load_llm_events(state_dir)
    for event in reversed(events):
        payload = event.get("payload")
        if not isinstance(payload, dict):
            continue
        if event.get("event") == "agent.milestone.turn_completed":
            reason = payload.get("exit_reason")
            if isinstance(reason, str):
                return reason
        reason = payload.get("outcome")
        if isinstance(reason, str) and (
            "repair" in reason or "missing" in reason or reason in {"done", "max_iterations"}
        ):
            return reason
    lower = output.lower()
    if "repair_exhausted" in lower:
        return "repair_exhausted"
    if "safe_stop_verifier_missing" in lower:
        return "safe_stop_verifier_missing"
    if "max iterations" in lower or "max_iterations" in lower:
        return "max_iterations"
    if "done" in lower:
        return "done"
    return ""


def setup_empty(_: Path) -> None:
    return None


def setup_data_orders(root: Path) -> None:
    write(
        root / "input" / "orders.csv",
        """
        id,total
        1,10
        2,25
        """,
    )


def setup_data_profile(root: Path) -> None:
    write(
        root / "input" / "profile.json",
        """
        {"id": 7, "total": 42, "name": "local"}
        """,
    )


def setup_ops_health(root: Path) -> None:
    script = root / "scripts" / "health.sh"
    write(
        script,
        """
        #!/usr/bin/env bash
        echo "status=ok"
        echo "checks=3"
        exit 0
        """,
    )
    script.chmod(0o755)


def setup_feature_discount(root: Path) -> None:
    write(
        root / "discounts.py",
        """
        def final_price(price, percent):
            return price
        """,
    )
    write(
        root / "tests" / "test_discounts.py",
        """
        from discounts import final_price


        def test_zero_discount_keeps_price():
            assert final_price(10, 0) == 10
        """,
    )


def grade_docs(root: Path) -> tuple[bool, bool, bool, str]:
    path = root / "runbooks" / "local-agent-triage.md"
    if not path.exists():
        return False, False, False, "runbook missing"
    text = read(path).lower()
    ok = all(word in text for word in ["symptom", "logs", "recovery", "escalation"])
    return ok, ok, True, "" if ok else "missing required runbook sections"


def grade_data_csv(root: Path) -> tuple[bool, bool, bool, str]:
    path = root / "output" / "order-summary.csv"
    if not path.exists():
        return False, False, False, "output/order-summary.csv missing"
    lines = [line.strip() for line in read(path).splitlines() if line.strip()]
    ok = lines == ["id,total", "1,10", "2,25"]
    return ok, ok, True, "" if ok else f"unexpected CSV: {lines!r}"


def grade_data_json(root: Path) -> tuple[bool, bool, bool, str]:
    path = root / "output" / "profile-summary.json"
    if not path.exists():
        return False, False, False, "output/profile-summary.json missing"
    try:
        value = json.loads(read(path))
    except json.JSONDecodeError as exc:
        return False, False, False, f"invalid JSON: {exc}"
    ok = value == {"id": 7, "total": 42}
    return ok, ok, True, "" if ok else f"unexpected JSON: {value!r}"


def grade_research(root: Path) -> tuple[bool, bool, bool, str]:
    path = root / "research" / "cache-strategy-brief.md"
    if not path.exists():
        return False, False, False, "research brief missing"
    text = read(path).lower()
    ok = all(word in text for word in ["in-memory", "sqlite", "file", "recommend", "assumption"])
    return ok, ok, True, "" if ok else "missing required research content"


def grade_ops(root: Path) -> tuple[bool, bool, bool, str]:
    path = root / "reports" / "health-check.md"
    if not path.exists():
        return False, False, False, "health report missing"
    text = read(path).lower()
    ok = "status=ok" in text and ("exit code" in text or "exit=0" in text or "exit status 0" in text)
    return ok, ok, True, "" if ok else "report does not capture command output and exit code"


def grade_feature_discount(root: Path) -> tuple[bool, bool, bool, str]:
    if not (root / "discounts.py").exists():
        return False, False, False, "discounts.py missing"
    if not (root / "tests" / "test_discounts.py").exists():
        return False, False, False, "tests/test_discounts.py missing"
    pytest_run = run_cmd(["python3", "-m", "pytest", "-q", "tests/test_discounts.py"], root, timeout=120)
    if pytest_run.returncode != 0:
        return False, False, False, f"pytest failed: {(pytest_run.stdout + pytest_run.stderr)[-300:]}"
    check = run_cmd(
        [
            "python3",
            "-c",
            (
                "from discounts import final_price; "
                "assert final_price(100, 15) == 85.0; "
                "assert final_price(19.99, 10) == 17.99; "
                "assert final_price(10, 0) == 10"
            ),
        ],
        root,
        timeout=60,
    )
    ok = check.returncode == 0
    return ok, ok, ok, "" if ok else f"direct behavior check failed: {(check.stdout + check.stderr)[-300:]}"


def grade_python_sales(root: Path) -> tuple[bool, bool, bool, str]:
    if not (root / "main.py").exists():
        return False, False, False, "main.py missing"
    sample = root / "sample.csv"
    write(
        sample,
        """
        salesperson,region,amount
        Alice,West,10
        Bob,East,7
        Alice,East,5
        """,
    )
    cp = run_cmd(["python3", "main.py", "sample.csv"], root, timeout=20)
    functional = False
    note = ""
    if cp.returncode == 0:
        try:
            functional = json.loads(cp.stdout) == {"Alice": 15, "Bob": 7}
        except json.JSONDecodeError:
            note = f"stdout not JSON: {cp.stdout!r}"
    else:
        note = f"main.py failed: {cp.stderr[-300:]}"
    test_path = root / "tests" / "test_main.py"
    verification = False
    if test_path.exists():
        pytest_cp = run_cmd(["python3", "-m", "pytest", "-q", "tests/test_main.py"], root, timeout=60)
        verification = pytest_cp.returncode == 0
        if not verification and not note:
            note = f"pytest failed: {(pytest_cp.stdout + pytest_cp.stderr)[-300:]}"
    return functional, functional and verification, verification, note


def grade_python_markdown(root: Path) -> tuple[bool, bool, bool, str]:
    script = root / "markdown_lint.py"
    if not script.exists():
        return False, False, False, "markdown_lint.py missing"
    good = root / "good.md"
    bad = root / "bad.md"
    write(good, "# Title\n\n## Section\nContent\n")
    write(bad, "# Title  \n\n### Jump\n")
    good_cp = run_cmd(["python3", "markdown_lint.py", "good.md"], root, timeout=20)
    bad_cp = run_cmd(["python3", "markdown_lint.py", "bad.md"], root, timeout=20)
    functional = good_cp.returncode == 0 and bad_cp.returncode != 0
    tests = root / "tests" / "test_markdown_lint.py"
    verification = False
    if tests.exists():
        pytest_cp = run_cmd(["python3", "-m", "pytest", "-q", "tests/test_markdown_lint.py"], root, timeout=60)
        verification = pytest_cp.returncode == 0
    return functional, functional and verification, verification, "" if functional else "markdown lint behavior failed"


def grade_toml(root: Path) -> tuple[bool, bool, bool, str]:
    script = root / "merge_toml.py"
    if not script.exists():
        return False, False, False, "merge_toml.py missing"
    write(root / "a.toml", 'title = "A"\n[owner]\nname = "Alice"\n')
    write(root / "b.toml", 'title = "B"\nactive = true\n[owner]\nname = "Bob"\n')
    cp = run_cmd(["python3", "merge_toml.py", "a.toml", "b.toml"], root, timeout=30)
    text = cp.stdout.lower()
    functional = cp.returncode == 0 and 'title = "b"' in text and "active = true" in text and 'name = "bob"' in text
    tests = root / "tests" / "test_merge_toml.py"
    verification = False
    if tests.exists():
        pytest_cp = run_cmd(["python3", "-m", "pytest", "-q", "tests/test_merge_toml.py"], root, timeout=60)
        verification = pytest_cp.returncode == 0
    return functional, functional and verification, verification, "" if functional else f"TOML merge failed: {(cp.stdout + cp.stderr)[-300:]}"


def grade_cargo(root: Path) -> tuple[bool, bool, bool, str]:
    if not (root / "Cargo.toml").exists():
        return False, False, False, "Cargo.toml missing"
    cp = run_cmd(["cargo", "test"], root, timeout=120)
    ok = cp.returncode == 0
    return ok, ok, ok, "" if ok else f"cargo test failed: {(cp.stdout + cp.stderr)[-300:]}"


def grade_node_test(root: Path, test_path: str) -> tuple[bool, bool, bool, str]:
    if not (root / test_path).exists():
        return False, False, False, f"{test_path} missing"
    if (root / "package.json").exists():
        cp = run_cmd(["npm", "test"], root, timeout=120)
        if cp.returncode == 0:
            return True, True, True, ""
    cp = run_cmd(["node", "--test", test_path], root, timeout=120)
    ok = cp.returncode == 0
    return ok, ok, ok, "" if ok else f"node test failed: {(cp.stdout + cp.stderr)[-300:]}"


def grade_node_json(root: Path) -> tuple[bool, bool, bool, str]:
    return grade_node_test(root, "tests/format_json.test.js")


def grade_node_csv(root: Path) -> tuple[bool, bool, bool, str]:
    return grade_node_test(root, "tests/csv_to_json.test.js")


def grade_node_notes_api(root: Path) -> tuple[bool, bool, bool, str]:
    return grade_node_test(root, "tests/notes_api.test.js")


def grade_fastapi(root: Path) -> tuple[bool, bool, bool, str]:
    if not (root / "app.py").exists():
        return False, False, False, "app.py missing"
    tests = root / "tests" / "test_app.py"
    if tests.exists():
        cp = run_cmd(["python3", "-m", "pytest", "-q", "tests/test_app.py"], root, timeout=120)
        ok = cp.returncode == 0
        return ok, ok, ok, "" if ok else f"pytest failed: {(cp.stdout + cp.stderr)[-300:]}"
    return False, False, False, "tests/test_app.py missing"


CASES: dict[str, Case] = {
    "python_sales": Case(
        "python_sales",
        "coding",
        "Create a Python CLI in main.py that reads a CSV file path from argv. The CSV has columns salesperson,region,amount. Print JSON totals by salesperson sorted by salesperson name. Add tests in tests/test_main.py and run them. Do not create README or setup files.",
        setup_empty,
        grade_python_sales,
        ("main.py", "tests/", "sample.csv"),
    ),
    "docs_runbook": Case(
        "docs_runbook",
        "docs",
        "Prepare an operational triage runbook at runbooks/local-agent-triage.md for a local Ollama-backed agent. Include a symptom checklist, commands to collect logs, recovery steps, and escalation criteria. Do not modify code.",
        setup_empty,
        grade_docs,
        ("runbooks/",),
    ),
    "toml_merge": Case(
        "toml_merge",
        "coding",
        "Create merge_toml.py, a Python 3.9-compatible CLI that reads two simple TOML files from argv and prints merged TOML to stdout. The second file overrides duplicate keys. Support strings, integers, booleans, and one-level tables. Use only the Python standard library; do not use tomllib. Add pytest tests in tests/test_merge_toml.py and verify them.",
        setup_empty,
        grade_toml,
        ("merge_toml.py", "tests/", "a.toml", "b.toml"),
    ),
    "rust_word": Case(
        "rust_word",
        "coding",
        "Create a minimal Rust Cargo library. Implement count_words(input: &str) -> usize in src/lib.rs. Words are separated by ASCII whitespace. Add focused tests and run cargo test.",
        setup_empty,
        grade_cargo,
        ("Cargo.toml", "Cargo.lock", "src/", "tests/"),
    ),
    "rust_ndjson": Case(
        "rust_ndjson",
        "coding",
        "Create a minimal Rust Cargo library. Implement merge_ndjson_lines(left: &str, right: &str) -> String in src/lib.rs. It should concatenate non-empty NDJSON lines from left then right, preserving line order and ending with one trailing newline when output is non-empty. Add focused tests and run cargo test.",
        setup_empty,
        grade_cargo,
        ("Cargo.toml", "Cargo.lock", "src/", "tests/"),
    ),
    "node_json": Case(
        "node_json",
        "coding",
        "Create src/format_json.js and tests/format_json.test.js. Implement formatJson(input) that parses a JSON string and returns pretty JSON with two-space indentation and a trailing newline. Use node:test and verify with node --test tests/format_json.test.js. Do not create README files.",
        setup_empty,
        grade_node_json,
        ("package.json", "src/", "tests/"),
    ),
    "node_csv": Case(
        "node_csv",
        "coding",
        "Create src/csv_to_json.js and tests/csv_to_json.test.js. Implement csvToJson(input) for simple comma-separated CSV with a header row, returning an array of objects with string values. Use node:test and verify with node --test tests/csv_to_json.test.js. Do not create README files.",
        setup_empty,
        grade_node_csv,
        ("package.json", "src/", "tests/"),
    ),
    "fastapi_notes": Case(
        "fastapi_notes",
        "coding",
        "Create app.py and tests/test_app.py for a small FastAPI notes API. Implement GET /notes returning an empty list and POST /notes accepting JSON with title and body, returning the created note with id=1. Use fastapi.testclient in pytest tests and verify them.",
        setup_empty,
        grade_fastapi,
        ("app.py", "tests/"),
    ),
    "node_notes_api": Case(
        "node_notes_api",
        "coding",
        "Create src/notes_api.js and tests/notes_api.test.js for a small HTTP-style notes API module. Model GET /notes with getNotes() returning an empty list. Model POST /notes with createNote(requestJson) accepting a JSON object with title and body and returning the created note with id=1, title, and body. Use node:test and verify with node --test tests/notes_api.test.js. Do not install packages or create README files.",
        setup_empty,
        grade_node_notes_api,
        ("package.json", "src/", "tests/"),
    ),
    "python_markdown": Case(
        "python_markdown",
        "coding",
        "Create markdown_lint.py, a Python CLI that exits 0 for valid Markdown and non-zero when a line has trailing spaces or heading levels jump by more than one. Add pytest tests in tests/test_markdown_lint.py and verify them.",
        setup_empty,
        grade_python_markdown,
        ("markdown_lint.py", "tests/", "good.md", "bad.md"),
    ),
    "feature_discount": Case(
        "feature_discount",
        "feature",
        "Improve the existing discounts.py function final_price(price, percent). It should return the price after applying the percentage discount, rounded to 2 decimal places. Keep the existing zero-discount behavior and update tests/test_discounts.py. Do not create documentation or unrelated files.",
        setup_feature_discount,
        grade_feature_discount,
        ("discounts.py", "tests/", "__pycache__/"),
    ),
    "data_csv": Case(
        "data_csv",
        "data",
        "Read input/orders.csv and create output/order-summary.csv with exactly the same columns id,total and the same two data rows. This is a data-only task; do not create source code or tests.",
        setup_data_orders,
        grade_data_csv,
        ("input/", "output/"),
    ),
    "data_json": Case(
        "data_json",
        "data",
        "Read input/profile.json and create output/profile-summary.json with exactly the top-level fields id,total and no extra top-level fields. Preserve the id and total values from input/profile.json. This is a data-only task; do not create source code or tests.",
        setup_data_profile,
        grade_data_json,
        ("input/", "output/"),
    ),
    "research_cache": Case(
        "research_cache",
        "research",
        "Research and compare in-memory, SQLite, and file-based cache options for a local CLI agent. Write research/cache-strategy-brief.md with a recommendation, tradeoffs, failure modes, and an assumptions section. Do not modify code.",
        setup_empty,
        grade_research,
        ("research/",),
    ),
    "ops_health": Case(
        "ops_health",
        "ops",
        "Run ./scripts/health.sh and write reports/health-check.md containing the command, stdout, stderr, and exit code. Do not modify scripts/health.sh.",
        setup_ops_health,
        grade_ops,
        ("scripts/health.sh", "reports/"),
    ),
}


WP10_SEQUENCE = [
    "python_sales",
    "docs_runbook",
    "toml_merge",
    "rust_word",
    "rust_ndjson",
    "node_json",
    "node_csv",
    "fastapi_notes",
    "python_markdown",
    "data_csv",
    "research_cache",
    "ops_health",
    "python_sales",
    "docs_runbook",
    "toml_merge",
    "rust_word",
    "node_json",
    "node_csv",
    "fastapi_notes",
    "python_sales",
]

WP11_PER_VARIANT = [
    "python_sales",
    "python_sales",
    "python_sales",
    "python_sales",
    "docs_runbook",
    "docs_runbook",
    "toml_merge",
    "toml_merge",
    "toml_merge",
    "rust_word",
    "rust_word",
    "rust_ndjson",
    "rust_ndjson",
    "node_json",
    "node_json",
    "node_json",
    "node_csv",
    "node_csv",
    "fastapi_notes",
    "fastapi_notes",
    "fastapi_notes",
    "python_markdown",
    "data_csv",
    "research_cache",
    "ops_health",
]


def case_sequence(suite: str) -> list[tuple[str, str]]:
    if suite == "wp10":
        return [("no_pam", case_id) for case_id in WP10_SEQUENCE]
    if suite == "wp11":
        return [("no_pam", c) for c in WP11_PER_VARIANT] + [("pam", c) for c in WP11_PER_VARIANT]
    raise ValueError(f"unknown suite: {suite}")


def latest_eval_record(state_dir: Path) -> dict[str, object]:
    latest: dict[str, object] = {}
    for path in sorted(state_dir.glob("sessions/*/logs/eval.jsonl")):
        with path.open(encoding="utf-8") as fh:
            for line in fh:
                line = line.strip()
                if not line:
                    continue
                try:
                    latest = json.loads(line)
                except json.JSONDecodeError:
                    continue
    return latest


def pam_availability_from_eval(variant: str, record: dict[str, object]) -> tuple[str, str, str, str]:
    pam_eval = record.get("pam_eval")
    if not isinstance(pam_eval, dict):
        return ("unknown", "0", "", "")
    availability = str(pam_eval.get("availability") or "")
    injected_count = str(pam_eval.get("actual_injected_count") or 0)
    unused_reason = str(pam_eval.get("unused_reason") or "")
    failure_phase = str(pam_eval.get("failure_phase") or "")
    if availability:
        return (availability, injected_count, unused_reason, failure_phase)
    if variant == "no_pam" and unused_reason == "photon_unavailable":
        return ("disabled", injected_count, unused_reason, failure_phase)
    if unused_reason == "photon_unavailable" or unused_reason.startswith("context_pack_failed"):
        return ("failed", injected_count, unused_reason, failure_phase)
    if unused_reason in {"disabled", "plan_mode"}:
        return ("disabled", injected_count, unused_reason, failure_phase)
    try:
        actual = int(injected_count)
        suppressed = int(pam_eval.get("suppressed_count") or 0)
        would = int(pam_eval.get("would_inject_in_live_count") or 0)
    except (TypeError, ValueError):
        return ("unknown", injected_count, unused_reason, failure_phase)
    if actual > 0:
        return ("injected", injected_count, unused_reason, failure_phase)
    if suppressed > 0:
        return ("blocked_warning", injected_count, unused_reason, failure_phase)
    if would > 0 or variant == "pam":
        return ("not_injected", injected_count, unused_reason, failure_phase)
    return ("disabled", injected_count, unused_reason, failure_phase)


def shadow_terminal_from_eval(record: dict[str, object]) -> tuple[str, str, str, str]:
    shadow = record.get("shadow_terminal_projection")
    if not isinstance(shadow, dict):
        return ("unknown", "false", "", "")
    cls = str(shadow.get("class") or "unknown")
    conflict = bool_s(bool(shadow.get("conflict")))
    missing = ",".join(str(item) for item in (shadow.get("missing_evidence_ids") or []))
    failed = ",".join(str(item) for item in (shadow.get("failed_evidence_ids") or []))
    return (cls, conflict, missing, failed)


def run_one(
    *,
    suite: str,
    run_id: str,
    seq: int,
    variant: str,
    case: Case,
    run_dir: Path,
    anvil_bin: str,
    model: str,
    sidecar_model: str,
    max_iterations: int,
    chat_timeout_secs: int,
    timeout_secs: int,
) -> dict[str, str]:
    workdir = run_dir / "workdirs" / f"{seq:03d}-{variant}-{case.case_id}"
    state_dir = run_dir / "state" / f"{seq:03d}-{variant}-{case.case_id}"
    shutil.rmtree(workdir, ignore_errors=True)
    shutil.rmtree(state_dir, ignore_errors=True)
    workdir.mkdir(parents=True)
    state_dir.mkdir(parents=True)
    case.setup(workdir)
    before = snapshot(workdir)

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
        "--fresh-session",
        "--state-dir",
        str(state_dir),
        "--cwd",
        str(workdir),
        "--prompt",
        case.prompt,
    ]
    env = os.environ.copy()
    if variant == "pam":
        env.update(
            {
                "ANVIL_PHOTON_ENABLED": "true",
                "ANVIL_PHOTON_SHADOW_MODE": "false",
                "ANVIL_PHOTON_CANARY": "1000",
                "ANVIL_PHOTON_URL": "http://127.0.0.1:18765",
                "ANVIL_PHOTON_TIMEOUT_MS": "700",
            }
        )
    else:
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
        stdout = cp.stdout
        stderr = cp.stderr
        rc = cp.returncode
    except subprocess.TimeoutExpired as exc:
        stdout = exc.stdout if isinstance(exc.stdout, str) else (exc.stdout or b"").decode("utf-8", "replace")
        stderr = exc.stderr if isinstance(exc.stderr, str) else (exc.stderr or b"").decode("utf-8", "replace")
        stderr += f"\nTIMEOUT after {timeout_secs}s\n"
        rc = 124
    duration = time.monotonic() - started

    raw_dir = run_dir / "raw" / f"{seq:03d}-{variant}-{case.case_id}"
    raw_dir.mkdir(parents=True, exist_ok=True)
    write(raw_dir / "command.txt", " ".join(cmd) + "\n")
    write(raw_dir / "stdout.log", stdout)
    write(raw_dir / "stderr.log", stderr)

    after = snapshot(workdir)
    changed_files = changed(before, after)
    unexpected = allowed_changes(changed_files, case.allowed_prefixes)
    output = stdout + "\n" + stderr
    exit_reason = extract_exit_reason(state_dir, output)
    passed, high_quality, verification_pass, notes = case.grade(workdir)
    if rc == 124:
        passed = False
        high_quality = False
        verification_pass = False
        notes = "timeout"
    if unexpected:
        high_quality = False
        extra = "unexpected changes: " + ",".join(unexpected)
        notes = f"{notes}; {extra}" if notes else extra
    false_done = exit_reason == "done" and not passed
    false_missing = passed and ("missing" in exit_reason or "safe_stop" in exit_reason)
    repair_exhausted = "repair_exhausted" in exit_reason or "repair_safe_stop" in exit_reason or "repair_exhausted" in output
    max_iter = "max_iterations" in exit_reason or "max iterations" in output.lower()
    eval_record = latest_eval_record(state_dir)
    (
        pam_availability,
        pam_injected_count,
        pam_unused_reason,
        pam_failure_phase,
    ) = pam_availability_from_eval(variant, eval_record)
    shadow_class, shadow_conflict, shadow_missing, shadow_failed = shadow_terminal_from_eval(
        eval_record
    )

    return {
        "suite": suite,
        "run_id": run_id,
        "seq": str(seq),
        "variant": variant,
        "case_id": case.case_id,
        "task_kind": case.task_kind,
        "pass": bool_s(passed),
        "high_quality": bool_s(high_quality),
        "verification_pass": bool_s(verification_pass),
        "anvil_rc": str(rc),
        "exit_reason": exit_reason,
        "duration_sec": f"{duration:.1f}",
        "changed_files": ",".join(changed_files),
        "unexpected_changes": ",".join(unexpected),
        "false_done": bool_s(false_done),
        "false_missing": bool_s(false_missing),
        "repair_exhausted": bool_s(repair_exhausted),
        "max_iterations": bool_s(max_iter),
        "pam_availability": pam_availability,
        "pam_injected_count": pam_injected_count,
        "pam_unused_reason": pam_unused_reason,
        "pam_failure_phase": pam_failure_phase,
        "shadow_terminal_class": shadow_class,
        "shadow_terminal_conflict": shadow_conflict,
        "shadow_missing_evidence": shadow_missing,
        "shadow_failed_evidence": shadow_failed,
        "notes": notes,
    }


def summarize(rows: list[dict[str, str]]) -> dict[str, object]:
    total = len(rows)
    def count(field: str, value: str = "true") -> int:
        return sum(1 for row in rows if row.get(field) == value)

    by_case: dict[str, dict[str, int]] = {}
    for row in rows:
        case = row["case_id"]
        bucket = by_case.setdefault(case, {"total": 0, "pass": 0, "hq": 0})
        bucket["total"] += 1
        if row["pass"] == "true":
            bucket["pass"] += 1
        if row["high_quality"] == "true":
            bucket["hq"] += 1
    by_variant: dict[str, dict[str, int]] = {}
    by_pam_availability: dict[str, dict[str, int]] = {}
    by_shadow_terminal: dict[str, dict[str, int]] = {}
    for row in rows:
        variant = row["variant"]
        bucket = by_variant.setdefault(variant, {"total": 0, "pass": 0, "hq": 0})
        bucket["total"] += 1
        if row["pass"] == "true":
            bucket["pass"] += 1
        if row["high_quality"] == "true":
            bucket["hq"] += 1
        availability = row.get("pam_availability") or "unknown"
        availability_key = f"{variant}:{availability}"
        avail_bucket = by_pam_availability.setdefault(
            availability_key, {"total": 0, "pass": 0, "hq": 0}
        )
        avail_bucket["total"] += 1
        if row["pass"] == "true":
            avail_bucket["pass"] += 1
        if row["high_quality"] == "true":
            avail_bucket["hq"] += 1
        shadow_key = row.get("shadow_terminal_class") or "unknown"
        shadow_bucket = by_shadow_terminal.setdefault(
            shadow_key, {"total": 0, "pass": 0, "hq": 0}
        )
        shadow_bucket["total"] += 1
        if row["pass"] == "true":
            shadow_bucket["pass"] += 1
        if row["high_quality"] == "true":
            shadow_bucket["hq"] += 1
    return {
        "total": total,
        "pass": count("pass"),
        "high_quality": count("high_quality"),
        "verification_pass": count("verification_pass"),
        "false_done": count("false_done"),
        "false_missing": count("false_missing"),
        "repair_exhausted": count("repair_exhausted"),
        "max_iterations": count("max_iterations"),
        "shadow_conflict": count("shadow_terminal_conflict"),
        "by_case": by_case,
        "by_variant": by_variant,
        "by_pam_availability": by_pam_availability,
        "by_shadow_terminal": by_shadow_terminal,
    }


def write_summary(run_dir: Path, rows: list[dict[str, str]]) -> None:
    summary = summarize(rows)
    (run_dir / "summary.json").write_text(json.dumps(summary, indent=2, sort_keys=True) + "\n", encoding="utf-8")
    lines = [
        f"# {rows[0]['suite'].upper()} Evaluation Summary\n",
        "",
        f"- run_id: `{rows[0]['run_id']}`",
        f"- total: {summary['total']}",
        f"- pass: {summary['pass']}/{summary['total']}",
        f"- high_quality: {summary['high_quality']}/{summary['total']}",
        f"- verification_pass: {summary['verification_pass']}/{summary['total']}",
        f"- false_done: {summary['false_done']}",
        f"- false_missing: {summary['false_missing']}",
        f"- repair_exhausted: {summary['repair_exhausted']}",
        f"- max_iterations: {summary['max_iterations']}",
        f"- shadow_conflict: {summary['shadow_conflict']}",
        "",
        "## By Variant",
        "",
        "| variant | pass | high_quality | total |",
        "| --- | ---: | ---: | ---: |",
    ]
    for variant, bucket in sorted(summary["by_variant"].items()):
        lines.append(f"| {variant} | {bucket['pass']} | {bucket['hq']} | {bucket['total']} |")
    lines.extend(
        [
            "",
            "## By PAM Availability",
            "",
            "| variant:availability | pass | high_quality | total |",
            "| --- | ---: | ---: | ---: |",
        ]
    )
    for key, bucket in sorted(summary["by_pam_availability"].items()):
        lines.append(f"| {key} | {bucket['pass']} | {bucket['hq']} | {bucket['total']} |")
    lines.extend(
        [
            "",
            "## By Shadow Terminal",
            "",
            "| shadow_terminal_class | pass | high_quality | total |",
            "| --- | ---: | ---: | ---: |",
        ]
    )
    for key, bucket in sorted(summary["by_shadow_terminal"].items()):
        lines.append(f"| {key} | {bucket['pass']} | {bucket['hq']} | {bucket['total']} |")
    lines.extend(["", "## By Case", "", "| case | pass | high_quality | total |", "| --- | ---: | ---: | ---: |"])
    for case_id, bucket in sorted(summary["by_case"].items()):
        lines.append(f"| {case_id} | {bucket['pass']} | {bucket['hq']} | {bucket['total']} |")
    failures = [row for row in rows if row["high_quality"] != "true"]
    if failures:
        lines.extend(["", "## Non High-Quality Rows", ""])
        for row in failures:
            note = row["notes"] or row["exit_reason"] or "no note"
            lines.append(
                f"- {row['seq']} {row['variant']} {row['case_id']}: "
                f"pass={row['pass']} exit={row['exit_reason']} note={note}"
            )
    (run_dir / "summary.md").write_text("\n".join(lines) + "\n", encoding="utf-8")


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--suite", choices=["wp10", "wp11"], required=True)
    parser.add_argument(
        "--case-sequence",
        default="",
        help="Optional comma-separated case ids. When set, overrides the suite default sequence.",
    )
    parser.add_argument(
        "--variant",
        choices=["no_pam", "pam"],
        default="no_pam",
        help="Variant used with --case-sequence.",
    )
    parser.add_argument("--run-id", required=True)
    parser.add_argument("--out-root", default="workspace/v0.6.11/eval-runs")
    parser.add_argument("--anvil-bin", default="target/debug/anvil")
    parser.add_argument("--model", default="qwen3.6:27b-coding-nvfp4")
    parser.add_argument("--sidecar-model", default="qwen3-coder:30b")
    parser.add_argument("--max-iterations", type=int, default=20)
    parser.add_argument("--chat-timeout-secs", type=int, default=180)
    parser.add_argument("--timeout-secs", type=int, default=420)
    return parser.parse_args()


def main() -> int:
    args = parse_args()
    repo = Path(__file__).resolve().parents[2]
    run_dir = repo / args.out_root / args.run_id
    if run_dir.exists():
        raise SystemExit(f"run dir already exists: {run_dir}")
    run_dir.mkdir(parents=True)
    if args.case_sequence.strip():
        seqs = [
            (args.variant, case_id.strip())
            for case_id in args.case_sequence.split(",")
            if case_id.strip()
        ]
        unknown = [case_id for _, case_id in seqs if case_id not in CASES]
        if unknown:
            raise SystemExit(f"unknown case id(s): {', '.join(unknown)}")
    else:
        seqs = case_sequence(args.suite)
    rows: list[dict[str, str]] = []
    for idx, (variant, case_id) in enumerate(seqs, start=1):
        case = CASES[case_id]
        print(f"{args.suite} {idx}/{len(seqs)} {variant} {case_id}", flush=True)
        rows.append(
            run_one(
                suite=args.suite,
                run_id=args.run_id,
                seq=idx,
                variant=variant,
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
    with (run_dir / "results.csv").open("w", newline="", encoding="utf-8") as fh:
        writer = csv.DictWriter(fh, fieldnames=RESULT_FIELDS)
        writer.writeheader()
        writer.writerows(rows)
    write_summary(run_dir, rows)
    summary = summarize(rows)
    print(f"wrote {run_dir / 'results.csv'}")
    print(f"pass={summary['pass']}/{summary['total']} high_quality={summary['high_quality']}/{summary['total']}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
