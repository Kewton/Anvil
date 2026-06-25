#!/usr/bin/env python3
from __future__ import annotations

import argparse
import concurrent.futures
import json
import os
import shutil
import sys
import threading
import time
from pathlib import Path

from eval_lib.artifacts import create_run_root, write_json, write_jsonl
from eval_lib.config import merge_dotenv_into_env
from eval_lib.failure_classification import classify_failure, read_jsonl
from eval_lib.matrix import expand_matrix, parse_modes
from eval_lib.models import ModelRef, load_model_profiles
from eval_lib.plan_scoring import score_plan_file
from eval_lib.postcheck import is_dependency_command, run_postcheck
from eval_lib.redaction import redact_json
from eval_lib.process import run_capture
from eval_lib.run_summary import calculate_overall, empty_summary_row, write_summary
from eval_lib.suites import load_suite


def main() -> int:
    parser = argparse.ArgumentParser(description="Run anvilminimal eval matrix.")
    parser.add_argument("--suite", required=True)
    parser.add_argument("--model-profile", required=True)
    parser.add_argument("--model-profiles", default="eval/model_profiles.yaml")
    parser.add_argument("--modes", default="minimal-loop,step-plan,plan-run,ultra-plan-run")
    parser.add_argument("--scenario")
    parser.add_argument("--runs", type=int, default=1)
    parser.add_argument("--parallel", type=int, default=4)
    parser.add_argument("--context-budget", type=int, default=65536)
    parser.add_argument("--binary", default="anvilminimal")
    parser.add_argument("--run-root")
    parser.add_argument("--timeout-sec", type=int)
    parser.add_argument("--dry-run", action="store_true")
    parser.add_argument(
        "--provider-smoke-summary",
        help="Path to mvp-provider-smoke summary.eval.tsv that must be green before this run.",
    )
    parser.add_argument(
        "--allow-provider-smoke-failure",
        action="store_true",
        help="Explicitly bypass a failed provider smoke summary.",
    )
    args = parser.parse_args()

    run_root = Path(args.run_root) if args.run_root else create_run_root()
    suite = load_suite(args.suite)
    profiles, warnings = load_model_profiles(args.model_profiles)
    if args.model_profile not in profiles:
        raise SystemExit(f"unknown model profile: {args.model_profile}")
    if args.provider_smoke_summary and not args.allow_provider_smoke_failure:
        assert_provider_smoke_green(Path(args.provider_smoke_summary))
    matrix = expand_matrix(
        suite,
        profiles[args.model_profile],
        parse_modes(args.modes),
        args.runs,
        args.context_budget,
        args.binary,
        scenario_filter=args.scenario,
    )
    write_jsonl(run_root / "warnings.jsonl", warnings)
    write_json(run_root / "matrix.json", scrub_matrix(matrix))
    print(f"[eval] run_root={run_root}")
    print(f"[matrix] total_runs={len(matrix)} dry_run={str(args.dry_run).lower()}")

    prepared = []
    rows = []
    events: list[dict] = []
    for index, spec in enumerate(matrix, start=1):
        run_dir = run_root / "runs" / spec["run_id"]
        workdir = run_dir / "workdir"
        run_dir.mkdir(parents=True, exist_ok=True)
        workdir.mkdir(parents=True, exist_ok=True)
        command = actual_command(spec, workdir)
        contract = completion_contract_for_spec(spec)
        if contract:
            contract_path = run_dir / "completion-contract.json"
            write_json(contract_path, redact_json(contract))
            command = inject_completion_contract_arg(command, contract_path)
        (run_dir / "command.txt").write_text(" ".join(json.dumps(part) for part in command) + "\n", encoding="utf-8")
        write_json(run_dir / "meta.json", scrub_spec(spec))
        if args.dry_run:
            rows.append({**empty_summary_row(spec), "workdir": str(workdir), "extras_json": {"dry_run": True}})
            continue
        prepared.append((index, spec, command, run_dir, workdir))

    if args.dry_run:
        print("[done] dry-run complete")
        return 0
    provider_limit = profiles[args.model_profile].get("provider_limit", 2)
    semaphores = {
        "openai": threading.Semaphore(provider_limit),
        "gemini": threading.Semaphore(provider_limit),
    }
    parallel_items = [item for item in prepared if not item[1].get("serial_lane") and not item[1].get("port_mutex")]
    serial_items = [item for item in prepared if item not in parallel_items]
    results: list[tuple[int, dict, list[dict]]] = []
    if parallel_items:
        print(f"[scheduler] cloud_parallel={min(args.parallel, len(parallel_items))} serial={len(serial_items)}")
        with concurrent.futures.ThreadPoolExecutor(max_workers=max(1, args.parallel)) as pool:
            futures = [pool.submit(run_prepared, item, args.timeout_sec, semaphores, len(matrix)) for item in parallel_items]
            for future in concurrent.futures.as_completed(futures):
                results.append(future.result())
    for item in serial_items:
        results.append(run_prepared(item, args.timeout_sec, semaphores, len(matrix)))
    for _, row, run_events in sorted(results, key=lambda item: item[0]):
        rows.append(row)
        events.extend(run_events)
    apply_time_scores(rows)
    write_summary(run_root / "summary.eval.tsv", rows)
    write_jsonl(run_root / "events.jsonl", events)
    print(f"[write] {run_root / 'summary.eval.tsv'}")
    print(f"[write] {run_root / 'events.jsonl'}")
    failed = [row for row in rows if row.get("success") not in {True, "true", "diagnostic_skipped"}]
    skipped = [row for row in rows if row.get("success") == "diagnostic_skipped"]
    print(f"[done] success={len(rows)-len(failed)-len(skipped)} failed={len(failed)} skipped={len(skipped)}")
    return 1 if failed else 0


