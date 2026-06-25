from __future__ import annotations

import subprocess
import time
from dataclasses import dataclass
from pathlib import Path


@dataclass
class ProcessResult:
    rc: int
    elapsed_sec: float
    timeout: bool
    stdout: str
    stderr: str


def run_capture(
    command: list[str],
    cwd: Path,
    timeout_sec: int,
    stdout_path: Path | None = None,
    stderr_path: Path | None = None,
    env: dict[str, str] | None = None,
) -> ProcessResult:
    start = time.monotonic()
    try:
        proc = subprocess.run(
            command,
            cwd=str(cwd),
            env=env,
            text=True,
            capture_output=True,
            timeout=timeout_sec,
        )
        elapsed = time.monotonic() - start
        stdout = proc.stdout or ""
        stderr = proc.stderr or ""
        timeout = False
        rc = proc.returncode
    except subprocess.TimeoutExpired as err:
        elapsed = time.monotonic() - start
        stdout = err.stdout or ""
        stderr = err.stderr or ""
        if isinstance(stdout, bytes):
            stdout = stdout.decode("utf-8", errors="replace")
        if isinstance(stderr, bytes):
            stderr = stderr.decode("utf-8", errors="replace")
        timeout = True
        rc = 124
    if stdout_path:
        stdout_path.parent.mkdir(parents=True, exist_ok=True)
        stdout_path.write_text(stdout, encoding="utf-8")
    if stderr_path:
        stderr_path.parent.mkdir(parents=True, exist_ok=True)
        stderr_path.write_text(stderr, encoding="utf-8")
    return ProcessResult(rc=rc, elapsed_sec=elapsed, timeout=timeout, stdout=stdout, stderr=stderr)


def command_available(name: str) -> bool:
    return subprocess.run(["/usr/bin/env", "sh", "-c", f"command -v {name}"], capture_output=True).returncode == 0

