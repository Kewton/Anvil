import sys
import tempfile
import unittest
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(ROOT / "scripts"))

from eval_lib.runtime_scoring import score_runtime_health


class RuntimeScoringTest(unittest.TestCase):
    def test_runtime_health_separates_stalled_inspection_from_artifact_progress(self):
        scenario = {"expected_artifacts": ["date-helper.js"]}
        stalled = [
            {"event": "provider_response", "tool_calls": 1},
            {"event": "tool_call_raw", "name": "Glob"},
            {"event": "tool_call_raw", "name": "Read"},
            {"event": "tool_call_raw", "name": "Read"},
            {"event": "tool_call_raw", "name": "Read"},
            {"event": "tool_call_raw", "name": "Read"},
            {"event": "tool_call_raw", "name": "Read"},
            {"event": "provider_response", "tool_calls": 0},
            {"event": "artifact_stagnation_feedback"},
            {"event": "loop_stop", "reason": "max_iterations"},
        ]
        successful = [
            {"event": "provider_response", "tool_calls": 1},
            {"event": "tool_call_raw", "name": "Write"},
            {"event": "tool_execute", "name": "Write", "status": "ok"},
            {"event": "loop_stop", "reason": "required_artifacts_satisfied_after_tool"},
        ]
        with tempfile.TemporaryDirectory() as td:
            workdir = Path(td)
            stalled_score = score_runtime_health(
                stalled,
                mode="plan-run",
                success=False,
                scenario=scenario,
                workdir=workdir,
            )
            (workdir / "date-helper.js").write_text("module.exports = {}", encoding="utf-8")
            success_score = score_runtime_health(
                successful,
                mode="plan-run",
                success=True,
                scenario=scenario,
                workdir=workdir,
            )
        self.assertLess(stalled_score["runtime_friction_score"], success_score["runtime_friction_score"])
        self.assertLess(stalled_score["artifact_progress_score"], success_score["artifact_progress_score"])
        self.assertLess(stalled_score["plan_run_runtime_health_score"], success_score["plan_run_runtime_health_score"])

    def test_runtime_health_is_blank_without_runtime_events(self):
        with tempfile.TemporaryDirectory() as td:
            score = score_runtime_health(
                [{"event": "plan_score", "score": 80}],
                mode="plan-run",
                success=True,
                scenario={"expected_artifacts": ["README.md"]},
                workdir=Path(td),
            )
        self.assertEqual(score["runtime_friction_score"], "")
        self.assertEqual(score["plan_run_runtime_health_score"], "")


if __name__ == "__main__":
    unittest.main()
