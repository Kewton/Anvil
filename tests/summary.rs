use anvil::state::summary::{CarryoverState, SummaryController, SummaryInput, SummaryPolicy};

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
fn summary_controller_estimates_tokens_and_builds_prompt_prefix() {
    let controller = SummaryController::new(SummaryPolicy::default());
    let history = vec![
        "You requested a browser-runnable game".to_string(),
        "Tool wrote ./sandbox/demo/index.html".to_string(),
    ];
    let summary = controller.summarize_for_carryover(None, &history);
    let carryover = CarryoverState {
        rolling_summary: Some(summary.clone()),
        summarized_events: 12,
    };

    let estimated = controller.estimate_tokens(&history, Some(&summary));
    let prefix = controller.prompt_prefix(&carryover).unwrap();

    assert!(estimated > 0);
    assert!(prefix.contains("Session carryover summary"));
    assert!(prefix.contains("Summarized prior events: 12"));
}

#[test]
fn compact_history_returns_summary_and_retains_recent_events() {
    let controller = SummaryController::new(SummaryPolicy::default());
    let history = (0..16)
        .map(|idx| format!("event {idx} ./sandbox/file{idx}.html"))
        .collect::<Vec<_>>();

    let outcome = controller.compact_history(None, &history).unwrap();

    assert!(outcome.rolling_summary.contains("Changed files"));
    assert_eq!(
        outcome.retained_events,
        controller.policy().keep_recent_events
    );
    assert_eq!(
        outcome.summarized_events,
        16 - controller.policy().keep_recent_events
    );
}

#[test]
fn summary_controller_recommends_subagent_for_large_contexts() {
    let policy = SummaryPolicy::default();
    let controller = SummaryController::new(policy);

    assert!(controller.should_spawn_subagent(98_000, 30));
    assert!(!controller.should_spawn_subagent(20_000, 5));
}
