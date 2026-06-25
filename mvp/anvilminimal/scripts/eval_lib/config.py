from __future__ import annotations

import os
from pathlib import Path


def load_dotenv(path: Path | None = None) -> dict[str, str]:
    path = path or find_dotenv()
    values: dict[str, str] = {}
    if path is None or not path.exists():
        return values
    for raw in path.read_text(encoding="utf-8").splitlines():
        line = raw.strip()
        if not line or line.startswith("#") or "=" not in line:
            continue
        key, value = line.split("=", 1)
        values[key.strip()] = value.strip().strip('"').strip("'")
    return values


def find_dotenv(start: Path | None = None) -> Path | None:
    current = (start or Path.cwd()).resolve()
    candidates = [current, *current.parents]
    for directory in candidates:
        path = directory / ".env"
        if path.exists():
            return path
    return None


def merge_dotenv_into_env(env: dict[str, str], dotenv: dict[str, str] | None = None) -> dict[str, str]:
    dotenv = dotenv or load_dotenv()
    merged = dict(env)
    for key in ("OPENAI_API_KEY", "GEMINI_API_KEY"):
        if key not in merged and dotenv.get(key):
            merged[key] = dotenv[key]
    return merged


def env_value(name: str, dotenv: dict[str, str] | None = None) -> str | None:
    if os.environ.get(name):
        return os.environ[name]
    dotenv = dotenv or load_dotenv()
    return dotenv.get(name)


def credential_status(provider: str, dotenv: dict[str, str] | None = None) -> str:
    if provider == "openai":
        return "present" if env_value("OPENAI_API_KEY", dotenv) else "missing"
    if provider == "gemini":
        return "present" if env_value("GEMINI_API_KEY", dotenv) else "missing"
    if provider == "ollama":
        return "not_required"
    return "unknown_provider"