def run_prepared(
    item: tuple[int, dict, list[str], Path, Path],
    timeout_sec: int | None,
    semaphores: dict[str, threading.Semaphore],
    total: int,
) -> tuple[int, dict, list[dict]]:
    index, spec, command, run_dir, workdir = item
    providers = sorted({spec["main"]["provider"], spec["planner"]["provider"]})
    acquired: list[threading.Semaphore] = []
    try:
        for provider in providers:
            sem = semaphores.get(provider)
            if sem:
                sem.acquire()
                acquired.append(sem)
        print(
            f"[run {index:02d}/{total:02d}] {spec['scenario']['id']} {spec['mode']} "
            f"{spec['main']['provider']}:{spec['main']['model']}"
        )
        row, run_events = run_one(spec, command, run_dir, workdir, timeout_sec)
        print(
            f"[result] {spec['run_id']} success={row['success']} rc={row['rc']} "
            f"elapsed={row['exec_elapsed_sec']}"
        )
        return index, row, run_events
    finally:
        for sem in reversed(acquired):
            sem.release()


def actual_command(spec: dict, workdir: Path) -> list[str]:
    command = list(spec["command"])
    return [str(workdir) if part == "workdir" else part for part in command]


def completion_contract_for_spec(spec: dict) -> dict | None:
    if spec.get("mode") != "minimal-loop":
        return None
    scenario = spec.get("scenario", {}) or {}
    required_paths = list(dict.fromkeys(scenario.get("expected_artifacts", []) or []))
    commands = list(scenario.get("postcheck", {}).get("commands", []) or [])
    has_dependency_setup = any(is_dependency_command(command) for command in commands)
    verify_commands = [
        command
        for command in commands
        if deterministic_verify_command(command, has_dependency_setup)
    ]
    if not required_paths and not verify_commands:
        return None
    return {
        "required_paths": required_paths,
        "verify_commands": verify_commands,
        "verify_repair_cap": 2,
        "source": "eval_scenario",
    }


def deterministic_verify_command(command: str, has_dependency_setup: bool) -> bool:
    lowered = command.lower().strip()
    if not lowered or is_dependency_command(command):
        return False
    if "&&" in lowered or "||" in lowered or "|" in lowered or ";" in lowered:
        return False
    if "next dev" in lowered or "vite --host" in lowered:
        return False
    if has_dependency_setup and (
        lowered.startswith("npm ")
        or lowered.startswith("pnpm ")
        or lowered.startswith("yarn ")
    ):
        return False
    return True


def inject_completion_contract_arg(command: list[str], contract_path: Path) -> list[str]:
    injected = ["--completion-contract-json", str(contract_path)]
    action_flags = {
        "--prompt",
        "--plan-steps",
        "--plan-run",
        "--run-plan",
        "--ultra-plan",
        "--ultra-plan-run",
        "--run-ultra-plan",
    }
    for index, part in enumerate(command):
        if part in action_flags:
            return [*command[:index], *injected, *command[index:]]
    return [*command, *injected]


