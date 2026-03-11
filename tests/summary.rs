use anvil::state::summary::{SummaryController, SummaryInput, SummaryPolicy};

#[test]
fn summary_policy_table_driven_thresholds() {
    let policy = SummaryPolicy::default();
    let cases = vec![
        (
            SummaryInput {
                tokens: 10_000,
                turns: 4,
            },
            false,
        ),
        (
            SummaryInput {
                tokens: 50_000,
                turns: 10,
            },
            true,
        ),
        (
            SummaryInput {
                tokens: 12_000,
                turns: 24,
            },
            true,
        ),
        (
            SummaryInput {
                tokens: 97_000,
                turns: 5,
            },
            true,
        ),
    ];

    for (input, expected) in cases {
        assert_eq!(policy.should_summarize(input), expected);
    }
}

#[test]
fn summary_controller_truncates_large_output_and_produces_rolling_summary() {
    let policy = SummaryPolicy::default();
    let controller = SummaryController::new(policy);
    let history = vec![
        "User asked for a refactor".to_string(),
        "Agent inspected files and found repeated parsing code".to_string(),
        "Tool output: ".to_string() + &"x".repeat(12_000),
    ];

    let summary = controller.summarize_history(&history);
    let truncated = controller.truncate_tool_output(&"y".repeat(15_000), 2_000);

    assert!(summary.contains("Rolling summary"));
    assert!(summary.len() < 800);
    assert!(truncated.len() <= 2_050);
    assert!(truncated.contains("truncated"));
}

#[test]
fn summary_controller_recommends_subagent_for_large_contexts() {
    let policy = SummaryPolicy::default();
    let controller = SummaryController::new(policy);

    assert!(controller.should_spawn_subagent(98_000, 30));
    assert!(!controller.should_spawn_subagent(20_000, 5));
}
