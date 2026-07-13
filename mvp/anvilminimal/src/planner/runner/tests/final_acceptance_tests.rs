#[cfg(test)]
mod moved {
    use super::super::*;

    #[test]
    fn final_acceptance_repair_prompt_prefix_keeps_attempt_counter_at_tail() {
        let dir = tempfile::tempdir().unwrap();
        let plan = UltraPlan {
            goal: "Build a game".to_string(),
            profile: "nextjs".to_string(),
            style: "default".to_string(),
            intent: "create".to_string(),
            phases: Vec::new(),
        };
        let context = UltraRunContext::new(vec!["src/app/page.tsx".to_string()]);
        let report = VerificationReport::profile_failed(
            "missing_required_evidence:restart_or_recoverable_state_evidence",
        );
        let first = final_acceptance_repair_prompt(
            dir.path(),
            PromptLayout::Stable,
            &plan,
            &report,
            &context,
            "implementation",
            &["src/app/page.tsx".to_string()],
            &[],
            (1, 2),
            false,
            false,
        );
        let second = final_acceptance_repair_prompt(
            dir.path(),
            PromptLayout::Stable,
            &plan,
            &report,
            &context,
            "implementation",
            &["src/app/page.tsx".to_string()],
            &[],
            (2, 2),
            false,
            false,
        );

        let prefix = common_prefix(&first, &second);

        assert!(prefix.contains("Bounded repair rules:"), "{prefix}");
        assert!(prefix.contains("Original ultra goal:"), "{prefix}");
        assert!(
            prefix.contains("Pending capability evidence remedies:"),
            "{prefix}"
        );
        assert!(prefix.ends_with("- attempt: "), "{prefix}");
    }