def run_one(spec: dict, command: list[str], run_dir: Path, workdir: Path, timeout_sec: int | None) -> tuple[dict, list[dict]]:
    row = empty_summary_row(spec)
    row["workdir"] = str(workdir)
    events = []
    if spec["mode"] == "ultra-step-run":
        row.update(
            {
                "rc": "",
                "success": "diagnostic_skipped",
                "execution_score": "",
                "time_score": "",
                "overall_score": "",
                "extras_json": {"reason": "phase snapshot not available in initial matrix run"},
            }
        )
        events.append({"event": "diagnostic_skipped", "run_id": spec["run_id"], "mode": spec["mode"]})
        return row, events

    env = merge_dotenv_into_env(os.environ.copy())
    env["ANVIL_EVAL_EVENTS"] = str(run_dir / "anvil-events.jsonl")
    scenario_timeout = timeout_sec or int(spec["scenario"].get("timeouts", {}).get("total_sec", 1800))
    start = time.monotonic()
    result = run_capture(
        command,
        cwd=Path.cwd(),
        timeout_sec=scenario_timeout,
        stdout_path=run_dir / "stdout.log",
        stderr_path=run_dir / "stderr.log",
        env=env,
    )
    process_elapsed = result.elapsed_sec
    child_events = read_jsonl(run_dir / "anvil-events.jsonl")
    for event in child_events:
        event.setdefault("run_id", spec["run_id"])
    events.extend(child_events)
    plans = collect_plans(workdir, run_dir)
    plan_score = None
    ultra_score = None
    for plan in plans:
        score = score_plan_file(plan, spec["scenario"])
        events.append({"event": "plan_score", "run_id": spec["run_id"], "plan": plan.name, **score})
        if score["kind"] == "ultra":
            ultra_score = float(score["score"])
        else:
            plan_score = float(score["score"])

    post = {"ok": True, "postcheck_elapsed_sec": 0.0, "dependency_elapsed_sec": 0.0}
    if result.rc == 0 and spec["mode"] != "step-plan":
        post = run_postcheck(spec["scenario"], workdir, run_dir / "postcheck", timeout_sec=scenario_timeout)
        events.append({"event": "postcheck_summary", "run_id": spec["run_id"], **post})
    success = result.rc == 0 and bool(post["ok"])
    diagnostics = summarize_run_events(events, post)
    execution_score = 100.0 if success else 0.0
    time_score = 100.0
    extras = {
        "timeout": result.timeout,
        "metric_source": "events+process" if child_events else "process",
        "elapsed_total_sec": round(time.monotonic() - start, 3),
    }
    if not success:
        stderr = (run_dir / "stderr.log").read_text(encoding="utf-8", errors="replace")
        extras.update(
            {
                key: value
                for key, value in classify_failure(
                    events=events,
                    stderr=stderr,
                    rc=result.rc,
                    timeout=result.timeout,
                    post_ok=bool(post["ok"]),
                ).items()
                if value not in {"", None}
            }
        )
    row.update(
        {
            "rc": result.rc,
            "success": success,
            "queue_wait_sec": 0.0,
            "process_elapsed_sec": round(process_elapsed, 3),
            "exec_elapsed_sec": round(process_elapsed + float(post["postcheck_elapsed_sec"]) - float(post["dependency_elapsed_sec"]), 3),
            "model_elapsed_sec": "",
            "tool_elapsed_sec": "",
            "postcheck_elapsed_sec": post["postcheck_elapsed_sec"],
            "dependency_elapsed_sec": post["dependency_elapsed_sec"],
            "iterations": "",
            "tool_calls": "",
            "files_changed": count_files(workdir),
            "stop_reason": diagnostics["stop_reason"],
            "last_blocking_reason": diagnostics["last_blocking_reason"],
            "missing_artifacts": diagnostics["missing_artifacts"],
            "verify_attempts": diagnostics["verify_attempts"],
            "last_provider_error_kind": diagnostics["last_provider_error_kind"],
            "last_provider_http_status": diagnostics["last_provider_http_status"],
            "provider_attempts": diagnostics["provider_attempts"],
            "fallback_decision": diagnostics["fallback_decision"],
            "plan_quality_score": plan_score if plan_score is not None else "",
            "ultra_phase_quality_score": ultra_score if ultra_score is not None else "",
            "execution_score": execution_score,
            "time_score": time_score,
            "overall_score": calculate_overall(spec["mode"], plan_score, ultra_score, execution_score, time_score),
            "plan_artifacts": ",".join(str(path.relative_to(run_dir)) for path in plans),
            "extras_json": extras,
        }
    )
    return row, events


