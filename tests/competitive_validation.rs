use anvil::metrics::{
    BenchmarkArtifact, BenchmarkTarget, CommandBenchmark, ComparisonAxis, MeasurementRecord,
    MeasurementSource, MetricsRegistry,
};

#[test]
fn metrics_registry_exposes_phase9_scenarios() {
    let registry = MetricsRegistry::new();

    assert!(
        registry
            .scenarios()
            .iter()
            .any(|scenario| scenario.axis == ComparisonAxis::FirstUseExperience)
    );
    assert!(
        registry
            .scenarios()
            .iter()
            .any(|scenario| scenario.axis == ComparisonAxis::IterationSpeed)
    );
}

#[test]
fn compare_picks_lower_value_for_latency_metrics() {
    let registry = MetricsRegistry::new();
    let records = vec![
        MeasurementRecord {
            target: BenchmarkTarget::Anvil,
            scenario_id: "startup_latency_ms".to_string(),
            value: 420,
            source: MeasurementSource::Measured,
            notes: "cold start".to_string(),
        },
        MeasurementRecord {
            target: BenchmarkTarget::VibeLocal,
            scenario_id: "startup_latency_ms".to_string(),
            value: 610,
            source: MeasurementSource::Measured,
            notes: "cold start".to_string(),
        },
    ];

    let outcome = registry
        .compare("startup_latency_ms", &records)
        .expect("scenario should exist");

    assert_eq!(outcome.winner, Some(BenchmarkTarget::Anvil));
}

#[test]
fn compare_picks_higher_value_for_quality_scores() {
    let registry = MetricsRegistry::new();
    let records = vec![
        MeasurementRecord {
            target: BenchmarkTarget::Anvil,
            scenario_id: "ux_clarity_score".to_string(),
            value: 5,
            source: MeasurementSource::OperationalScore,
            notes: "clear console separation".to_string(),
        },
        MeasurementRecord {
            target: BenchmarkTarget::VibeLocal,
            scenario_id: "ux_clarity_score".to_string(),
            value: 3,
            source: MeasurementSource::OperationalScore,
            notes: "mixed output".to_string(),
        },
    ];

    let outcome = registry
        .compare("ux_clarity_score", &records)
        .expect("scenario should exist");

    assert_eq!(outcome.winner, Some(BenchmarkTarget::Anvil));
}

#[test]
fn markdown_summary_renders_registered_scenarios() {
    let registry = MetricsRegistry::new();
    let markdown = registry.render_markdown_summary(&[
        MeasurementRecord {
            target: BenchmarkTarget::Anvil,
            scenario_id: "startup_latency_ms".to_string(),
            value: 420,
            source: MeasurementSource::Measured,
            notes: String::new(),
        },
        MeasurementRecord {
            target: BenchmarkTarget::VibeLocal,
            scenario_id: "startup_latency_ms".to_string(),
            value: 610,
            source: MeasurementSource::Measured,
            notes: String::new(),
        },
    ]);

    assert!(markdown.contains("# Competitive Validation Summary"));
    assert!(markdown.contains("Startup latency"));
    assert!(markdown.contains("Anvil"));
    assert!(markdown.contains("Measured"));
    assert!(
        markdown
            .contains("| Startup latency | FirstUseExperience | Measured | 420 | 610 | Anvil |")
    );
}

#[test]
fn command_benchmark_runs_multiple_times_and_reports_average_ms() {
    let benchmark = CommandBenchmark::new("python3", &["-c", "import time; time.sleep(0.01)"]);

    let result = benchmark.run(2).expect("benchmark command should succeed");

    assert_eq!(result.runs_ms.len(), 2);
    assert!(result.average_ms >= 5);
}

#[test]
fn command_benchmark_surfaces_non_zero_exit_as_error() {
    let benchmark = CommandBenchmark::new("python3", &["-c", "import sys; sys.exit(3)"]);
    let err = benchmark.run(1).expect_err("non-zero exit should fail");

    assert!(err.to_string().contains("benchmark command failed"));
}

#[test]
fn command_benchmark_can_build_run_log_artifact() {
    let benchmark = CommandBenchmark::new("python3", &["-c", "import time; time.sleep(0.01)"]);

    let artifact = benchmark
        .run_artifact("startup_latency_ms", BenchmarkTarget::Anvil, 2)
        .expect("artifact benchmark should succeed");

    assert_eq!(artifact.scenario_id, "startup_latency_ms");
    assert_eq!(artifact.target, BenchmarkTarget::Anvil);
    assert_eq!(artifact.runs_ms.len(), 2);
    assert!(artifact.average_ms >= 5);
}

#[test]
fn run_log_renders_raw_benchmark_artifacts() {
    let registry = MetricsRegistry::new();
    let log = registry.render_run_log(&[BenchmarkArtifact {
        scenario_id: "startup_latency_ms".to_string(),
        target: BenchmarkTarget::Anvil,
        command: "cargo run --quiet -- --help".to_string(),
        runs_ms: vec![210, 220, 220],
        average_ms: 216,
    }]);

    assert!(log.contains("# Competitive Validation Run Log"));
    assert!(log.contains("## Startup latency"));
    assert!(log.contains("- average_ms: 216"));
}
