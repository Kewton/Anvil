"""Security regression tests for scripts/compare.py.

Cases are built dynamically on a tempdir so they do not pollute the golden
snapshot fixtures. They cover symlink escape, missing directories, model_slug
shape violations, and the MIN_VALID_RUNS floor.

Run from repo root:
    python3 tests/test_compare_security.py
"""

from __future__ import annotations

import json
import os
import pathlib
import subprocess
import sys
import tempfile
import unittest

REPO_ROOT = pathlib.Path(__file__).resolve().parents[1]
SCRIPT = REPO_ROOT / "scripts" / "compare.py"


def _run(
    baseline: pathlib.Path,
    experiment: pathlib.Path,
    *extra: str,
) -> subprocess.CompletedProcess[str]:
    return subprocess.run(
        [sys.executable, str(SCRIPT), str(baseline), str(experiment), *extra],
        capture_output=True,
        text=True,
        check=False,
        cwd=str(REPO_ROOT),
    )


def _write_json(path: pathlib.Path, obj: object) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(json.dumps(obj), encoding="utf-8")


def _build_run(run_dir: pathlib.Path, rc: int, elapsed_s: int, run_id: str) -> None:
    run_dir.mkdir(parents=True, exist_ok=True)
    _write_json(
        run_dir / "meta.json",
        {
            "rc": rc,
            "elapsed_s": elapsed_s,
            "model": "test-model",
            "start_ts": "2026-01-01T00:00:00Z",
        },
    )
    _write_json(
        run_dir / "session.json",
        {
            "id": run_id,
            "messages": [
                {"role": "assistant", "content": "ok", "tool_calls": []},
            ],
        },
    )


def _build_bench_root(
    root: pathlib.Path, slug: str, n_runs: int, *, prefix: str = ""
) -> None:
    for i in range(1, n_runs + 1):
        _build_run(
            root / slug / f"run-{i}",
            rc=0,
            elapsed_s=100 + i,
            run_id=f"{prefix}run-{i}",
        )


class TestCompareSecurity(unittest.TestCase):
    def setUp(self) -> None:
        self._tmp = tempfile.TemporaryDirectory()
        self.addCleanup(self._tmp.cleanup)
        self.tmp = pathlib.Path(self._tmp.name)
        self.baseline = self.tmp / "baseline"
        self.experiment = self.tmp / "experiment"

    def test_baseline_symlink_exits_1(self) -> None:
        real_baseline = self.tmp / "real-baseline"
        _build_bench_root(real_baseline, "test-model", 3, prefix="b-")
        _build_bench_root(self.experiment, "test-model", 3, prefix="e-")
        os.symlink(real_baseline, self.baseline)

        r = _run(self.baseline, self.experiment)
        self.assertEqual(r.returncode, 1, r.stderr)

    def test_experiment_symlink_exits_1(self) -> None:
        real_experiment = self.tmp / "real-experiment"
        _build_bench_root(self.baseline, "test-model", 3, prefix="b-")
        _build_bench_root(real_experiment, "test-model", 3, prefix="e-")
        os.symlink(real_experiment, self.experiment)

        r = _run(self.baseline, self.experiment)
        self.assertEqual(r.returncode, 1, r.stderr)

    def test_missing_baseline_exits_2(self) -> None:
        _build_bench_root(self.experiment, "test-model", 3, prefix="e-")
        # baseline does not exist
        r = _run(self.baseline, self.experiment)
        self.assertEqual(r.returncode, 2, r.stderr)

    def test_missing_experiment_exits_2(self) -> None:
        _build_bench_root(self.baseline, "test-model", 3, prefix="b-")
        # experiment does not exist
        r = _run(self.baseline, self.experiment)
        self.assertEqual(r.returncode, 2, r.stderr)

    def test_zero_model_slug_exits_1(self) -> None:
        self.baseline.mkdir()
        _build_bench_root(self.experiment, "test-model", 3, prefix="e-")
        r = _run(self.baseline, self.experiment)
        self.assertEqual(r.returncode, 1, r.stderr)

    def test_multiple_model_slug_exits_1(self) -> None:
        _build_bench_root(self.baseline, "model-a", 3, prefix="ba-")
        _build_bench_root(self.baseline, "model-b", 3, prefix="bb-")
        _build_bench_root(self.experiment, "model-a", 3, prefix="ea-")
        r = _run(self.baseline, self.experiment)
        self.assertEqual(r.returncode, 1, r.stderr)

    def test_model_slug_mismatch_exits_1(self) -> None:
        _build_bench_root(self.baseline, "model-a", 3, prefix="b-")
        _build_bench_root(self.experiment, "model-b", 3, prefix="e-")
        r = _run(self.baseline, self.experiment)
        self.assertEqual(r.returncode, 1, r.stderr)

    def test_min_valid_runs_exits_3(self) -> None:
        _build_bench_root(self.baseline, "test-model", 2, prefix="b-")
        _build_bench_root(self.experiment, "test-model", 3, prefix="e-")
        r = _run(self.baseline, self.experiment)
        self.assertEqual(r.returncode, 3, r.stderr)

    def test_rc130_filtered_to_below_min_exits_3(self) -> None:
        # All 3 baseline runs have rc=130 -> filtered -> 0 valid runs
        for i in range(1, 4):
            _build_run(
                self.baseline / "test-model" / f"run-{i}",
                rc=130,
                elapsed_s=100 + i,
                run_id=f"b-{i}",
            )
        _build_bench_root(self.experiment, "test-model", 3, prefix="e-")
        r = _run(self.baseline, self.experiment)
        self.assertEqual(r.returncode, 3, r.stderr)


if __name__ == "__main__":
    unittest.main()
