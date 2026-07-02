from __future__ import annotations

import json
import os
import re
from datetime import datetime
from pathlib import Path
from typing import Any


def default_eval_root(cwd: Path | None = None) -> Path:
    cwd = cwd or Path.cwd()
    env = os.environ.get("ANVIL_EVAL_ROOT")
    if env:
        return Path(env)
    return (cwd / "../../workspace/eval-artifacts/anvilminimal-mvp").resolve()


def create_run_root(root: Path | None = None, timestamp: str | None = None) -> Path:
    base = root or default_eval_root()
    stamp = timestamp or datetime.now().strftime("%Y%m%d-%H%M%S-%f")
    path = base / stamp
    suffix = 1
    while path.exists() and any(path.iterdir()):
        path = base / f"{stamp}-{suffix}"
        suffix += 1
    path.mkdir(parents=True, exist_ok=True)
    (path / "runs").mkdir(exist_ok=True)
    return path


def sanitize(value: str) -> str:
    value = value.strip().replace(":", "-").replace("/", "-")
    value = re.sub(r"[^A-Za-z0-9_.-]+", "_", value)
    return value.strip("_") or "x"


def run_id(
    suite: str,
    scenario: str,
    mode: str,
    main_provider: str,
    main_model: str,
    planner_provider: str,
    planner_model: str,
    run_index: int,
) -> str:
    parts = [
        sanitize(suite),
        sanitize(scenario),
        sanitize(mode),
        f"{sanitize(main_provider)}-{sanitize(main_model)}",
        f"{sanitize(planner_provider)}-{sanitize(planner_model)}",
        f"r{run_index}",
    ]
    return "__".join(parts)


def write_json(path: Path, data: Any) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(json.dumps(data, ensure_ascii=False, indent=2, sort_keys=True) + "\n", encoding="utf-8")


def append_jsonl(path: Path, event: dict[str, Any]) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    with path.open("a", encoding="utf-8") as f:
        f.write(json.dumps(event, ensure_ascii=False, sort_keys=True) + "\n")


def write_jsonl(path: Path, events: list[dict[str, Any]]) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    with path.open("w", encoding="utf-8") as f:
        for event in events:
            f.write(json.dumps(event, ensure_ascii=False, sort_keys=True) + "\n")


def copy_snapshot(src: Path, dst: Path, excludes: set[str] | None = None) -> None:
    excludes = excludes or {"node_modules", "target", ".next", ".git"}
    dst.mkdir(parents=True, exist_ok=True)
    for child in src.iterdir():
        if child.name in excludes:
            continue
        target = dst / child.name
        if child.is_dir():
            copy_snapshot(child, target, excludes)
        else:
            target.parent.mkdir(parents=True, exist_ok=True)
            target.write_bytes(child.read_bytes())
