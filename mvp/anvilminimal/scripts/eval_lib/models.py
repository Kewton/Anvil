from __future__ import annotations

import json
import subprocess
import urllib.request
from dataclasses import dataclass
from pathlib import Path
from typing import Any

from .simple_yaml import load_yaml


VALID_MODELS: dict[str, set[str]] = {
    "ollama": {"qwen3.6:27b-coding-nvfp4"},
    "openai": {"gpt-5.4-mini"},
    "gemini": {"gemini-3.1-flash", "gemini-3.5-flash"},
}


@dataclass(frozen=True)
class ModelRef:
    provider: str
    model: str

    @property
    def raw(self) -> str:
        return f"{self.provider}:{self.model}"

    @property
    def is_local(self) -> bool:
        return self.provider == "ollama"


def normalize_model_ref(raw: str) -> tuple[ModelRef, list[dict[str, str]]]:
    warnings: list[dict[str, str]] = []
    original = raw
    if raw.startswith("gollama:"):
        raw = "ollama:" + raw.split(":", 1)[1]
        warnings.append({"kind": "model_typo_normalized", "from": original, "to": raw})
    if raw.startswith("gemini:emini-"):
        raw = "gemini:gemini-" + raw.split("gemini:emini-", 1)[1]
        warnings.append({"kind": "model_typo_normalized", "from": original, "to": raw})
    provider, sep, model = raw.partition(":")
    if not sep or not provider or not model:
        raise ValueError(f"model must be provider:model: {original}")
    ref = ModelRef(provider=provider, model=model)
    if provider not in VALID_MODELS:
        raise ValueError(f"unknown provider: {provider}")
    if model not in VALID_MODELS[provider]:
        raise ValueError(f"unknown model for {provider}: {model}")
    return ref, warnings


def cli_model_args(main: ModelRef, planner: ModelRef) -> list[str]:
    return [
        "--provider",
        main.provider,
        "--model",
        main.model,
        "--planner-provider",
        planner.provider,
        "--planner-model",
        planner.model,
    ]


def load_model_profiles(path: str | Path) -> tuple[dict[str, Any], list[dict[str, str]]]:
    data = load_yaml(path)
    profiles = data.get("profiles", {})
    warnings: list[dict[str, str]] = []
    normalized: dict[str, Any] = {}
    for name, profile in profiles.items():
        runs = []
        for item in profile.get("runs", []):
            main, main_warnings = normalize_model_ref(item["main"])
            planner, planner_warnings = normalize_model_ref(item["planner"])
            warnings.extend(main_warnings)
            warnings.extend(planner_warnings)
            local = main.is_local or planner.is_local or bool(profile.get("serial"))
            runs.append(
                {
                    "main": main,
                    "planner": planner,
                    "serial_lane": bool(local),
                    "local_llm_used": main.is_local or planner.is_local,
                }
            )
        normalized[name] = {
            "name": name,
            "serial": bool(profile.get("serial")),
            "parallel": bool(profile.get("parallel", not profile.get("serial"))),
            "provider_limit": int(profile.get("provider_limit", 2)),
            "runs": runs,
        }
    return normalized, warnings


def required_providers_for_profile(profile: dict[str, Any]) -> set[str]:
    providers: set[str] = set()
    for run in profile["runs"]:
        providers.add(run["main"].provider)
        providers.add(run["planner"].provider)
    return providers


def ollama_models(host: str = "http://localhost:11434", timeout_sec: int = 5) -> set[str]:
    url = host.rstrip("/") + "/api/tags"
    with urllib.request.urlopen(url, timeout=timeout_sec) as resp:
        payload = json.loads(resp.read().decode("utf-8"))
    return {item.get("name", "") for item in payload.get("models", [])}


def command_exists(command: str) -> bool:
    return subprocess.run(["/usr/bin/env", "sh", "-c", f"command -v {command}"], capture_output=True).returncode == 0

