#[test]
fn benchmark_has_25_scenarios() {
    let text = std::fs::read_to_string("benchmarks/minimal-loop-expanded.yaml").unwrap();
    assert_eq!(text.matches("  - id:").count(), 25);
}
