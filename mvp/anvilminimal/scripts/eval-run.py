#!/usr/bin/env python3
from __future__ import annotations

import argparse
import concurrent.futures
import json
import os
import shutil
import statistics
import sys
import threading
import time
from pathlib import Path

from eval_lib.acceptance_outcome import evaluate_acceptance_outcome
from eval_lib.acceptance_contract import contract_from_scenario
from eval_lib.artifacts import create_run_root, write_json, write_jsonl
from eval_lib.config import merge_dotenv_into_env
from eval_lib.failure_classification import (
    capability_failure_included,
    classify_failure,
    failure_layer_for_kind,
    read_jsonl,
)
from eval_lib.matrix import expand_matrix, parse_modes
from eval_lib.models import ModelRef, load_model_profiles
from eval_lib.plan_readiness import (
    READINESS_FIELDS,
    aggregate_ultra_phase_readiness,
    classify_readiness_outcome,
    empty_plan_readiness_scores,
    score_plan_readiness_file,
)
from eval_lib.plan_capability_contract import score_plan_capability_contract
from eval_lib.plan_verify_coverage import score_plan_verify_coverage
from eval_lib.plan_scoring import score_plan_file
from eval_lib.postcheck import is_dependency_command, run_postcheck
from eval_lib.redaction import redact_json
from eval_lib.process import run_capture
from eval_lib.run_summary import (
    calculate_overall,
    calculate_plan_run_predictive_score,
    empty_summary_row,
    write_summary,
)
from eval_lib.runtime_scoring import score_runtime_health
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
    parser.add_argument("--provider-limit", type=int)
    parser.add_argument("--context-budget", type=int, default=65536)
    parser.add_argument("--binary", default="anvilminimal")
    parser.add_argument(
        "--binary-kind",
        choices=["auto", "anvilminimal", "anvildev"],
        default="auto",
        help="CLI dialect for --binary. auto treats anvildev/anvil as source anvildev and otherwise anvilminimal.",
    )
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
    profile_config = profiles[args.model_profile]
    if args.provider_smoke_summary and not args.allow_provider_smoke_failure:
        assert_provider_smoke_green(Path(args.provider_smoke_summary))
    provider_limit = args.provider_limit if args.provider_limit is not None else profile_config.get("provider_limit", 2)
    matrix = expand_matrix(
        suite,
        profile_config,
        parse_modes(args.modes),
        args.runs,
        args.context_budget,
        args.binary,
        binary_kind=args.binary_kind,
        scenario_filter=args.scenario,
    )
    for spec in matrix:
        spec["_provider_limit"] = provider_limit
        spec["_parallel_limit"] = args.parallel
        spec["_model_profile"] = args.model_profile
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
            rows.append({
                **empty_summary_row(spec),
                "workdir": str(workdir),
                "extras_json": {
                    "dry_run": True,
                    "provider_probe": spec.get("provider_probe", {}),
                },
            })
            continue
        prepared.append((index, spec, command, run_dir, workdir))

    if args.dry_run:
        print("[done] dry-run complete")
        return 0
    semaphores = {
        "openai": threading.Semaphore(provider_limit),
        "gemini": threading.Semaphore(provider_limit),
    }
    parallel_items = [item for item in prepared if not item[1].get("serial_lane") and not item[1].get("port_mutex")]
    serial_items = [item for item in prepared if item not in parallel_items]
    results: list[tuple[int, dict, list[dict]]] = []
    if parallel_items:
        print(f"[scheduler] cloud_parallel={min(args.parallel, len(parallel_items))} serial={len(serial_items)}")
        queued_at = time.monotonic()
        for _, spec, _, _, _ in parallel_items:
            spec["_queued_at"] = queued_at
            spec["_scheduler_lane"] = "cloud"
            spec["_effective_parallelism"] = min(args.parallel, len(parallel_items))
        with concurrent.futures.ThreadPoolExecutor(max_workers=max(1, args.parallel)) as pool:
            futures = [pool.submit(run_prepared, item, args.timeout_sec, semaphores, len(matrix)) for item in parallel_items]
            for future in concurrent.futures.as_completed(futures):
                results.append(future.result())
    for item in serial_items:
        _, spec, _, _, _ = item
        spec["_queued_at"] = time.monotonic()
        spec["_scheduler_lane"] = "serial"
        spec["_effective_parallelism"] = 1
        results.append(run_prepared(item, args.timeout_sec, semaphores, len(matrix)))
    for _, row, run_events in sorted(results, key=lambda item: item[0]):
        rows.append(row)
        events.extend(run_events)
    apply_time_scores(rows)
    apply_stability_scores(rows)
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
    started_at = time.monotonic()
    queued_at = float(spec.get("_queued_at", started_at) or started_at)
    queue_wait_sec = max(0.0, started_at - queued_at)
    provider_wait_start = time.monotonic()
    provider_wait_sec = 0.0
    try:
        for provider in providers:
            sem = semaphores.get(provider)
            if sem:
                sem.acquire()
                acquired.append(sem)
        provider_wait_sec = max(0.0, time.monotonic() - provider_wait_start)
        print(
            f"[run {index:02d}/{total:02d}] {spec['scenario']['id']} {spec['mode']} "
            f"{spec['main']['provider']}:{spec['main']['model']}"
        )
        row, run_events = run_one(spec, command, run_dir, workdir, timeout_sec)
        wall_clock_sec = max(0.0, time.monotonic() - queued_at)
        serial_reason = ""
        if spec.get("port_mutex"):
            serial_reason = "port_mutex"
        elif spec.get("serial_lane"):
            serial_reason = "local_llm_or_profile_serial"
        row.update(
            {
                "queue_wait_sec": round(queue_wait_sec, 3),
                "provider_wait_sec": round(provider_wait_sec, 3),
                "port_mutex_wait_sec": 0.0,
                "scheduler_lane": spec.get("_scheduler_lane", ""),
                "serial_reason": serial_reason,
                "effective_parallelism": spec.get("_effective_parallelism", ""),
                "provider_limit": spec.get("_provider_limit", ""),
                "parallel_limit": spec.get("_parallel_limit", ""),
                "wall_clock_sec": round(wall_clock_sec, 3),
            }
        )
        row.setdefault("extras_json", {})
        row["extras_json"].update(
            {
                "scheduler_lane": row["scheduler_lane"],
                "provider_limit": row["provider_limit"],
                "parallel_limit": row["parallel_limit"],
                "provider_wait_sec": row["provider_wait_sec"],
                "queue_wait_sec": row["queue_wait_sec"],
                "wall_clock_sec": row["wall_clock_sec"],
            }
        )
        run_events.append(
            {
                "event": "scheduler_diagnostics",
                "run_id": spec["run_id"],
                "queue_wait_sec": row["queue_wait_sec"],
                "provider_wait_sec": row["provider_wait_sec"],
                "scheduler_lane": row["scheduler_lane"],
                "provider_limit": row["provider_limit"],
                "parallel_limit": row["parallel_limit"],
            }
        )
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
    rendered = []
    for part in command:
        if part == "workdir":
            rendered.append(str(workdir))
        elif isinstance(part, str) and part.startswith("workdir/"):
            rendered.append(str(workdir / part.removeprefix("workdir/")))
        else:
            rendered.append(part)
    return rendered


