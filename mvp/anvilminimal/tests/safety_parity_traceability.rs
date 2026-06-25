#[derive(Debug)]
struct SafetyTrace {
    id: &'static str,
    test_or_defer: &'static str,
}

const TRACES: &[SafetyTrace] = &[
    SafetyTrace {
        id: "SG-01",
        test_or_defer: "early_success_paths_stop_after_tool_execution",
    },
    SafetyTrace {
        id: "SG-02",
        test_or_defer: "plan_step_expected_paths_stop_after_tool_execution",
    },
    SafetyTrace {
        id: "SG-03",
        test_or_defer: "prompt_requested_artifact_feedback_then_write",
    },
    SafetyTrace {
        id: "SG-04",
        test_or_defer: "completion_without_write_feedback_then_write_then_complete",
    },
    SafetyTrace {
        id: "SG-05",
        test_or_defer: "repeated_planned_action_without_tool_returns_error",
    },
    SafetyTrace {
        id: "SG-06",
        test_or_defer: "prompt_requested_artifact_feedback_then_write",
    },
    SafetyTrace {
        id: "SG-07",
        test_or_defer: "defer:別計画: frontend relative import scanner is larger than minimal loop convergence fix",
    },
    SafetyTrace {
        id: "SG-08",
        test_or_defer: "defer:別計画: edit fallback parity needs edit-tool focused implementation",
    },
    SafetyTrace {
        id: "SG-09",
        test_or_defer: "missing_tool_argument_feedback_allows_retry",
    },
    SafetyTrace {
        id: "SG-10",
        test_or_defer: "planned_action_without_tool_discards_failed_assistant",
    },
    SafetyTrace {
        id: "SG-11",
        test_or_defer: "defer:別計画: workspace policy wiring needs read/glob/grep rollout",
    },
    SafetyTrace {
        id: "SG-12",
        test_or_defer: "system_prompt_contains_source_safety_rules",
    },
    SafetyTrace {
        id: "SG-13",
        test_or_defer: "verify_command_rejects_shell_control_syntax",
    },
    SafetyTrace {
        id: "SG-14",
        test_or_defer: "defer:別計画: typed StepKind changes YAML compatibility surface",
    },
    SafetyTrace {
        id: "SG-15",
        test_or_defer: "defer:別計画: semantic step kind lint follows typed StepKind",
    },
    SafetyTrace {
        id: "SG-16",
        test_or_defer: "defer:別計画: workspace semantic plan lint needs profile-specific rollout",
    },
    SafetyTrace {
        id: "SG-17",
        test_or_defer: "invalid_planner_output_gets_corrective_retry",
    },
    SafetyTrace {
        id: "SG-18",
        test_or_defer: "defer:別計画: per-step iteration caps require config plumbing",
    },
    SafetyTrace {
        id: "SG-19",
        test_or_defer: "defer:別計画: progress-aware bounded repair requires repair runner redesign",
    },
    SafetyTrace {
        id: "SG-20",
        test_or_defer: "defer:別計画: repair exhausted report depends on changed-file repair tracking",
    },
    SafetyTrace {
        id: "SG-21",
        test_or_defer: "defer:別計画: verification aggregation changes public report shape",
    },
    SafetyTrace {
        id: "SG-22",
        test_or_defer: "nextjs_build_missing_next_binary_is_dependency_missing",
    },
    SafetyTrace {
        id: "SG-23",
        test_or_defer: "required_final_artifacts_are_preserved_in_ultra_phase_prompt",
    },
    SafetyTrace {
        id: "SG-24",
        test_or_defer: "ultra_plan_non_final_profile_failure_stops",
    },
    SafetyTrace {
        id: "SG-25",
        test_or_defer: "defer:別計画: nextjs Tailwind/rootDir contract is profile-specific expansion",
    },
    SafetyTrace {
        id: "SG-26",
        test_or_defer: "test_failure_kind_max_iterations_snapshot",
    },
    SafetyTrace {
        id: "SG-27",
        test_or_defer: "empty_response_gets_one_retry_feedback",
    },
    SafetyTrace {
        id: "SG-28",
        test_or_defer: "completion_without_write_feedback_then_write_then_complete",
    },
    SafetyTrace {
        id: "SG-29",
        test_or_defer: "xml_fallback_prompt_contains_tool_call_example",
    },
    SafetyTrace {
        id: "SG-30",
        test_or_defer: "tool_call_assistant_preamble_is_not_reprompted",
    },
    SafetyTrace {
        id: "SG-31",
        test_or_defer: "defer:別計画: compaction evidence protection needs state-level migration",
    },
    SafetyTrace {
        id: "SG-32",
        test_or_defer: "defer:別計画: bash timeout/process group requires runner replacement",
    },
    SafetyTrace {
        id: "SG-33",
        test_or_defer: "test_expected_artifacts_can_be_rendered_as_required_final_artifacts",
    },
    SafetyTrace {
        id: "SG-34",
        test_or_defer: "defer:別計画: data profile raw-input protection is profile-specific expansion",
    },
    SafetyTrace {
        id: "SG-35",
        test_or_defer: "defer:別計画: read/glob/grep ignore policy needs tool-wide rollout",
    },
    SafetyTrace {
        id: "SG-36",
        test_or_defer: "defer:別計画: edit normalized fallback is edit-tool focused work",
    },
    SafetyTrace {
        id: "SG-37",
        test_or_defer: "duplicate_step_ids_are_rejected",
    },
    SafetyTrace {
        id: "SG-38",
        test_or_defer: "defer:別計画: bash output shaping needs test-output summarizer port",
    },
];

#[test]
fn safety_traceability_all_sg_have_test_or_defer_note() {
    let allowed_defer_prefixes = [
        "defer:旧 heavy loop由来:",
        "defer:別計画:",
        "defer:意図的非互換:",
    ];
    assert_eq!(TRACES.len(), 38);
    for number in 1..=38 {
        let id = format!("SG-{number:02}");
        let trace = TRACES
            .iter()
            .find(|trace| trace.id == id)
            .unwrap_or_else(|| panic!("{id} missing from traceability table"));
        assert!(!trace.test_or_defer.trim().is_empty(), "{id}");
        if trace.test_or_defer.starts_with("defer:") {
            assert!(
                allowed_defer_prefixes
                    .iter()
                    .any(|prefix| trace.test_or_defer.starts_with(prefix)),
                "{id} has invalid defer reason: {}",
                trace.test_or_defer
            );
        }
    }
}
