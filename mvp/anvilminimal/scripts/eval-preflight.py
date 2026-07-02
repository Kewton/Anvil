#!/usr/bin/env python3
from __future__ import annotations

import argparse
import json
import sys
from pathlib import Path

from eval_lib.artifacts import create_run_root, write_json, write_jsonl
from eval_lib.config import credential_status, env_value, load_dotenv
from eval_lib.matrix import expand_matrix, parse_modes
from eval_lib.models import (
    gemini_interactions_smoke,
    load_model_profiles,
    models_for_provider,
    ollama_models,
    openai_responses_smoke,
    required_providers_for_profile,
)
from eval_lib.parity_gate import build_parity_gate_report, validate_parity_gate_report
from eval_lib.postcheck import port_available
from eval_lib.process import command_available
from eval_lib.suites import load_suite


def main() -> int:
    parser = argparse.ArgumentParser(description="Preflight checks for anvilminimal eval.")
    parser.add_argument("--suite", required=True)
    parser.add_argument("--model-profile", required=True)
    parser.add_argument("--model-profiles", default="eval/model_profiles.yaml")
    parser.add_argument("--modes", default="minimal-loop,step-plan,plan-run,ultra-plan-run")
    parser.add_argument("--runs", type=int, default=1)
    parser.add_argument("--parallel", type=int, default=4)
    parser.add_argument("--context-budget", type=int, default=65536)
    parser.add_argument("--binary", default="anvilminimal")
    parser.add_argument("--run-root")
    parser.add_argument("--offline-ok", action="store_true")
    parser.add_argument("--ollama-host", default="http://localhost:11434")
    parser.add_argument(
        "--gate-level",
        choices=["local", "network", "comparative", "release"],
        default="local",
        help="Requested parity gate level. Comparative/release checks are opt-in.",
    )
    parser.add_argument(
        "--parity-gate-report",
        help="Existing parity_gate_report.json to validate or update.",
    )
    parser.add_argument(
        "--write-parity-gate-report",
        help="Output path for generated parity_gate_report.json. Defaults to run-root when gate inputs are supplied.",
    )
    parser.add_argument("--mvp-summary", help="MVP summary.eval.tsv for parity comparison.")
    parser.add_argument("--anvildev-summary", help="anvildev --engine minimal summary.eval.tsv for parity comparison.")
    parser.add_argument("--source-trace-report", help="source anvildev runtime-semantics-trace-report.json.")
    parser.add_argument("--mvp-trace-report", help="MVP runtime-semantics-trace-report.json.")
    parser.add_argument("--trace-diff", help="Precomputed runtime-semantics-trace-diff.json.")
    parser.add_argument(
        "--intentional-difference-evidence",
        action="append",
        default=[],
        help="Evidence path explaining an intentional MVP/source difference.",
    )
    parser.add_argument("--uat-evidence", action="append", default=[])
    parser.add_argument("--browser-evidence", action="append", default=[])
    parser.add_argument("--interaction-evidence", action="append", default=[])
    parser.add_argument("--tui-events", action="append", default=[])
    parser.add_argument("--warn-delta-pp", type=float, default=5.0)
    parser.add_argument("--fail-delta-pp", type=float, default=10.0)
    parser.add_argument(
        "--live-provider-smoke",
        nargs="?",
        const="all",
        default="",
        help="Run live no-tool and tool-declaration provider smoke checks for all/openai/gemini.",
    )
    args = parser.parse_args()

    run_root = Path(args.run_root) if args.run_root else create_run_root()
    suite = load_suite(args.suite)
    profiles, warnings = load_model_profiles(args.model_profiles)
    if args.model_profile not in profiles:
        raise SystemExit(f"unknown model profile: {args.model_profile}")
    profile = profiles[args.model_profile]
    matrix = expand_matrix(suite, profile, parse_modes(args.modes), args.runs, args.context_budget, args.binary)

    dotenv = load_dotenv()
    deps = dependency_status(suite, args.binary)
    credentials = {provider: credential_status(provider, dotenv) for provider in sorted(required_providers_for_profile(profile))}
    ports = {}
    for row in matrix:
        if row.get("port_mutex"):
            ports[str(row["port_mutex"])] = "available" if port_available(int(row["port_mutex"])) else "unavailable"
    ollama = {}
    if "ollama" in credentials:
        try:
            available = ollama_models(args.ollama_host)
            needed = {
                run["main"].model
                for run in profile["runs"]
                if run["main"].provider == "ollama"
            } | {
                run["planner"].model
                for run in profile["runs"]
                if run["planner"].provider == "ollama"
            }
            ollama = {model: ("present" if model in available else "missing") for model in sorted(needed)}
        except Exception as err:
            ollama = {"error": str(err)}
    live_provider_smoke = {}
    if args.live_provider_smoke:
        requested = (
            required_providers_for_profile(profile)
            if args.live_provider_smoke == "all"
            else {part.strip() for part in args.live_provider_smoke.split(",") if part.strip()}
        )
        if "openai" in requested:
            key = env_value("OPENAI_API_KEY", dotenv)
            live_provider_smoke["openai"] = smoke_openai_models(profile, key)
        if "gemini" in requested:
            key = env_value("GEMINI_API_KEY", dotenv)
            live_provider_smoke["gemini"] = smoke_gemini_models(profile, key)

    ok = True
    if any(status == "missing" for status in deps.values()):
        ok = False
    if not args.offline_ok and any(status == "missing" for status in credentials.values()):
        ok = False
    if any(status == "unavailable" for status in ports.values()):
        ok = False
    if "error" in ollama and not args.offline_ok:
        ok = False
    if any(status == "missing" for status in ollama.values()) and not args.offline_ok:
        ok = False
    if live_provider_smoke and not args.offline_ok:
        for provider in live_provider_smoke.values():
            for smoke in provider.values():
                if not smoke.get("ok"):
                    ok = False
    parity_gate = parity_gate_status(args, run_root)
    if parity_gate.get("requested") and not parity_gate.get("ok"):
        ok = False

    payload = {
        "ok": ok,
        "suite": suite["name"],
        "model_profile": args.model_profile,
        "runs": len(matrix),
        "parallel": args.parallel,
        "dependencies": deps,
        "credentials": credentials,
        "ports": ports,
        "ollama": ollama,
        "live_provider_smoke": live_provider_smoke,
        "parity_gate": parity_gate,
    }
    write_json(run_root / "preflight.json", payload)
    write_jsonl(run_root / "warnings.jsonl", warnings)
    print(json.dumps({"run_root": str(run_root), **payload}, ensure_ascii=False, indent=2))
    if parity_gate.get("requested") and not parity_gate.get("ok"):
        return 5
    if ok or args.offline_ok:
        return 0
    if any(status == "missing" for status in deps.values()):
        return 2
    if any(status == "missing" for status in credentials.values()) or ollama:
        return 3
    return 4