def completion_contract_for_spec(spec: dict) -> dict | None:
    if spec.get("binary_kind", "anvilminimal") != "anvilminimal":
        return None
    if spec.get("mode") not in {"minimal-loop", "plan-run", "ultra-plan-run"}:
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
    deferred_requirements = [
        {
            "command": command,
            "reason": "requires dependency setup",
            "authority": "eval_setup",
            "profile": scenario.get("profile", "generic"),
            "status": "blocked_by_dependency_setup",
        }
        for command in commands
        if deferred_verify_requirement(command, has_dependency_setup)
    ]
    acceptance_contract = contract_from_scenario(scenario)
    required_capabilities = unique_strings(acceptance_contract.required_capabilities)
    required_obligations = unique_strings(acceptance_contract.required_obligations)
    deterministic_oracles = unique_strings(
        acceptance_contract.oracle_contract.get("deterministic_oracles", []) or []
    )
    required_evidence = unique_strings(
        evidence
        for capability in required_capabilities
        for evidence in required_evidence_for_capability(capability)
    )
    profile = scenario.get("profile", "generic")
    profile_value = profile if profile not in {"", "generic", "default", "none"} else None
    if (
        not required_paths
        and not verify_commands
        and not deferred_requirements
        and not profile_value
        and not required_capabilities
        and not required_evidence
    ):
        return None
    contract = {
        "required_paths": required_paths,
        "verify_commands": verify_commands,
        "required_capabilities": required_capabilities,
        "required_obligations": required_obligations,
        "deterministic_oracles": deterministic_oracles,
        "required_evidence": required_evidence,
        "verify_repair_cap": 3 if required_capabilities or required_evidence else 2,
        "source": "eval_scenario",
    }
    if profile_value:
        contract["profile"] = profile_value
        contract["goal"] = scenario.get("prompt", "")
    if deferred_requirements:
        contract["deferred_verify_requirements"] = deferred_requirements
    return contract


def unique_strings(values) -> list[str]:
    out: list[str] = []
    seen: set[str] = set()
    for value in values or []:
        text = str(value).strip()
        if text and text not in seen:
            seen.add(text)
            out.append(text)
    return out


def required_evidence_for_capability(capability: str) -> list[str]:
    mapping = {
        "implementation": ["implementation_artifact"],
        "entrypoint": ["implementation_artifact"],
        "input_output_contract": ["implementation_artifact"],
        "requested_content": ["requested_content_evidence"],
        "deterministic_test": ["test_artifact", "bound_verify_command"],
        "deterministic_check": [
            "bound_verify_command",
            "non_zero_test_or_assertion_evidence",
        ],
        "buildable": ["build_command_or_dependency_missing_boundary"],
        "browser_interaction": [
            "implementation_artifact",
            "interactive_ui_source_evidence",
            "non_static_screen_evidence",
        ],
        "playable_ui": [
            "implementation_artifact",
            "interactive_ui_source_evidence",
            "non_static_screen_evidence",
        ],
        "stateful_interaction": [
            "implementation_artifact",
            "interactive_ui_source_evidence",
            "non_static_screen_evidence",
        ],
        "start_or_restart_flow": [
            "implementation_artifact",
            "interactive_ui_source_evidence",
            "non_static_screen_evidence",
        ],
        "player_control": [
            "implementation_artifact",
            "interactive_ui_source_evidence",
            "non_static_screen_evidence",
        ],
        "adversary_or_challenge": [
            "implementation_artifact",
            "interactive_ui_source_evidence",
            "non_static_screen_evidence",
        ],
        "progression_or_score": [
            "implementation_artifact",
            "interactive_ui_source_evidence",
            "non_static_screen_evidence",
        ],
        "failure_or_collision_rule": [
            "implementation_artifact",
            "interactive_ui_source_evidence",
            "non_static_screen_evidence",
        ],
        "user_input_or_action": [
            "implementation_artifact",
            "interactive_ui_source_evidence",
            "non_static_screen_evidence",
        ],
        "visible_state_change": [
            "implementation_artifact",
            "interactive_ui_source_evidence",
            "non_static_screen_evidence",
        ],
    }
    return mapping.get(str(capability).strip(), [])


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


