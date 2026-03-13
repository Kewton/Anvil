use anvil::metrics::{BenchmarkTarget, ComparisonAxis, MeasurementRecord, MetricsRegistry};

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
            notes: "cold start".to_string(),
        },
        MeasurementRecord {
            target: BenchmarkTarget::VibeLocal,
            scenario_id: "startup_latency_ms".to_string(),
            value: 610,
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
            notes: "clear console separation".to_string(),
        },
        MeasurementRecord {
            target: BenchmarkTarget::VibeLocal,
            scenario_id: "ux_clarity_score".to_string(),
            value: 3,
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
            notes: String::new(),
        },
        MeasurementRecord {
            target: BenchmarkTarget::VibeLocal,
            scenario_id: "startup_latency_ms".to_string(),
            value: 610,
            notes: String::new(),
        },
    ]);

    assert!(markdown.contains("# Competitive Validation Summary"));
    assert!(markdown.contains("Startup latency"));
    assert!(markdown.contains("Anvil"));
}