def parity_gate_status(args: argparse.Namespace, run_root: Path) -> dict:
    requested = any(
        [
            args.parity_gate_report,
            args.write_parity_gate_report,
            args.mvp_summary,
            args.anvildev_summary,
            args.source_trace_report,
            args.mvp_trace_report,
            args.trace_diff,
            args.gate_level != "local",
            args.uat_evidence,
            args.browser_evidence,
            args.interaction_evidence,
            args.tui_events,
        ]
    )
    if not requested:
        return {"requested": False}
    base_report = {}
    if args.parity_gate_report:
        base_report = json.loads(Path(args.parity_gate_report).read_text(encoding="utf-8"))
    report = build_parity_gate_report(
        base_report=base_report,
        gate_level=args.gate_level,
        mvp_summary_path=args.mvp_summary,
        anvildev_summary_path=args.anvildev_summary,
        source_trace_report_path=args.source_trace_report,
        mvp_trace_report_path=args.mvp_trace_report,
        trace_diff_path=args.trace_diff,
        uat_evidence_paths=args.uat_evidence,
        browser_evidence_paths=args.browser_evidence,
        interaction_evidence_paths=args.interaction_evidence,
        tui_event_paths=args.tui_events,
        intentional_difference_evidence_paths=args.intentional_difference_evidence,
        warn_delta_pp=args.warn_delta_pp,
        fail_delta_pp=args.fail_delta_pp,
    )
    output_path = (
        Path(args.write_parity_gate_report)
        if args.write_parity_gate_report
        else run_root / "parity_gate_report.json"
    )
    write_json(output_path, report)
    schema_errors = validate_parity_gate_report(report)
    report_errors = list(report.get("errors", []) or [])
    return {
        "requested": True,
        "ok": not schema_errors and not report_errors,
        "gate_level": args.gate_level,
        "report_path": str(output_path),
        "schema_errors": schema_errors,
        "report_errors": report_errors,
        "warnings": list(report.get("warnings", []) or []),
        "anvildev_comparison": report.get("anvildev_comparison", {}),
        "uat_equivalent": report.get("uat_equivalent", {}),
    }


def smoke_openai_models(profile: dict, api_key: str | None) -> dict[str, dict]:
    if not api_key:
        return {model: {"ok": False, "error_kind": "missing_credential"} for model in sorted(models_for_provider(profile, "openai"))}
    results = {}
    for model in sorted(models_for_provider(profile, "openai")):
        no_tool = openai_responses_smoke(model, api_key, tools=False)
        tool = openai_responses_smoke(model, api_key, tools=True) if no_tool.get("ok") else {"ok": False, "skipped": "no_tool_failed"}
        results[model] = {"ok": bool(no_tool.get("ok") and tool.get("ok")), "no_tool": no_tool, "tool_declaration": tool}
    return results


def smoke_gemini_models(profile: dict, api_key: str | None) -> dict[str, dict]:
    if not api_key:
        return {model: {"ok": False, "error_kind": "missing_credential"} for model in sorted(models_for_provider(profile, "gemini"))}
    results = {}
    for model in sorted(models_for_provider(profile, "gemini")):
        no_tool = gemini_interactions_smoke(model, api_key, tools=False)
        tool = gemini_interactions_smoke(model, api_key, tools=True) if no_tool.get("ok") else {"ok": False, "skipped": "no_tool_failed"}
        results[model] = {"ok": bool(no_tool.get("ok") and tool.get("ok")), "no_tool": no_tool, "tool_declaration": tool}
    return results


def dependency_status(suite: dict, binary: str) -> dict[str, str]:
    required = {"python3", binary}
    for scenario in suite["scenarios"]:
        for command in scenario.get("postcheck", {}).get("commands", []) or []:
            first = command.split()[0]
            if first in {"npm", "node", "cargo", "python3", "pytest"}:
                required.add(first)
        dev = scenario.get("postcheck", {}).get("dev_server")
        if dev:
            required.add(dev["command"].split()[0])
    status = {}
    for command in sorted(required):
        if command == "anvilminimal" and not command_available(command):
            status[command] = "present" if local_binary_exists() else "missing"
        else:
            status[command] = "present" if command_available(command) else "missing"
    return status


def local_binary_exists() -> bool:
    return Path("target/debug/anvilminimal").exists() or Path("target/release/anvilminimal").exists()


if __name__ == "__main__":
    sys.exit(main())