    #[test]
    fn final_acceptance_evidence_regeneration_prompt_targets_route_bound_source() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("src/app")).unwrap();
        std::fs::write(
            dir.path().join("src/app/page.tsx"),
            "export default function Page(){ return <button>Restart</button>; }\n",
        )
        .unwrap();
        let mut report = VerificationReport::profile_failed(
            "missing_required_evidence:restart_or_recoverable_state_evidence",
        );
        report.compile_errors.clear();
        let plan = UltraPlan::deterministic(
            "Create an interactive browser game with restart flow",
            "nextjs",
            "default",
            "create",
        );

        assert!(evidence_repair_zero_edit_eligible(
            &report,
            RepairTarget::Implementation
        ));
        let target = final_acceptance_evidence_regeneration_target(
            dir.path(),
            "nextjs",
            &report,
            &["src/app/page.tsx".to_string()],
        )
        .expect("evidence regeneration target");
        let prompt = build_final_acceptance_evidence_regeneration_prompt(
            dir.path(),
            &plan,
            &report,
            &target,
        );

        assert_eq!(target, "src/app/page.tsx");
        assert!(prompt.contains("Repair session mode: compact regeneration"));
        assert!(prompt.contains("restart_or_recoverable_state_evidence"));
        assert!(prompt.contains("Current content of src/app/page.tsx"));
        assert!(prompt.contains(
            "Write the complete corrected file via the Write tool (full content, one file only): src/app/page.tsx"
        ));
    }

    #[test]
    fn final_acceptance_evidence_regeneration_event_records_gate_and_resolution() {
        let dir = tempfile::tempdir().unwrap();
        let events = dir.path().join("events.jsonl");
        let mut cfg = config(dir.path().to_path_buf());
        cfg.eval_events_path = Some(events.clone());
        emit_evidence_regeneration_event(
            &cfg,
            true,
            true,
            Some("src/app/page.tsx"),
            &["restart_or_recoverable_state_evidence".to_string()],
            &[],
            &["src/app/page.tsx".to_string()],
            "accepted",
        );

        let event = latest_event(&events, "repair_regeneration");
        assert_eq!(
            event.get("lifecycle_stage").and_then(Value::as_str),
            Some("final_acceptance_repair")
        );
        assert_eq!(
            event.get("repair_session_mode").and_then(Value::as_str),
            Some("compact_regeneration")
        );
        assert_eq!(
            event.get("regeneration_gate").and_then(Value::as_str),
            Some("evidence_static_present_and_build_passes")
        );
        assert_eq!(event.get("accepted").and_then(Value::as_bool), Some(true));
        assert!(
            event
                .get("resolved_missing_evidence")
                .and_then(Value::as_array)
                .is_some_and(|items| items
                    .iter()
                    .any(|item| item.as_str() == Some("restart_or_recoverable_state_evidence"))),
            "{event}"
        );
    }

    #[test]
    fn final_acceptance_evidence_regeneration_event_records_skip_decision() {
        let dir = tempfile::tempdir().unwrap();
        let events = dir.path().join("events.jsonl");
        let mut cfg = config(dir.path().to_path_buf());
        cfg.eval_events_path = Some(events.clone());
        emit_evidence_regeneration_event(
            &cfg,
            false,
            false,
            Some("src/app/page.tsx"),
            &["restart_or_recoverable_state_evidence".to_string()],
            &["restart_or_recoverable_state_evidence".to_string()],
            &[],
            "capability_evidence_unresolved:restart_or_recoverable_state_evidence",
        );

        let event = latest_event(&events, "repair_regeneration");
        assert_eq!(event.get("fired").and_then(Value::as_bool), Some(false));
        assert_eq!(event.get("accepted").and_then(Value::as_bool), Some(false));
        assert_eq!(
            event
                .get("changed_paths")
                .and_then(Value::as_array)
                .unwrap()
                .len(),
            0
        );
        assert_eq!(
            event.get("repair_session_mode").and_then(Value::as_str),
            Some("")
        );
        assert!(
            event
                .get("before_missing_evidence")
                .and_then(Value::as_array)
                .is_some_and(|items| items
                    .iter()
                    .any(|item| item.as_str() == Some("restart_or_recoverable_state_evidence"))),
            "{event}"
        );
    }

    #[test]
    fn final_acceptance_repair_prompt_adds_plan_adherence_as_secondary_guidance() {
        let mut report = VerificationReport::pass();
        report.push_profile_failure(
            "missing_required_evidence:user_input_handler_evidence".to_string(),
        );
        let plan = UltraPlan {
            goal: "Create an interactive browser game".to_string(),
            profile: "nextjs".to_string(),
            style: "default".to_string(),
            intent: "create".to_string(),
            phases: Vec::new(),
        };

        let prompt = final_acceptance_repair_prompt(
            Path::new("."),
            PromptLayout::Stable,
            &plan,
            &report,
            &UltraRunContext::default(),
            "required_evidence_missing",
            &["src/app/page.tsx".to_string()],
            &[
                "keyboard".to_string(),
                "lives".to_string(),
                "pause".to_string(),
            ],
            (1, 1),
            false,
            false,
        );

        assert!(
            prompt.contains(
                "Profile failures:\n- missing_required_evidence:user_input_handler_evidence"
            ),
            "{prompt}"
        );
        assert!(
        prompt.contains(
            "Secondary plan-adherence guidance:\n- also close if in scope: keyboard, lives, pause"
        ),
        "{prompt}"
    );
    }

    #[test]
    fn final_acceptance_repair_prompt_includes_state_binding_diagnosis_for_interaction_evidence() {
        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join("src/app");
        std::fs::create_dir_all(&src).unwrap();
        std::fs::write(
            dir.path().join("package.json"),
            nextjs_complete_package_json(),
        )
        .unwrap();
        std::fs::write(
            src.join("page.tsx"),
            r#"
import { useRef } from "react";
export default function Quiz() {
  const quizRef = useRef({ score: 0 });
  const answer = () => { quizRef.current.score += 1; };
  return <main><button onClick={answer}>Answer</button><output data-anvil-state={JSON.stringify({ score: quizRef.current.score })} /></main>;
}
"#,
        )
        .unwrap();
        let interaction = dir.path().join("browser-interaction.json");
        std::fs::write(
            &interaction,
            r#"{"failure_kind":"input_state_change_missing_after_start","contract_hook_status":"primary_missing","action_hooks":[],"state_dimensions_changed":[]}"#,
        )
        .unwrap();
        let events = dir.path().join("events.jsonl");
        let mut report = VerificationReport::profile_failed(
            "browser_interaction_failed:input_state_change_missing_after_start",
        );
        report.push_profile_failure(format!(
            "interaction evidence path: {}",
            interaction.display()
        ));
        let plan = UltraPlan {
            goal: "Create a Next.js quiz app with answer buttons, score, and retry behavior"
                .to_string(),
            profile: "nextjs".to_string(),
            style: "default".to_string(),
            intent: "create".to_string(),
            phases: Vec::new(),
        };
        let pending_evidence = final_acceptance_repair_signals(&report);
        assert_eq!(
            pending_evidence,
            vec!["browser_interaction_failed:input_state_change_missing_after_start"]
        );
        let diagnosis = crate::planner::state_binding_scan::final_acceptance_actionable_diagnosis(
            dir.path(),
            "nextjs",
            &report,
        )
        .unwrap();
        assert_eq!(diagnosis.path, "src/app/page.tsx");
        assert_eq!(diagnosis.diagnosis.as_str(), "state_bound_to_ref");
        let selection = crate::planner::repair_targeting::resolve_final_acceptance_repair_targets(
            crate::planner::repair_targeting::FinalAcceptanceRepairTargetInput {
                root: dir.path(),
                profile: "nextjs",
                pending_evidence: &pending_evidence,
                contract_attribute_paths: &[],
                repair_changed_paths: &[],
                required_paths: &["package.json".to_string(), "src/app/page.tsx".to_string()],
                diagnosis_path: Some(&diagnosis.path),
            },
        );
        assert_eq!(selection.selected_targets, vec!["src/app/page.tsx"]);
        assert_eq!(selection.selection_reason, "diagnosis_mapped");

        let prompt = final_acceptance_repair_prompt_with_events(
            dir.path(),
            PromptLayout::Stable,
            &plan,
            &report,
            &UltraRunContext::default(),
            "required_evidence_missing",
            &["src/app/page.tsx".to_string()],
            &[],
            (1, 1),
            false,
            false,
            Some(&events),
        );

        assert!(
            prompt.contains("State binding repair guidance:"),
            "{prompt}"
        );
        assert!(
            prompt.contains("State binding diagnosis: state_bound_to_ref"),
            "{prompt}"
        );
        assert!(
            prompt.contains("after start and after input, the `data-anvil-state` JSON value"),
            "{prompt}"
        );
        assert!(
            prompt.contains("Contract attribute repair guidance:"),
            "{prompt}"
        );
        assert!(
            prompt.contains(r#"data-anvil-action="primary""#),
            "{prompt}"
        );
        assert!(prompt.contains("src/app/page.tsx"), "{prompt}");
        assert!(prompt.contains("near line"), "{prompt}");
        assert!(
            prompt.contains(
                &crate::planner::profiles::nextjs::knowledge::get()
                    .repair_guidance
                    .generic_interaction
            ),
            "{prompt}"
        );
        assert!(!prompt.contains("projectiles"), "{prompt}");
        assert!(!prompt.contains("rAF loop"), "{prompt}");
        assert!(
            prompt.contains(
                &crate::planner::profiles::nextjs::knowledge::get()
                    .contracts
                    .primary_example
            ),
            "{prompt}"
        );
        let event_text = std::fs::read_to_string(events).unwrap();
        assert!(
            event_text.contains(r#""event":"state_binding_diagnosis""#),
            "{event_text}"
        );
        assert!(
            event_text.contains(r#""event":"contract_attribute_repair_guidance""#),
            "{event_text}"
        );
        assert!(
            event_text.contains(r#""attribute":"data-anvil-action=\"primary\"""#),
            "{event_text}"
        );
    }

    #[test]
    fn interaction_failure_without_diagnosis_uses_route_bound_evidence_mapping() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("src/app")).unwrap();
        std::fs::write(
            dir.path().join("package.json"),
            nextjs_complete_package_json(),
        )
        .unwrap();
        std::fs::write(
            dir.path().join("src/app/page.tsx"),
            "export default function Page(){ return <main>Quiz</main>; }\n",
        )
        .unwrap();
        let report = VerificationReport::profile_failed(
            "browser_interaction_failed:input_state_change_missing_after_start",
        );
        let pending_evidence = final_acceptance_repair_signals(&report);

        let selection = crate::planner::repair_targeting::resolve_final_acceptance_repair_targets(
            crate::planner::repair_targeting::FinalAcceptanceRepairTargetInput {
                root: dir.path(),
                profile: "nextjs",
                pending_evidence: &pending_evidence,
                contract_attribute_paths: &[],
                repair_changed_paths: &[],
                required_paths: &["package.json".to_string()],
                diagnosis_path: None,
            },
        );

        assert_eq!(selection.selected_targets, vec!["src/app/page.tsx"]);
        assert_eq!(selection.selection_reason, "evidence_mapped");
    }

    #[test]
    fn final_acceptance_repair_prompt_omits_state_binding_diagnosis_for_compile_failure() {
        let report = VerificationReport::command_failed(
            "npm run build",
            "implementation_compile_error: TS2304 missing name",
        );
        let plan = UltraPlan::deterministic(
            "Create an interactive browser game",
            "nextjs",
            "default",
            "create",
        );

        let prompt = final_acceptance_repair_prompt(
            Path::new("."),
            PromptLayout::Stable,
            &plan,
            &report,
            &UltraRunContext::default(),
            "compile_error",
            &["src/app/page.tsx".to_string()],
            &[],
            (1, 1),
            false,
            false,
        );

        assert!(
            !prompt.contains("State binding repair guidance:"),
            "{prompt}"
        );
        assert!(!prompt.contains("setter_never_called"), "{prompt}");
    }

    #[test]
    fn compile_regeneration_target_skips_multi_file_failures() {
        let mut report = VerificationReport::pass();
        report.compile_errors = vec![
            CompileError {
                path: "src/app/page.tsx".to_string(),
                line: 1,
                column: 1,
                message: "Type error: first".to_string(),
                excerpt: String::new(),
                symbol: None,
                route_bound: Some(true),
            },
            CompileError {
                path: "src/app/game.ts".to_string(),
                line: 2,
                column: 1,
                message: "Type error: second".to_string(),
                excerpt: String::new(),
                symbol: None,
                route_bound: Some(true),
            },
        ];

        assert_eq!(
            single_compile_regeneration_target(&report).unwrap_err(),
            "multi_file_compile_failure"
        );
    }
}