def deferred_verify_requirement(command: str, has_dependency_setup: bool) -> bool:
    lowered = command.lower().strip()
    if not has_dependency_setup:
        return False
    if is_dependency_command(command):
        return False
    if "&&" in lowered or "||" in lowered or "|" in lowered or ";" in lowered:
        return False
    return lowered in {
        "npm run build",
        "npm test",
        "npm run test",
        "pnpm build",
        "pnpm test",
        "yarn build",
        "yarn test",
    }


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
    valid_plan_generated = (
        bool(plans)
        if spec["mode"] in {"step-plan", "plan-run", "ultra-plan-run", "ultra-step-run"}
        else ""
    )
    plan_score = None
    executable_plan_score = None
    constraint_coverage_score = None
    verify_strength_score = None
    plan_capability_contract_score = None
    prompt_plan_capability_coverage_score = None
    prompt_plan_missing_capability_count = None
    plan_required_capability_count = None
    plan_verify_declared_coverage_score = None
    executed_verify_coverage_score = None
    plan_verify_coverage_score = None
    plan_verified_capability_count = None
    plan_unverified_capability_count = None
    prompt_plan_gap_kind = ""
    plan_verify_gap_kind = ""
    plan_capability_oracle_version = ""
    plan_verify_oracle_version = ""
    verify_adequacy_cap_reason = ""
    verify_adequacy_score = None
    semantic_verify_coverage_score = None
    behavior_oracle_declared_score = None
    contentless_verify_penalty = None
    artifact_ownership_score = None
    execution_shape_readiness_score = None
    ultra_score = None
    plan_run_readiness = empty_plan_readiness_scores()
    phase_readiness_scores: list[dict] = []
    for plan in plans:
        score = score_plan_file(plan, spec["scenario"])
        events.append({"event": "plan_score", "run_id": spec["run_id"], "plan": plan.name, **score})
        if score["kind"] == "ultra":
            ultra_score = float(score["score"])
        else:
            readiness = score_plan_readiness_file(
                plan,
                profile=str(spec["scenario"].get("profile", "")),
                prompt=str(spec["scenario"].get("prompt", "")),
                handoff_events=child_events,
            )
            phase_readiness_scores.append(readiness)
            events.append(
                {
                    "event": "plan_run_readiness_evaluated",
                    "run_id": spec["run_id"],
                    "plan": plan.name,
                    **{
                        key: value
                        for key, value in readiness.items()
                        if key in READINESS_FIELDS or key == "details"
                    },
                }
            )
            plan_run_readiness.update(
                {key: readiness.get(key, "") for key in READINESS_FIELDS if key in readiness}
            )
            plan_score = float(score["score"])
            if score.get("executable_score") is not None:
                executable_plan_score = float(score["executable_score"])
            if score.get("constraint_coverage_score") is not None:
                constraint_coverage_score = float(score["constraint_coverage_score"])
            if score.get("verify_strength_score") is not None:
                verify_strength_score = float(score["verify_strength_score"])
            if score.get("plan_capability_contract_score") not in {"", None}:
                plan_capability_contract_score = float(score["plan_capability_contract_score"])
            if score.get("prompt_plan_capability_coverage_score") not in {"", None}:
                prompt_plan_capability_coverage_score = float(score["prompt_plan_capability_coverage_score"])
            if score.get("prompt_plan_missing_capability_count") not in {"", None}:
                prompt_plan_missing_capability_count = score["prompt_plan_missing_capability_count"]
            if score.get("plan_required_capability_count") not in {"", None}:
                plan_required_capability_count = score["plan_required_capability_count"]
            if score.get("plan_verify_declared_coverage_score") not in {"", None}:
                plan_verify_declared_coverage_score = float(score["plan_verify_declared_coverage_score"])
            if score.get("executed_verify_coverage_score") not in {"", None}:
                executed_verify_coverage_score = float(score["executed_verify_coverage_score"])
            if score.get("plan_verify_coverage_score") not in {"", None}:
                plan_verify_coverage_score = float(score["plan_verify_coverage_score"])
            if score.get("plan_verified_capability_count") not in {"", None}:
                plan_verified_capability_count = score["plan_verified_capability_count"]
            if score.get("plan_unverified_capability_count") not in {"", None}:
                plan_unverified_capability_count = score["plan_unverified_capability_count"]
            prompt_plan_gap_kind = str(score.get("prompt_plan_gap_kind", prompt_plan_gap_kind) or prompt_plan_gap_kind)
            plan_verify_gap_kind = str(score.get("plan_verify_gap_kind", plan_verify_gap_kind) or plan_verify_gap_kind)
            plan_capability_oracle_version = str(score.get("plan_capability_oracle_version", plan_capability_oracle_version) or plan_capability_oracle_version)
            plan_verify_oracle_version = str(score.get("plan_verify_oracle_version", plan_verify_oracle_version) or plan_verify_oracle_version)
            if score.get("verify_adequacy_score") is not None:
                verify_adequacy_score = float(score["verify_adequacy_score"])
            if score.get("verify_adequacy_details", {}).get("cap_reason"):
                verify_adequacy_cap_reason = str(score["verify_adequacy_details"]["cap_reason"])
            if score.get("semantic_verify_coverage_score") is not None:
                semantic_verify_coverage_score = float(score["semantic_verify_coverage_score"])
            if score.get("behavior_oracle_declared_score") is not None:
                behavior_oracle_declared_score = float(score["behavior_oracle_declared_score"])
            if score.get("contentless_verify_penalty") is not None:
                contentless_verify_penalty = float(score["contentless_verify_penalty"])
            if score.get("artifact_ownership_score") is not None:
                artifact_ownership_score = float(score["artifact_ownership_score"])
            if score.get("execution_shape_readiness_score") is not None:
                execution_shape_readiness_score = float(score["execution_shape_readiness_score"])
    ultra_phase_readiness = aggregate_ultra_phase_readiness(phase_readiness_scores)
    if spec["mode"] == "ultra-plan-run" and ultra_phase_readiness.get("ultra_phase_readiness_min_score") not in {"", None}:
        plan_run_readiness.update(ultra_phase_readiness)
        plan_run_readiness["plan_run_readiness_score"] = ultra_phase_readiness[
            "ultra_phase_readiness_min_score"
        ]
        plan_run_readiness["readiness_cap_reason"] = ultra_phase_readiness[
            "ultra_phase_readiness_cap_reason"
        ]

    post = {"ok": True, "postcheck_elapsed_sec": 0.0, "dependency_elapsed_sec": 0.0}
    if result.rc == 0 and spec["mode"] != "step-plan":
        post = run_postcheck(spec["scenario"], workdir, run_dir / "postcheck", timeout_sec=scenario_timeout)
        events.append({"event": "postcheck_summary", "run_id": spec["run_id"], **post})
    success = result.rc == 0 and bool(post["ok"])
    post_events = read_jsonl(run_dir / "postcheck" / "events.jsonl")
    runtime_capability_contract = score_plan_capability_contract(
        scenario=spec["scenario"],
        plan_paths=plans,
    )
    runtime_verify_coverage = score_plan_verify_coverage(
        scenario=spec["scenario"],
        mode=spec["mode"],
        plan_paths=plans,
        workdir=workdir,
        postcheck_events=post_events,
        plan_capability_result=runtime_capability_contract,
    )
    events.append({"event": "plan_capability_contract_evaluated", "run_id": spec["run_id"], **runtime_capability_contract})
    events.append({"event": "plan_verify_coverage_evaluated", "run_id": spec["run_id"], **runtime_verify_coverage})
    if runtime_capability_contract.get("plan_capability_contract_score") not in {"", None}:
        plan_capability_contract_score = float(runtime_capability_contract["plan_capability_contract_score"])
    if runtime_capability_contract.get("prompt_plan_capability_coverage_score") not in {"", None}:
        prompt_plan_capability_coverage_score = float(runtime_capability_contract["prompt_plan_capability_coverage_score"])
    if runtime_capability_contract.get("prompt_plan_missing_capability_count") not in {"", None}:
        prompt_plan_missing_capability_count = runtime_capability_contract["prompt_plan_missing_capability_count"]
    if runtime_capability_contract.get("plan_required_capability_count") not in {"", None}:
        plan_required_capability_count = runtime_capability_contract["plan_required_capability_count"]
    prompt_plan_gap_kind = str(runtime_capability_contract.get("prompt_plan_gap_kind", prompt_plan_gap_kind) or prompt_plan_gap_kind)
    plan_capability_oracle_version = str(runtime_capability_contract.get("plan_capability_oracle_version", plan_capability_oracle_version) or plan_capability_oracle_version)
    if runtime_verify_coverage.get("plan_verify_declared_coverage_score") not in {"", None}:
        plan_verify_declared_coverage_score = float(runtime_verify_coverage["plan_verify_declared_coverage_score"])
    if runtime_verify_coverage.get("executed_verify_coverage_score") not in {"", None}:
        executed_verify_coverage_score = float(runtime_verify_coverage["executed_verify_coverage_score"])
    if runtime_verify_coverage.get("plan_verify_coverage_score") not in {"", None}:
        plan_verify_coverage_score = float(runtime_verify_coverage["plan_verify_coverage_score"])
    if runtime_verify_coverage.get("plan_verified_capability_count") not in {"", None}:
        plan_verified_capability_count = runtime_verify_coverage["plan_verified_capability_count"]
    if runtime_verify_coverage.get("plan_unverified_capability_count") not in {"", None}:
        plan_unverified_capability_count = runtime_verify_coverage["plan_unverified_capability_count"]
    plan_verify_gap_kind = str(runtime_verify_coverage.get("plan_verify_gap_kind", plan_verify_gap_kind) or plan_verify_gap_kind)
    plan_verify_oracle_version = str(runtime_verify_coverage.get("plan_verify_oracle_version", plan_verify_oracle_version) or plan_verify_oracle_version)
    acceptance_start = time.monotonic()
    acceptance = evaluate_acceptance_outcome(
        scenario=spec["scenario"],
        workdir=workdir,
        run_dir=run_dir,
        mode=spec["mode"],
        process_success=result.rc == 0,
        legacy_success=success,
        postcheck=post,
        plan_paths=plans,
        plan_capability=runtime_capability_contract,
        plan_verify_coverage=runtime_verify_coverage,
    )
    acceptance_oracle_sec = round(time.monotonic() - acceptance_start, 3)
    events.append({"event": "acceptance_summary", "run_id": spec["run_id"], **acceptance})
    diagnostics = summarize_run_events(events, post)
    lint_repair_score = calculate_lint_repair_score(diagnostics)
    execution_score = 100.0 if success else 0.0
    time_score = 100.0
    extras = {
        "timeout": result.timeout,
        "metric_source": "events+process" if child_events else "process",
        "elapsed_total_sec": round(time.monotonic() - start, 3),
        "oracle_kind": post.get("oracle_kind", ""),
        "provider_probe": spec.get("provider_probe", {}),
    }
    failure_kind = ""
    if not success:
        stderr = (run_dir / "stderr.log").read_text(encoding="utf-8", errors="replace")
        failure = {
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
        failure_kind = str(failure.get("failure_kind", ""))
        if failure_kind:
            failure["failure_layer"] = failure_layer_for_kind(failure_kind)
            failure["agent_capability_failure"] = capability_failure_included(failure_kind)
        extras.update(failure)
    readiness_outcome = classify_readiness_outcome(
        plan_run_readiness.get("plan_run_readiness_score", ""),
        success=success,
        failure_kind=failure_kind,
    )
    plan_run_readiness.update(readiness_outcome)
    if readiness_outcome.get("plan_run_missed_predictive_signal"):
        extras.update(readiness_outcome)
        events.append(
            {
                "event": "plan_run_missed_predictive_signal",
                "run_id": spec["run_id"],
                "readiness_score": plan_run_readiness.get("plan_run_readiness_score", ""),
                **readiness_outcome,
            }
        )
    if diagnostics["repair_progress"]:
        extras["repair_progress"] = diagnostics["repair_progress"]
    if diagnostics["provider_transient_excluded_from_agent_capability"]:
        extras["provider_transient_excluded_from_agent_capability"] = diagnostics[
            "provider_transient_excluded_from_agent_capability"
        ]
    planner_observability = summarize_planner_observability(events)
    if planner_observability:
        extras.update(planner_observability)
    completion_observability = summarize_completion_observability(events)
    if completion_observability:
        extras.update(completion_observability)
    plan_run_predictive_score = ""
    if executable_plan_score is not None:
        plan_run_predictive_score = calculate_plan_run_predictive_score(
            executable_plan_score,
            artifact_ownership_score,
            verify_strength_score,
            constraint_coverage_score,
            lint_repair_score,
            execution_shape_readiness_score,
        )
    runtime_scores = score_runtime_health(
        events,
        mode=spec["mode"],
        success=success,
        scenario=spec["scenario"],
        workdir=workdir,
        plan_paths=plans,
        run_dir=run_dir,
    )
    ultra_runtime_health_score = (
        float(runtime_scores["ultra_runtime_health_score"])
        if runtime_scores.get("ultra_runtime_health_score") not in {"", None}
        else None
    )
    row.update(
        {
            "rc": result.rc,
            "success": success,
            "legacy_success": acceptance.get("legacy_success", success),
            "process_success": acceptance.get("process_success", ""),
            "artifact_success": acceptance.get("artifact_success", ""),
            "build_success": acceptance.get("build_success", ""),
            "launch_success": acceptance.get("launch_success", ""),
            "behavior_success": acceptance.get("behavior_success", ""),
            "source_semantic_success": acceptance.get("source_semantic_success", ""),
            "source_semantic_score": acceptance.get("source_semantic_score", ""),
            "plan_output_adherence_success": acceptance.get("plan_output_adherence_success", ""),
            "plan_output_adherence_score": acceptance.get("plan_output_adherence_score", ""),
            "plan_output_failure_kind": acceptance.get("plan_output_failure_kind", ""),
            "plan_capability_contract_score": plan_capability_contract_score
            if plan_capability_contract_score is not None
            else "",
            "plan_capability_oracle_version": plan_capability_oracle_version,
            "prompt_plan_capability_coverage_score": prompt_plan_capability_coverage_score
            if prompt_plan_capability_coverage_score is not None
            else "",
            "prompt_plan_missing_capability_count": prompt_plan_missing_capability_count
            if prompt_plan_missing_capability_count is not None
            else "",
            "plan_required_capability_count": plan_required_capability_count
            if plan_required_capability_count is not None
            else "",
            "plan_verify_declared_coverage_score": plan_verify_declared_coverage_score
            if plan_verify_declared_coverage_score is not None
            else "",
            "executed_verify_coverage_score": executed_verify_coverage_score
            if executed_verify_coverage_score is not None
            else "",
            "plan_verify_coverage_score": plan_verify_coverage_score
            if plan_verify_coverage_score is not None
            else "",
            "plan_verified_capability_count": plan_verified_capability_count
            if plan_verified_capability_count is not None
            else "",
            "plan_unverified_capability_count": plan_unverified_capability_count
            if plan_unverified_capability_count is not None
            else "",
            "prompt_plan_gap_kind": prompt_plan_gap_kind,
            "plan_verify_gap_kind": plan_verify_gap_kind,
            "plan_verify_oracle_version": plan_verify_oracle_version,
            "verify_adequacy_cap_reason": verify_adequacy_cap_reason,
            "acceptance_confidence_score": acceptance.get("acceptance_confidence_score", ""),
            "acceptance_confidence_reason": acceptance.get("acceptance_confidence_reason", ""),
            "prompt_contract_success": acceptance.get("prompt_contract_success", ""),
            "capability_acceptance_success": acceptance.get("capability_acceptance_success", ""),
            "acceptance_success": acceptance.get("acceptance_success", ""),
            "acceptance_failure_kind": acceptance.get("acceptance_failure_kind", ""),
            "acceptance_failure_reasons": acceptance.get("acceptance_failure_reasons", ""),
            "acceptance_false_positive": acceptance.get("acceptance_false_positive", ""),
            "oracle_gap_kind": acceptance.get("oracle_gap_kind", ""),
            "acceptance_oracle_version": acceptance.get("acceptance_oracle_version", ""),
            "queue_wait_sec": 0.0,
            "provider_wait_sec": 0.0,
            "port_mutex_wait_sec": 0.0,
            "scheduler_lane": spec.get("_scheduler_lane", ""),
            "serial_reason": "",
            "effective_parallelism": spec.get("_effective_parallelism", ""),
            "provider_limit": spec.get("_provider_limit", ""),
            "parallel_limit": spec.get("_parallel_limit", ""),
            "wall_clock_sec": "",
            "acceptance_oracle_sec": acceptance_oracle_sec,
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
            "required_capability_count": completion_observability.get("required_capability_count", ""),
            "missing_capability_count": completion_observability.get("missing_capability_count", ""),
            "required_evidence_count": completion_observability.get("required_evidence_count", ""),
            "missing_evidence_count": completion_observability.get("missing_evidence_count", ""),
            "weak_evidence_count": completion_observability.get("weak_evidence_count", ""),
            "runtime_acceptance_primary_reason": completion_observability.get(
                "runtime_acceptance_primary_reason", ""
            ),
            "last_provider_error_kind": diagnostics["last_provider_error_kind"],
            "last_provider_http_status": diagnostics["last_provider_http_status"],
            "provider_attempts": diagnostics["provider_attempts"],
            "fallback_decision": diagnostics["fallback_decision"],
            "planner_stage": diagnostics["planner_stage"],
            "planner_error_kind": diagnostics["planner_error_kind"],
            "planner_error_count": diagnostics["planner_error_count"],
            "planner_repair_attempts": diagnostics["planner_repair_attempts"],
            "planner_schema_repaired": diagnostics["planner_schema_repaired"],
            "planner_raw_schema_violation": diagnostics["planner_raw_schema_violation"],
            "planner_parser_limitation": diagnostics["planner_parser_limitation"],
            "planner_prompt_issue": diagnostics["planner_prompt_issue"],
            "planner_quality_issue_count": diagnostics["planner_quality_issue_count"],
            "planner_retryable_quality_count": diagnostics["planner_retryable_quality_count"],
            "planner_advisory_quality_count": diagnostics["planner_advisory_quality_count"],
            "planner_quality_retry_count": diagnostics["planner_quality_retry_count"],
            "planner_quality_retry_degraded_count": diagnostics[
                "planner_quality_retry_degraded_count"
            ],
            "valid_plan_generated": valid_plan_generated,
            "failure_layer": str(extras.get("failure_layer", "")),
            "capability_failure_included": str(extras.get("agent_capability_failure", "")).lower()
            if extras.get("agent_capability_failure") not in {"", None}
            else "",
            "plan_quality_score": plan_score if plan_score is not None else "",
            "executable_plan_score": executable_plan_score if executable_plan_score is not None else "",
            "constraint_coverage_score": constraint_coverage_score if constraint_coverage_score is not None else "",
            "verify_strength_score": verify_strength_score if verify_strength_score is not None else "",
            "verify_adequacy_score": verify_adequacy_score if verify_adequacy_score is not None else "",
            "semantic_verify_coverage_score": semantic_verify_coverage_score
            if semantic_verify_coverage_score is not None
            else "",
            "behavior_oracle_declared_score": behavior_oracle_declared_score
            if behavior_oracle_declared_score is not None
            else "",
            "contentless_verify_penalty": contentless_verify_penalty
            if contentless_verify_penalty is not None
            else "",
            "artifact_ownership_score": artifact_ownership_score if artifact_ownership_score is not None else "",
            "lint_repair_score": lint_repair_score,
            "execution_shape_readiness_score": execution_shape_readiness_score
            if execution_shape_readiness_score is not None
            else "",
            "plan_run_predictive_score": plan_run_predictive_score,
            **{key: plan_run_readiness.get(key, "") for key in READINESS_FIELDS},
            **runtime_scores,
            "ultra_phase_quality_score": ultra_score if ultra_score is not None else "",
            "execution_score": execution_score,
            "time_score": time_score,
            "overall_score": calculate_overall(
                spec["mode"],
                plan_score,
                ultra_score,
                execution_score,
                time_score,
                executable_plan_score,
                constraint_coverage_score,
                verify_strength_score,
                artifact_ownership_score,
                lint_repair_score,
                ultra_runtime_health_score,
            ),
            "plan_artifacts": ",".join(str(path.relative_to(run_dir)) for path in plans),
            "extras_json": extras,
        }
    )
    if acceptance.get("acceptance_details"):
        row["extras_json"]["acceptance_details"] = acceptance["acceptance_details"]
    return row, events


def summarize_planner_observability(events: list[dict]) -> dict[str, object]:
    raw_shapes = [
        {
            "planner_provider": event.get("planner_provider", ""),
            "planner_model": event.get("planner_model", ""),
            "attempt": event.get("attempt", ""),
            "content_len": event.get("content_len", ""),
            "has_json_object": event.get("has_json_object", ""),
            "contains_goal_key": event.get("contains_goal_key", ""),
            "contains_steps_key": event.get("contains_steps_key", ""),
            "json_extract_status": event.get("json_extract_status", ""),
        }
        for event in events
        if event.get("event") == "planner_raw_output_shape"
    ]
    quality_warnings = [
        {
            "planner_provider": event.get("planner_provider", ""),
            "planner_model": event.get("planner_model", ""),
            "message": event.get("planner_error_message", ""),
        }
        for event in events
        if event.get("event") == "planner_quality_warning"
    ]
    quality_issues = [
        {
            "planner_provider": event.get("planner_provider", ""),
            "planner_model": event.get("planner_model", ""),
            "category": event.get("planner_quality_category", ""),
            "severity": event.get("planner_quality_severity", ""),
            "step_id": event.get("planner_quality_step_id", ""),
            "message": event.get("planner_error_message", ""),
        }
        for event in events
        if event.get("event") == "planner_quality_issue"
    ]
    quality_retries = [
        event for event in events if event.get("event") == "planner_quality_retry"
    ]
    quality_degraded = [
        event for event in events if event.get("event") == "planner_quality_retry_degraded"
    ]
    ultra_generation_attempts = [
        event for event in events if event.get("event") == "ultra_plan_generation_attempt"
    ]
    ultra_generation_retries = [
        event for event in events if event.get("event") == "ultra_plan_generation_retry"
    ]
    ultra_generation_failures = [
        event for event in events if event.get("event") == "ultra_plan_generation_failed"
    ]
    ultra_generation_tool_rejections = [
        event
        for event in events
        if event.get("event") == "ultra_plan_generation_tool_call_rejected"
    ]
    ultra_generation_metadata_normalized = [
        event
        for event in events
        if event.get("event") == "ultra_plan_generation_metadata_normalized"
    ]
    out: dict[str, object] = {}
    if raw_shapes:
        out["planner_raw_output_shape_count"] = len(raw_shapes)
        out["planner_raw_output_shapes"] = raw_shapes
    if quality_warnings:
        out["planner_quality_warning_count"] = len(quality_warnings)
        out["planner_quality_warnings"] = quality_warnings
    if quality_issues:
        out["planner_quality_issue_count"] = len(quality_issues)
        out["planner_quality_issues"] = quality_issues
    if quality_retries:
        out["planner_quality_retry_count"] = len(quality_retries)
    if quality_degraded:
        out["planner_quality_retry_degraded_count"] = len(quality_degraded)
    if ultra_generation_attempts:
        out["ultra_plan_generation_attempt_count"] = len(ultra_generation_attempts)
    if ultra_generation_retries:
        out["ultra_plan_generation_retry_count"] = len(ultra_generation_retries)
        out["ultra_plan_generation_retry_kinds"] = [
            str(event.get("planner_error_kind", "")) for event in ultra_generation_retries
        ]
    if ultra_generation_failures:
        out["ultra_plan_generation_failed"] = True
    if ultra_generation_tool_rejections:
        out["ultra_plan_generation_tool_call_rejected_count"] = len(
            ultra_generation_tool_rejections
        )
    if ultra_generation_metadata_normalized:
        out["ultra_plan_generation_metadata_normalized_count"] = len(
            ultra_generation_metadata_normalized
        )
    return out


def summarize_completion_observability(events: list[dict]) -> dict[str, object]:
    completion = [
        event for event in events if event.get("event") == "completion_verify"
    ]
    if not completion:
        return {}
    last = completion[-1]
    out: dict[str, object] = {
        "completion_verify_count": len(completion),
    }
    if last.get("profile"):
        out["completion_profile"] = last.get("profile", "")
    if last.get("deferred_verify_requirements"):
        out["deferred_verify_requirements"] = last.get("deferred_verify_requirements", [])
    if last.get("profile_failures"):
        out["profile_failures"] = last.get("profile_failures", [])
    required_capabilities = last.get("required_capabilities", []) or []
    missing_capabilities = last.get("missing_capabilities", []) or []
    required_evidence = last.get("required_evidence", []) or []
    missing_evidence = last.get("missing_evidence", []) or []
    weak_evidence = last.get("weak_evidence", []) or []
    out["required_capability_count"] = len(required_capabilities)
    out["missing_capability_count"] = len(missing_capabilities)
    out["required_evidence_count"] = len(required_evidence)
    out["missing_evidence_count"] = len(missing_evidence)
    out["weak_evidence_count"] = len(weak_evidence)
    out["runtime_acceptance_primary_reason"] = last.get("runtime_acceptance_primary_reason", "")
    out["runtime_acceptance_passed"] = last.get("runtime_acceptance_passed", "")
    if required_capabilities:
        out["required_capabilities"] = required_capabilities
    if missing_capabilities:
        out["missing_capabilities"] = missing_capabilities
    if required_evidence:
        out["required_evidence"] = required_evidence
    if missing_evidence:
        out["missing_evidence"] = missing_evidence
    if weak_evidence:
        out["weak_evidence"] = weak_evidence
    return out


def summarize_run_events(events: list[dict], post: dict) -> dict[str, str]:
    loop_stop = next((event for event in reversed(events) if event.get("event") == "loop_stop"), {})
    completion_verify = [
        event for event in events if event.get("event") == "completion_verify"
    ]
    repair_progress = next(
        (event for event in reversed(events) if event.get("event") == "verify_repair_progress"),
        {},
    )
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
    planner_errors = [
        event for event in events if event.get("event") == "planner_error"
    ]
    planner_schema_repairs = [
        event for event in events if event.get("event") == "planner_schema_repaired"
    ]
    quality_issues = [
        event for event in events if event.get("event") == "planner_quality_issue"
    ]
    quality_retries = [
        event for event in events if event.get("event") == "planner_quality_retry"
    ]
    quality_degraded = [
        event for event in events if event.get("event") == "planner_quality_retry_degraded"
    ]
    planner_error = planner_errors[-1] if planner_errors else {}
    tool_error = next(
        (
            event
            for event in reversed(events)
            if event.get("event") == "tool_execute" and event.get("status") == "error"
        ),
        {},
    )
    missing = post.get("missing_artifacts") or loop_stop.get("missing_paths") or []
    provider_transient = (
        provider_error.get("status") in {429, 500, 502, 503, 504}
        or provider_error.get("error_kind") in {"network", "timeout"}
    )
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
        "repair_progress": str(repair_progress.get("verdict", "")),
        "provider_transient_excluded_from_agent_capability": str(provider_transient).lower()
        if provider_transient
        else "",
        "planner_stage": str(planner_error.get("planner_stage", "")),
        "planner_error_kind": str(planner_error.get("planner_error_kind", "")),
        "planner_error_count": str(len(planner_errors)) if planner_errors else "",
        "planner_repair_attempts": str(planner_error.get("repair_attempt", "")),
        "planner_schema_repaired": str(bool(planner_schema_repairs)).lower()
        if planner_schema_repairs
        else "",
        "planner_raw_schema_violation": str(any(
            event.get("planner_stage") == "schema" for event in planner_errors
        )).lower()
        if planner_errors
        else "",
        "planner_parser_limitation": str(any(
            "parser" in str(event.get("planner_error_message", "")).lower()
            for event in planner_errors
        )).lower()
        if planner_errors
        else "",
        "planner_prompt_issue": str(any(
            event.get("planner_stage") == "lint" for event in planner_errors
        )).lower()
        if planner_errors
        else "",
        "planner_quality_issue_count": str(len(quality_issues)) if quality_issues else "",
        "planner_retryable_quality_count": str(
            sum(
                1
                for event in quality_issues
                if event.get("planner_quality_severity") == "retryable_quality"
            )
        )
        if quality_issues
        else "",
        "planner_advisory_quality_count": str(
            sum(
                1
                for event in quality_issues
                if event.get("planner_quality_severity") == "advisory"
            )
        )
        if quality_issues
        else "",
        "planner_quality_retry_count": str(len(quality_retries)) if quality_retries else "",
        "planner_quality_retry_degraded_count": str(len(quality_degraded))
        if quality_degraded
        else "",
    }