def summarize_run_events(events: list[dict], post: dict) -> dict[str, str]:
    loop_stop = next((event for event in reversed(events) if event.get("event") == "loop_stop"), {})
    completion_verify = [
        event for event in events if event.get("event") == "completion_verify"
    ]
    provider_error = next(
        (event for event in reversed(events) if event.get("event") == "provider_error"),
        {},
    )
    provider_response = next(
        (event for event in reversed(events) if event.get("event") == "provider_response"),
        {},
    )
    fallback = next(
        (event for event in reversed(events) if event.get("event") == "fallback_decision"),
        {},
    )
    tool_error = next(
        (
            event
            for event in reversed(events)
            if event.get("event") == "tool_execute" and event.get("status") == "error"
        ),
        {},
    )
    missing = post.get("missing_artifacts") or loop_stop.get("missing_paths") or []
    return {
        "stop_reason": str(loop_stop.get("reason", "")),
        "last_blocking_reason": str(
            loop_stop.get(
                "primary_reason",
                loop_stop.get(
                    "last_blocking_reason",
                    tool_error_reason(tool_error),
                ),
            )
        ),
        "missing_artifacts": ",".join(str(item) for item in missing),
        "verify_attempts": str(len(completion_verify)) if completion_verify else "",
        "last_provider_error_kind": str(provider_error.get("error_kind", "")),
        "last_provider_http_status": str(provider_error.get("status", "")),
        "provider_attempts": str(
            provider_error.get("attempt", provider_response.get("attempt", ""))
        ),
        "fallback_decision": fallback_decision_cell(fallback),
    }


def fallback_decision_cell(event: dict) -> str:
    if not event:
        return ""
    allowed = event.get("allowed")
    if allowed is True:
        return "allowed"
    if allowed is False:
        return "blocked"
    return str(allowed)


def tool_error_reason(event: dict) -> str:
    if not event:
        return ""
    name = event.get("name", "")
    kind = event.get("error_kind", "")
    return f"{name} {kind}".strip()


def collect_plans(workdir: Path, run_dir: Path) -> list[Path]:
    src = workdir / ".anvil" / "plans"
    dst = run_dir / "plans"
    copied: list[Path] = []
    if not src.exists():
        return copied
    dst.mkdir(parents=True, exist_ok=True)
    for plan in sorted(src.glob("*.yaml")):
        target = dst / plan.name
        shutil.copy2(plan, target)
        copied.append(target)
    return copied


def count_files(root: Path) -> int:
    if not root.exists():
        return 0
    skip = {"node_modules", "target", ".next", ".git", ".anvil"}
    count = 0
    for path in root.rglob("*"):
        if any(part in skip for part in path.parts):
            continue
        if path.is_file():
            count += 1
    return count


def apply_time_scores(rows: list[dict]) -> None:
    groups: dict[tuple, float] = {}
    for row in rows:
        try:
            elapsed = float(row.get("exec_elapsed_sec", ""))
        except (TypeError, ValueError):
            continue
        key = (row["scenario"], row["mode"], row["local_llm_used"])
        if elapsed > 0:
            groups[key] = min(groups.get(key, elapsed), elapsed)
    for row in rows:
        try:
            elapsed = float(row.get("exec_elapsed_sec", ""))
        except (TypeError, ValueError):
            continue
        key = (row["scenario"], row["mode"], row["local_llm_used"])
        fastest = groups.get(key)
        if fastest and elapsed > 0:
            row["time_score"] = round(max(0.0, min(100.0, 100.0 * fastest / elapsed)), 1)
            plan = float(row["plan_quality_score"]) if row.get("plan_quality_score") not in {"", None} else None
            ultra = float(row["ultra_phase_quality_score"]) if row.get("ultra_phase_quality_score") not in {"", None} else None
            execution = float(row["execution_score"]) if row.get("execution_score") not in {"", None} else 0.0
            row["overall_score"] = calculate_overall(row["mode"], plan, ultra, execution, float(row["time_score"]))


def assert_provider_smoke_green(summary_path: Path) -> None:
    from eval_lib.run_summary import read_summary

    rows = read_summary(summary_path)
    required = [row for row in rows if row.get("success") != "diagnostic_skipped"]
    failed = [row for row in required if row.get("success") != "true"]
    if failed:
        raise SystemExit(
            f"provider smoke failed ({len(failed)}/{len(required)}); "
            "rerun provider smoke or pass --allow-provider-smoke-failure explicitly"
        )


def scrub_matrix(matrix: list[dict]) -> list[dict]:
    return [scrub_spec(row) for row in matrix]


def scrub_spec(spec: dict) -> dict:
    return json.loads(json.dumps(spec, ensure_ascii=False, default=str))


if __name__ == "__main__":
    sys.exit(main())
