from __future__ import annotations

import json
import shlex
import signal
import socket
import subprocess
import time
import urllib.error
import urllib.request
from pathlib import Path
from typing import Any

from .artifacts import append_jsonl
from .process import run_capture


def port_available(port: int, host: str = "127.0.0.1") -> bool:
    with socket.socket(socket.AF_INET, socket.SOCK_STREAM) as sock:
        sock.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
        try:
            sock.bind((host, port))
        except OSError:
            return False
    return True


def run_postcheck(
    scenario: dict[str, Any],
    workdir: Path,
    out_dir: Path,
    timeout_sec: int | None = None,
) -> dict[str, Any]:
    out_dir.mkdir(parents=True, exist_ok=True)
    events_path = out_dir / "events.jsonl"
    if events_path.exists():
        events_path.unlink()
    ok = True
    dependency_elapsed = 0.0
    postcheck_elapsed = 0.0
    missing = []
    for artifact in scenario.get("expected_artifacts", []) or []:
        exists = (workdir / artifact).exists()
        append_jsonl(events_path, {"event": "expected_artifact", "path": artifact, "exists": exists})
        if not exists:
            ok = False
            missing.append(artifact)
    postcheck = scenario.get("postcheck", {}) or {}
    for index, command in enumerate(postcheck.get("commands", []) or []):
        is_dependency = is_dependency_command(command)
        result = run_capture(
            shlex.split(command),
            cwd=workdir,
            timeout_sec=timeout_sec or int(scenario.get("timeouts", {}).get("total_sec", 1800)),
            stdout_path=out_dir / f"command-{index}.stdout.log",
            stderr_path=out_dir / f"command-{index}.stderr.log",
        )
        if is_dependency:
            dependency_elapsed += result.elapsed_sec
        postcheck_elapsed += result.elapsed_sec
        append_jsonl(
            events_path,
            {
                "event": "postcheck",
                "command": command,
                "rc": result.rc,
                "elapsed_sec": round(result.elapsed_sec, 3),
                "dependency": is_dependency,
                "timeout": result.timeout,
            },
        )
        if result.rc != 0:
            ok = False
    dev = postcheck.get("dev_server")
    if dev:
        dev_result = run_dev_server(dev, workdir, out_dir)
        postcheck_elapsed += dev_result["elapsed_sec"]
        append_jsonl(events_path, {"event": "dev_server", **dev_result})
        ok = ok and bool(dev_result["ready"])
    return {
        "ok": ok,
        "missing_artifacts": missing,
        "postcheck_elapsed_sec": round(postcheck_elapsed, 3),
        "dependency_elapsed_sec": round(dependency_elapsed, 3),
        "events_path": str(events_path),
    }


def is_dependency_command(command: str) -> bool:
    lowered = command.lower()
    return " install" in lowered or lowered.startswith("npm install") or lowered.startswith("pnpm install")


def run_dev_server(dev: dict[str, Any], workdir: Path, out_dir: Path) -> dict[str, Any]:
    command = shlex.split(dev["command"])
    stdout_file = (out_dir / "dev-server.stdout.log").open("w", encoding="utf-8")
    stderr_file = (out_dir / "dev-server.stderr.log").open("w", encoding="utf-8")
    start = time.monotonic()
    proc = subprocess.Popen(
        command,
        cwd=str(workdir),
        stdout=stdout_file,
        stderr=stderr_file,
        text=True,
        start_new_session=True,
    )
    ready = False
    status = None
    readiness = dev.get("readiness", {})
    url = readiness.get("url")
    expect_status = int(readiness.get("expect_status", 200))
    timeout = float(readiness.get("timeout_sec", 60))
    try:
        deadline = time.monotonic() + timeout
        while time.monotonic() < deadline:
            if proc.poll() is not None:
                break
            try:
                with urllib.request.urlopen(url, timeout=2) as resp:
                    status = resp.status
                    if status == expect_status:
                        ready = True
                        break
            except urllib.error.HTTPError as err:
                status = err.code
            except Exception:
                pass
            time.sleep(0.2)
    finally:
        shutdown = "none"
        if proc.poll() is None:
            shutdown = dev.get("shutdown", "signal")
            try:
                proc.send_signal(signal.SIGTERM)
                proc.wait(timeout=5)
            except subprocess.TimeoutExpired:
                proc.kill()
                shutdown = "kill"
        stdout_file.close()
        stderr_file.close()
    return {
        "command": dev["command"],
        "port": int(dev.get("port", 0)),
        "ready": ready,
        "status": status,
        "elapsed_sec": round(time.monotonic() - start, 3),
        "shutdown": shutdown,
    }


def load_postcheck_events(path: Path) -> list[dict[str, Any]]:
    if not path.exists():
        return []
    return [json.loads(line) for line in path.read_text(encoding="utf-8").splitlines() if line.strip()]