def calculate_lint_repair_score(diagnostics: dict[str, str]) -> float:
    score = 100.0
    score -= 12.0 * safe_int(diagnostics.get("planner_error_count"))
    score -= 8.0 * safe_int(diagnostics.get("planner_repair_attempts"))
    if diagnostics.get("planner_schema_repaired") == "true":
        score -= 10.0
    if diagnostics.get("planner_raw_schema_violation") == "true":
        score -= 15.0
    if diagnostics.get("planner_parser_limitation") == "true":
        score -= 10.0
    if diagnostics.get("planner_prompt_issue") == "true":
        score -= 10.0
    if diagnostics.get("planner_stage") == "lint":
        score -= 8.0
    return round(max(0.0, min(100.0, score)), 1)


def safe_int(value: object) -> int:
    try:
        return int(str(value))
    except (TypeError, ValueError):
        return 0


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
            executable = float(row["executable_plan_score"]) if row.get("executable_plan_score") not in {"", None} else None
            constraint = float(row["constraint_coverage_score"]) if row.get("constraint_coverage_score") not in {"", None} else None
            verify_strength = float(row["verify_strength_score"]) if row.get("verify_strength_score") not in {"", None} else None
            artifact_ownership = float(row["artifact_ownership_score"]) if row.get("artifact_ownership_score") not in {"", None} else None
            lint_repair = float(row["lint_repair_score"]) if row.get("lint_repair_score") not in {"", None} else None
            execution_shape = (
                float(row["execution_shape_readiness_score"])
                if row.get("execution_shape_readiness_score") not in {"", None}
                else None
            )
            ultra = float(row["ultra_phase_quality_score"]) if row.get("ultra_phase_quality_score") not in {"", None} else None
            ultra_runtime = (
                float(row["ultra_runtime_health_score"])
                if row.get("ultra_runtime_health_score") not in {"", None}
                else None
            )
            execution = float(row["execution_score"]) if row.get("execution_score") not in {"", None} else 0.0
            if executable is not None:
                row["plan_run_predictive_score"] = calculate_plan_run_predictive_score(
                    executable,
                    artifact_ownership,
                    verify_strength,
                    constraint,
                    lint_repair,
                    execution_shape,
                )
            row["overall_score"] = calculate_overall(
                row["mode"],
                plan,
                ultra,
                execution,
                float(row["time_score"]),
                executable,
                constraint,
                verify_strength,
                artifact_ownership,
                lint_repair,
                ultra_runtime,
            )


