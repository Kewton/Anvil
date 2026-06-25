use std::path::Path;

#[test]
fn eval_script_contract_files_exist() {
    let required = [
        "eval/model_profiles.yaml",
        "eval/suites/mvp-smoke.yaml",
        "eval/suites/mvp-balanced.yaml",
        "eval/suites/mvp-full.yaml",
        "scripts/eval-run.py",
        "scripts/eval-preflight.py",
        "scripts/eval-score-plan.py",
        "scripts/eval-postcheck.py",
        "scripts/eval-report.py",
        "scripts/eval-compare.py",
    ];
    for path in required {
        assert!(Path::new(path).exists(), "missing {path}");
    }
}

#[test]
fn eval_summary_schema_mentions_required_columns() {
    let schema = std::fs::read_to_string("eval/plan_score_schema.yaml").unwrap();
    for column in [
        "run_id",
        "mode",
        "plan_quality_score",
        "ultra_phase_quality_score",
        "overall_score",
    ] {
        assert!(schema.contains(column), "schema missing {column}");
    }
}
