from __future__ import annotations

import shlex
from pathlib import Path
from typing import Any

from .artifacts import run_id
from .models import ModelRef, cli_model_args
from .suites import prompt_with_required_final_artifacts

VALID_MODES = {"minimal-loop", "step-plan", "plan-run", "ultra-plan-run", "ultra-step-run"}


def parse_modes(value: str) -> list[str]:
    modes = [item.strip() for item in value.split(",") if item.strip()]
    unknown = [item for item in modes if item not in VALID_MODES]
    if unknown:
        raise ValueError(f"unknown eval modes: {', '.join(unknown)}")
    return modes


def expand_matrix(
    suite: dict[str, Any],
    profile: dict[str, Any],
    modes: list[str],
    runs: int,
    context_budget: int,
    binary: str = "anvilminimal",
    scenario_filter: str | None = None,
) -> list[dict[str, Any]]:
    rows: list[dict[str, Any]] = []
    scenarios = suite["scenarios"]
    if scenario_filter:
        scenarios = [s for s in scenarios if s["id"] == scenario_filter]
        if not scenarios:
            raise ValueError(f"scenario not found: {scenario_filter}")
    for scenario in scenarios:
        for mode in modes:
            for model_pair in profile["runs"]:
                main: ModelRef = model_pair["main"]
                planner: ModelRef = model_pair["planner"]
                for run_index in range(1, runs + 1):
                    rid = run_id(
                        suite["name"],
                        scenario["id"],
                        mode,
                        main.provider,
                        main.model,
                        planner.provider,
                        planner.model,
                        run_index,
                    )
                    row = {
                        "run_id": rid,
                        "suite": suite["name"],
                        "scenario": scenario,
                        "mode": mode,
                        "main": {"provider": main.provider, "model": main.model},
                        "planner": {"provider": planner.provider, "model": planner.model},
                        "serial_lane": bool(model_pair["serial_lane"]),
                        "local_llm_used": bool(model_pair["local_llm_used"]),
                        "port_mutex": scenario_port_mutex(scenario),
                        "provider_limits": profile.get("provider_limit", 2),
                        "chat_retries": int(profile.get("chat_retries", 1)),
                    }
                    row["command"] = render_command(
                        binary=binary,
                        mode=mode,
                        scenario=scenario,
                        main=main,
                        planner=planner,
                        context_budget=context_budget,
                        workdir=Path("workdir"),
                        chat_retries=row["chat_retries"],
                    )
                    row["command_text"] = shlex.join(row["command"])
                    rows.append(row)
    return rows


def scenario_port_mutex(scenario: dict[str, Any]) -> int | None:
    dev = scenario.get("postcheck", {}).get("dev_server")
    if dev and dev.get("port"):
        return int(dev["port"])
    if "3011" in str(scenario.get("prompt", "")):
        return 3011
    return None


def render_command(
    binary: str,
    mode: str,
    scenario: dict[str, Any],
    main: ModelRef,
    planner: ModelRef,
    context_budget: int,
    workdir: Path,
    chat_retries: int = 1,
) -> list[str]:
    base = [
        binary,
        "--yes",
        "--context-budget",
        str(context_budget),
        "--chat-retries",
        str(chat_retries),
        *cli_model_args(main, planner),
        "--cwd",
        str(workdir),
    ]
    prompt = prompt_with_required_final_artifacts(scenario)
    if mode == "minimal-loop":
        return [*base, "--prompt", prompt]
    if mode == "step-plan":
        return [*base, "--plan-steps", prompt]
    if mode == "plan-run":
        return [*base, "--plan-run", prompt]
    if mode == "ultra-plan-run":
        return [*base, "--ultra-plan-run", "--profile", scenario.get("profile", "generic"), prompt]
    if mode == "ultra-step-run":
        return [*base, "--run-plan", "<phase-step-plan.yaml>"]
    raise ValueError(f"unknown mode: {mode}")
