from __future__ import annotations

from copy import deepcopy
from pathlib import Path
from typing import Any

from .simple_yaml import load_yaml


VALID_SIZES = {"small", "medium", "large"}


def load_suite(path: str | Path) -> dict[str, Any]:
    path = Path(path)
    data = load_yaml(path)
    name = data.get("name") or path.stem
    scenarios: list[dict[str, Any]] = []
    for include in data.get("include_suites", []) or []:
        included = load_suite((path.parent / include).resolve())
        scenarios.extend(included["scenarios"])
    for benchmark in data.get("include_benchmarks", []) or []:
        scenarios.extend(load_benchmark_as_scenarios((path.parent / benchmark).resolve()))
    for scenario in data.get("scenarios", []) or []:
        scenarios.append(normalize_scenario(scenario))
    ids = set()
    deduped = []
    for scenario in scenarios:
        if scenario["id"] in ids:
            continue
        ids.add(scenario["id"])
        deduped.append(scenario)
    return {"name": name, "description": data.get("description", ""), "scenarios": deduped}


def load_benchmark_as_scenarios(path: Path) -> list[dict[str, Any]]:
    data = load_yaml(path)
    out = []
    for raw in data.get("scenarios", []):
        sid = raw["id"]
        size = infer_size(sid)
        category = infer_category(sid)
        out.append(
            normalize_scenario(
                {
                    "id": f"seed-{sid}",
                    "size": size,
                    "category": category,
                    "profile": "nextjs" if "nextjs" in sid else "generic",
                    "prompt": raw.get("prompt", sid),
                    "expected_artifacts": [],
                    "postcheck": {"commands": []},
                    "plan_constraints": {
                        "min_steps": 1 if size == "small" else 3,
                        "max_steps": 5 if size == "small" else 9,
                        "required_verify_keywords": [],
                    },
                }
            )
        )
    return out


def normalize_scenario(raw: dict[str, Any]) -> dict[str, Any]:
    scenario = deepcopy(raw)
    for key in ("id", "prompt"):
        if not scenario.get(key):
            raise ValueError(f"scenario missing {key}: {scenario}")
    scenario.setdefault("size", infer_size(scenario["id"]))
    scenario.setdefault("category", infer_category(scenario["id"]))
    scenario.setdefault("profile", "generic")
    scenario.setdefault("expected_artifacts", [])
    scenario.setdefault("postcheck", {"commands": []})
    scenario.setdefault("timeouts", {})
    scenario.setdefault("plan_constraints", {})
    scenario["timeouts"].setdefault("total_sec", 1800)
    scenario["timeouts"].setdefault("model_call_sec", 300)
    scenario["plan_constraints"].setdefault("min_steps", 1 if scenario["size"] == "small" else 3)
    scenario["plan_constraints"].setdefault("max_steps", 5 if scenario["size"] == "small" else 9)
    scenario["plan_constraints"].setdefault("required_verify_keywords", [])
    if scenario["size"] not in VALID_SIZES:
        raise ValueError(f"invalid scenario size: {scenario['size']}")
    return scenario


def infer_size(sid: str) -> str:
    sid = sid.lower()
    if "large" in sid or "multi" in sid or "long" in sid or "nextjs" in sid:
        return "large"
    if "medium" in sid or "schema" in sid or "typescript" in sid or "rust" in sid:
        return "medium"
    return "small"


def infer_category(sid: str) -> str:
    sid = sid.lower()
    if any(word in sid for word in ["repair", "parser-feedback", "tool", "zero-test", "blocked"]):
        return "recovery"
    if any(word in sid for word in ["nextjs", "alias", "port", "readme", "docs", "manifest"]):
        return "config-profile"
    if any(word in sid for word in ["data", "schema", "report"]):
        return "data-docs"
    if sid.startswith("new") or "scaffold" in sid or "copy" in sid:
        return "new-code"
    return "fix-code"

