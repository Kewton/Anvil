import sys
import tempfile
import unittest
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(ROOT / "scripts"))

from eval_lib.report import compare_summaries, generate_report
from eval_lib.run_summary import SUMMARY_HEADER, read_summary, write_summary


class SummarySchemaTest(unittest.TestCase):
    def test_fixture_header(self):
        rows = read_summary(ROOT / "eval/fixtures/summaries/baseline.summary.eval.tsv")
        self.assertEqual(set(rows[0].keys()), set(SUMMARY_HEADER))

    def test_write_read_round_trip(self):
        with tempfile.TemporaryDirectory() as td:
            path = Path(td) / "summary.eval.tsv"
            write_summary(path, [{"run_id": "x", "suite": "s"}])
            row = read_summary(path)[0]
            self.assertEqual(row["run_id"], "x")
            self.assertIn("eval_schema_version", row)

    def test_legacy_summary_subset_is_read_with_schema_default(self):
        with tempfile.TemporaryDirectory() as td:
            path = Path(td) / "legacy.summary.eval.tsv"
            path.write_text("run_id\tsuite\nx\ts\n", encoding="utf-8")
            row = read_summary(path)[0]
            self.assertEqual(row["run_id"], "x")
            self.assertEqual(row["eval_schema_version"], "legacy")

    def test_compare_generates_markdown(self):
        text = compare_summaries(
            ROOT / "eval/fixtures/summaries/baseline.summary.eval.tsv",
            ROOT / "eval/fixtures/summaries/experiment.summary.eval.tsv",
        )
        self.assertIn("# Eval Compare", text)
        self.assertIn("success_rate", text)
        self.assertIn("acceptance_success_rate", text)
        self.assertIn("acceptance_false_positive_count", text)
        self.assertIn("capability_acceptance", text)
        self.assertIn("plan_output_adherence_score_avg", text)
        self.assertIn("plan_capability_contract_score_avg", text)
        self.assertIn("prompt_plan_capability_coverage_score_avg", text)
        self.assertIn("plan_verify_coverage_score_avg", text)
        self.assertIn("plan_verify_declared_coverage_score_avg", text)
        self.assertIn("executed_verify_coverage_score_avg", text)
        self.assertIn("acceptance_confidence_score_avg", text)
        self.assertIn("valid_plan_generated_rate", text)
        self.assertIn("plan_quality_score_avg", text)
        self.assertIn("execution_shape_readiness_score_avg", text)
        self.assertIn("plan_run_predictive_score_avg", text)
        self.assertIn("plan_run_readiness_score_avg", text)
        self.assertIn("verify_policy_readiness_score_avg", text)
        self.assertIn("contract_handoff_score_avg", text)
        self.assertIn("declared_contract_completeness_score_avg", text)
        self.assertIn("runner_handoff_integrity_score_avg", text)
        self.assertIn("postcheck_contract_alignment_score_avg", text)
        self.assertIn("dependency_ordering_score_avg", text)
        self.assertIn("finalization_readiness_score_avg", text)
        self.assertIn("plan_run_runtime_health_score_avg", text)
        self.assertIn("execution_contract_adherence_score_avg", text)
        self.assertIn("execution_contract_adherence_raw_score_avg", text)
        self.assertIn("execution_contract_min_subscore_avg", text)
        self.assertIn("postcheck_stability_score_avg", text)
        self.assertIn("ultra_runtime_health_score_avg", text)
        self.assertIn("phase_completion_score_avg", text)
        self.assertIn("build_verify_pass_score_avg", text)
        self.assertIn("build_verifier_completion_score_avg", text)
        self.assertIn("dependency_setup_boundary_score_avg", text)
        self.assertIn("dependency_setup_bridge_score_avg", text)
        self.assertIn("build_verifier_lifecycle_score_avg", text)
        self.assertIn("profile_repair_symmetry_score_avg", text)
        self.assertIn("step_runtime_bridge_score_avg", text)
        self.assertIn("repair_target_followthrough_score_avg", text)
        self.assertIn("plan_run_success_predictor_avg", text)
        self.assertIn("repair_target_resolution_score_avg", text)
        self.assertIn("build_repair_effectiveness_score_avg", text)
        self.assertIn("compile_diagnostic_progress_score_avg", text)
        self.assertIn("verify_repair_edit_score_avg", text)
        self.assertIn("prompt_contract_score_avg", text)
        self.assertIn("step_obligation_scope_score_avg", text)
        self.assertIn("executable_plan_score_avg", text)
        self.assertIn("constraint_coverage_score_avg", text)
        self.assertIn("verify_strength_score_avg", text)
        self.assertIn("verify_adequacy_score_avg", text)
        self.assertIn("semantic_verify_coverage_score_avg", text)
        self.assertIn("behavior_oracle_declared_score_avg", text)
        self.assertIn("contentless_verify_penalty_avg", text)
        self.assertIn("artifact_ownership_score_avg", text)
        self.assertIn("lint_repair_score_avg", text)
        self.assertIn("stability_score_avg", text)
        self.assertIn("runtime_friction_raw_score_avg", text)
        self.assertIn("step_finalization_score_avg", text)

    def test_report_uses_failure_kind_from_extras(self):
        with tempfile.TemporaryDirectory() as td:
            run_root = Path(td)
            row = {key: "" for key in SUMMARY_HEADER}
            row.update(
                {
                    "run_id": "x",
                    "suite": "s",
                    "scenario": "scenario",
                    "size": "small",
                    "category": "provider-smoke",
                    "mode": "minimal-loop",
                    "main_provider": "openai",
                    "success": "false",
                    "rc": "1",
                    "stop_reason": "verify_repair_exhausted",
                    "last_blocking_reason": "command failed",
                    "planner_stage": "lint",
                    "planner_error_kind": "planner_lint_error",
                    "planner_schema_repaired": "true",
                    "extras_json": {"failure_kind": "tool_validation_error"},
                }
            )
            write_summary(run_root / "summary.eval.tsv", [row])
            report = generate_report(run_root)
            self.assertIn("blocking: all required runs failed", report)
            self.assertIn("## Eval Schema", report)
            self.assertIn("## Speed Diagnostics", report)
            self.assertIn("| tool_validation_error | 1 |", report)
            self.assertIn("verify_repair_exhausted", report)
            self.assertIn("command failed", report)
            self.assertIn("## Planner Failures", report)
            self.assertIn("planner_lint_error", report)
            self.assertIn("## Planner Repairs", report)
            self.assertIn("schema_repaired", report)
            self.assertIn("## Failure Layers", report)
            self.assertIn("| runtime | 1 | 1 | 0 |", report)


if __name__ == "__main__":
    unittest.main()