def apply_stability_scores(rows: list[dict]) -> None:
    groups: dict[tuple, list[dict]] = {}
    for row in rows:
        key = (
            row.get("scenario", ""),
            row.get("mode", ""),
            row.get("main_provider", ""),
            row.get("main_model", ""),
            row.get("planner_provider", ""),
            row.get("planner_model", ""),
            row.get("local_llm_used", ""),
        )
        groups.setdefault(key, []).append(row)
    for group in groups.values():
        if len(group) < 2:
            for row in group:
                row["stability_score"] = ""
            continue
        overall = [float(row["overall_score"]) for row in group if row.get("overall_score") not in {"", None}]
        elapsed = [float(row["exec_elapsed_sec"]) for row in group if row.get("exec_elapsed_sec") not in {"", None}]
        success_values = {str(row.get("success", "")).lower() for row in group}
        overall_stdev = statistics.pstdev(overall) if len(overall) > 1 else 0.0
        elapsed_cv = 0.0
        if len(elapsed) > 1 and statistics.fmean(elapsed) > 0:
            elapsed_cv = 100.0 * statistics.pstdev(elapsed) / statistics.fmean(elapsed)
        mixed_success_penalty = 20.0 if len(success_values) > 1 else 0.0
        score = 100.0 - min(60.0, overall_stdev * 1.5) - min(30.0, elapsed_cv * 0.3) - mixed_success_penalty
        for row in group:
            row["stability_score"] = round(max(0.0, min(100.0, score)), 1)


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
