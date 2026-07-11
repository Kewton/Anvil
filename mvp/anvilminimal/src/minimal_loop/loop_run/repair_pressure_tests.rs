#[cfg(test)]
mod moved {
    use super::*;

    #[test]
    fn setup_scaffold_completion_finishes_missing_nextjs_configs_at_budget_gate() {
        let dir = tempfile::tempdir().unwrap();
        let events = dir.path().join("events.jsonl");
        let mut cfg = config(dir.path().to_path_buf());
        cfg.profile = "nextjs".to_string();
        cfg.eval_events_path = Some(events.clone());
        cfg.max_iterations = 1;
        let required_paths = crate::planner::profiles::nextjs::setup_scaffold_paths(dir.path());
        let page = "export default function Page(){return <main>ready</main>;}\n";
        let mut fake = Fake::new(vec![
            Ok(AssistantReply {
                content: String::new(),
                tool_calls: vec![
                    ToolCall::new("Write", json!({"path":"package.json","content":"{}"})),
                    ToolCall::new("Write", json!({"path":"tsconfig.json","content":"{}"})),
                    ToolCall::new(
                        "Write",
                        json!({"path":"src/app/layout.tsx","content":"import './globals.css';\nexport default function RootLayout({children}:{children:React.ReactNode}){return <html><body>{children}</body></html>}\n"}),
                    ),
                    ToolCall::new("Write", json!({"path":"src/app/page.tsx","content":page})),
                    ToolCall::new(
                        "Write",
                        json!({"path":"src/app/globals.css","content":"@tailwind base;\n@tailwind components;\n@tailwind utilities;\n"}),
                    ),
                    ToolCall::new(
                        "Write",
                        json!({"path":"src/app/global.d.ts","content":"declare module \"*.css\";\n"}),
                    ),
                ],
                prompt_tokens: None,
                completion_tokens: None,
            }),
            Ok(empty_reply()),
        ]);
        let mut session = SessionSnapshot::new();

        let outcome = run_session_with_outcome_with_options(
            &mut fake,
            &mut session,
            "Scaffold the Next.js setup files.",
            &required_paths,
            &cfg,
            &NOOP_UI,
            RunSessionOptions::plan_step(RunSessionStepKind::Setup),
        )
        .unwrap();

        assert_eq!(
            outcome.stop_reason,
            RunStopReason::RequiredArtifactsSatisfiedAfterTool
        );
        assert_eq!(
            std::fs::read_to_string(dir.path().join("src/app/page.tsx")).unwrap(),
            page
        );
        let postcss = std::fs::read_to_string(dir.path().join("postcss.config.js")).unwrap();
        assert!(postcss.contains("tailwindcss"), "{postcss}");
        assert!(postcss.contains("autoprefixer"), "{postcss}");
        assert!(dir.path().join("tailwind.config.ts").is_file());
        assert!(
            outcome
                .changed_paths
                .contains(&"postcss.config.js".to_string())
        );
        assert!(
            outcome
                .changed_paths
                .contains(&"tailwind.config.ts".to_string())
        );
        let event_text = std::fs::read_to_string(events).unwrap();
        assert!(event_text.contains("\"event\":\"setup_scaffold_completed\""));
        assert!(event_text.contains("\"trigger\":\"budget_low\""));
        assert!(event_text.contains("postcss.config.js"));
        assert!(event_text.contains("tailwind.config.ts"));
    }

    #[test]
    fn setup_scaffold_completion_authors_absent_nextjs_application_page() {
        let dir = tempfile::tempdir().unwrap();
        let events = dir.path().join("events.jsonl");
        let mut cfg = config(dir.path().to_path_buf());
        cfg.profile = "nextjs".to_string();
        cfg.eval_events_path = Some(events.clone());
        cfg.max_iterations = 1;
        let mut fake = Fake::new(vec![
            Ok(empty_reply()),
            Ok(empty_reply()),
            Ok(empty_reply()),
            Ok(empty_reply()),
        ]);
        let mut session = SessionSnapshot::new();

        let outcome = run_session_with_outcome_with_options(
            &mut fake,
            &mut session,
            "Scaffold the Next.js setup files.",
            &["src/app/page.tsx".to_string()],
            &cfg,
            &NOOP_UI,
            RunSessionOptions::plan_step(RunSessionStepKind::Setup),
        )
        .unwrap();

        assert_eq!(
            outcome.stop_reason,
            RunStopReason::RequiredArtifactsSatisfiedAfterTool
        );
        assert!(dir.path().join("src/app/page.tsx").exists());
        let page = std::fs::read_to_string(dir.path().join("src/app/page.tsx")).unwrap();
        assert!(page.contains("export default function Page"), "{page}");
        let event_text = std::fs::read_to_string(events).unwrap_or_default();
        assert!(event_text.contains("\"event\":\"setup_scaffold_completed\""));
        assert!(event_text.contains("\"trigger\":\"budget_low\""));
    }

    #[test]
    fn artifact_recovery_exhaustion_rescues_nextjs_setup_page_scaffold() {
        let dir = tempfile::tempdir().unwrap();
        let events = dir.path().join("events.jsonl");
        let mut cfg = config(dir.path().to_path_buf());
        cfg.profile = "nextjs".to_string();
        cfg.eval_events_path = Some(events.clone());
        cfg.max_iterations = 8;
        let mut fake = Fake::new(vec![
            Ok(AssistantReply::text("I will create the page.")),
            Ok(AssistantReply::text("Still preparing.")),
            Ok(AssistantReply::text("I need to write it.")),
            Ok(AssistantReply::text("Scaffold now.")),
        ]);
        let mut session = SessionSnapshot::new();

        let outcome = run_session_with_outcome_with_options(
            &mut fake,
            &mut session,
            "Scaffold the Next.js setup page for test0708_018.",
            &["src/app/page.tsx".to_string()],
            &cfg,
            &NOOP_UI,
            RunSessionOptions::plan_step(RunSessionStepKind::Setup),
        )
        .unwrap();

        assert_eq!(
            outcome.stop_reason,
            RunStopReason::RequiredArtifactsSatisfiedAfterTool
        );
        assert!(dir.path().join("src/app/page.tsx").is_file());
        assert!(
            outcome
                .changed_paths
                .contains(&"src/app/page.tsx".to_string())
        );
        let event_text = std::fs::read_to_string(events).unwrap();
        assert!(event_text.contains("\"event\":\"artifact_stagnation_feedback\""));
        assert!(event_text.contains("\"event\":\"setup_scaffold_completed\""));
        assert!(event_text.contains("\"trigger\":\"exhausted\""));
        assert!(!event_text.contains("\"reason\":\"artifact_recovery_exhausted\""));
    }

    #[test]
    fn pre_scaffolded_setup_step_short_circuits_without_model_turn() {
        let dir = tempfile::tempdir().unwrap();
        let events = dir.path().join("events.jsonl");
        std::fs::create_dir_all(dir.path().join("src/app")).unwrap();
        std::fs::write(dir.path().join("src/app/page.tsx"), "ok").unwrap();
        let mut cfg = config(dir.path().to_path_buf());
        cfg.eval_events_path = Some(events.clone());
        let mut fake = Fake::new(Vec::new());
        let mut session = SessionSnapshot::new();

        let outcome = run_session_with_outcome_with_options(
            &mut fake,
            &mut session,
            "Current step id:\nsetup-scripts\n\nCurrent step instruction:\nConfirm the already present page.",
            &["src/app/page.tsx".to_string()],
            &cfg,
            &NOOP_UI,
            RunSessionOptions::plan_step(RunSessionStepKind::Setup),
        )
        .unwrap();

        assert_eq!(fake.remaining_replies(), 0);
        assert_eq!(outcome.iterations, 0);
        assert_eq!(outcome.tool_calls, 0);
        assert_eq!(
            outcome.stop_reason,
            RunStopReason::RequiredArtifactsSatisfiedAfterTool
        );
        let events = event_values(&events);
        assert!(events.iter().any(|event| {
            event.get("event").and_then(Value::as_str) == Some("step_short_circuited")
                && event.get("at").and_then(Value::as_str) == Some("start")
        }));
    }

    #[test]
    fn setup_step_with_missing_path_uses_normal_model_loop() {
        let dir = tempfile::tempdir().unwrap();
        let events = dir.path().join("events.jsonl");
        let mut cfg = config(dir.path().to_path_buf());
        cfg.eval_events_path = Some(events.clone());
        let mut fake = Fake::new(vec![Ok(AssistantReply {
            content: String::new(),
            tool_calls: vec![ToolCall::new(
                "Write",
                json!({"path": "src/app/page.tsx", "content": "ok"}),
            )],
            prompt_tokens: None,
            completion_tokens: None,
        })]);
        let mut session = SessionSnapshot::new();

        let outcome = run_session_with_outcome_with_options(
            &mut fake,
            &mut session,
            "Current step id:\nsetup-scripts\n\nCurrent step instruction:\nConfirm the missing page.",
            &["src/app/page.tsx".to_string()],
            &cfg,
            &NOOP_UI,
            RunSessionOptions::plan_step(RunSessionStepKind::Setup),
        )
        .unwrap();

        assert_eq!(fake.remaining_replies(), 0);
        assert_eq!(outcome.iterations, 1);
        assert_eq!(outcome.tool_calls, 1);
        assert!(dir.path().join("src/app/page.tsx").is_file());
        let event_text = std::fs::read_to_string(events).unwrap();
        assert!(!event_text.contains("\"event\":\"step_short_circuited\""));
    }

    #[test]
    fn artifact_stagnation_feedback_then_write_succeeds() {
        let dir = tempfile::tempdir().unwrap();
        let contract = dir.path().join("contract.json");
        std::fs::write(
            &contract,
            r#"{"required_paths":["date-helper.js"],"verify_commands":[]}"#,
        )
        .unwrap();
        let mut cfg = config(dir.path().to_path_buf());
        cfg.completion_contract_path = Some(contract);
        let mut fake = Fake::new(vec![
            Ok(AssistantReply {
                content: String::new(),
                tool_calls: vec![ToolCall::new("Glob", json!({"pattern":"**/*"}))],
                prompt_tokens: None,
                completion_tokens: None,
            }),
            Ok(AssistantReply {
                content: String::new(),
                tool_calls: vec![ToolCall::new("Grep", json!({"pattern":"date"}))],
                prompt_tokens: None,
                completion_tokens: None,
            }),
            Ok(AssistantReply {
                content: String::new(),
                tool_calls: vec![ToolCall::new("Bash", json!({"command":"ls"}))],
                prompt_tokens: None,
                completion_tokens: None,
            }),
            Ok(AssistantReply {
                content: String::new(),
                tool_calls: vec![ToolCall::new(
                    "Write",
                    json!({"path":"date-helper.js","content":"module.exports = {};\n"}),
                )],
                prompt_tokens: None,
                completion_tokens: None,
            }),
        ]);
        let mut session = SessionSnapshot::new();
        let outcome = run_session_with_outcome_with_ui(
            &mut fake,
            &mut session,
            "create date-helper.js",
            &[],
            &cfg,
            &NOOP_UI,
        )
        .unwrap();
        assert_eq!(
            outcome.stop_reason,
            RunStopReason::RequiredArtifactsSatisfiedAfterTool
        );
        assert!(dir.path().join("date-helper.js").is_file());
    }

    #[test]
    fn implement_read_only_stagnation_nudges_then_write_completes() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("notes.md"), "facts").unwrap();
        let events = dir.path().join("events.jsonl");
        let mut cfg = config(dir.path().to_path_buf());
        cfg.eval_events_path = Some(events.clone());
        cfg.max_iterations = 4;
        let mut fake = RecordingFake::new(vec![
            Ok(read_reply("notes.md")),
            Ok(read_reply("notes.md")),
            Ok(read_reply("notes.md")),
            Ok(AssistantReply {
                content: String::new(),
                tool_calls: vec![ToolCall::new(
                    "Write",
                    json!({"path":"implemented.txt","content":"done\n"}),
                )],
                prompt_tokens: None,
                completion_tokens: None,
            }),
            Ok(AssistantReply::text("done")),
        ]);
        let mut session = SessionSnapshot::new();
        let outcome = run_session_with_outcome_with_options(
            &mut fake,
            &mut session,
            "Implement the requested helper after inspecting notes.md.",
            &[],
            &cfg,
            &NOOP_UI,
            RunSessionOptions::plan_step(RunSessionStepKind::Implement),
        )
        .unwrap();

        assert_eq!(outcome.stop_reason, RunStopReason::AssistantFinal);
        assert!(dir.path().join("implemented.txt").is_file());
        let events = event_values(&events);
        let feedback = events
            .iter()
            .find(|event| {
                event.get("event").and_then(Value::as_str) == Some("read_only_stagnation_feedback")
            })
            .expect("read-only feedback event");
        assert_eq!(
            feedback.get("stage").and_then(Value::as_str),
            Some("intervention")
        );
        assert!(
            fake.requests().iter().any(|request| {
                request.iter().any(|message| {
                    message
                        .content
                        .contains("Inspection is sufficient - implement now via Write/Edit")
                })
            }),
            "{:#?}",
            fake.requests()
        );
    }

    #[test]
    fn implement_read_only_stagnation_exhausts_with_honest_classification() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("notes.md"), "facts").unwrap();
        let events = dir.path().join("events.jsonl");
        let mut cfg = config(dir.path().to_path_buf());
        cfg.eval_events_path = Some(events.clone());
        cfg.max_iterations = 2;
        let mut fake = Fake::new((0..5).map(|_| Ok(read_reply("notes.md"))).collect());
        let mut session = SessionSnapshot::new();
        let err = run_session_with_outcome_with_options(
            &mut fake,
            &mut session,
            "Implement the requested helper after inspecting notes.md.",
            &[],
            &cfg,
            &NOOP_UI,
            RunSessionOptions::plan_step(RunSessionStepKind::Implement),
        )
        .unwrap_err()
        .to_string();

        assert!(err.contains("model_stagnation:read_only_loop"), "{err}");
        assert!(err.contains("objective:"), "{err}");
        assert!(!err.contains("no concrete blocker recorded"), "{err}");
        let events = event_values(&events);
        let stages = events
            .iter()
            .filter(|event| {
                event.get("event").and_then(Value::as_str) == Some("read_only_stagnation_feedback")
            })
            .map(|event| {
                event
                    .get("stage")
                    .and_then(Value::as_str)
                    .unwrap()
                    .to_string()
            })
            .collect::<Vec<_>>();
        assert_eq!(stages, vec!["intervention", "compact_restatement"]);
        let stop = events
            .iter()
            .find(|event| event.get("event").and_then(Value::as_str) == Some("loop_stop"))
            .unwrap();
        assert_eq!(
            stop.get("reason").and_then(Value::as_str),
            Some("model_stagnation:read_only_loop")
        );
        assert_eq!(
            stop.get("read_only_streak").and_then(Value::as_u64),
            Some(5)
        );
    }

    #[test]
    fn artifact_recovery_exhausts_after_repeated_non_edit_tools() {
        let dir = tempfile::tempdir().unwrap();
        let contract = dir.path().join("contract.json");
        std::fs::write(
            &contract,
            r#"{"required_paths":["date-helper.js"],"verify_commands":[]}"#,
        )
        .unwrap();
        let mut cfg = config(dir.path().to_path_buf());
        cfg.completion_contract_path = Some(contract);
        cfg.max_iterations = 13;
        let mut replies = Vec::new();
        for _ in 0..12 {
            replies.push(Ok(AssistantReply {
                content: String::new(),
                tool_calls: vec![ToolCall::new("Glob", json!({"pattern":"**/*"}))],
                prompt_tokens: None,
                completion_tokens: None,
            }));
        }
        let mut fake = Fake::new(replies);
        let mut session = SessionSnapshot::new();
        let err = run_session_with_outcome_with_ui(
            &mut fake,
            &mut session,
            "create date-helper.js",
            &[],
            &cfg,
            &NOOP_UI,
        )
        .unwrap_err()
        .to_string();
        assert!(err.contains("artifact recovery exhausted"));
    }

    #[test]
    fn verify_failure_requires_edit_after_repeated_no_change() {
        let dir = tempfile::tempdir().unwrap();
        let contract = dir.path().join("contract.json");
        let events = dir.path().join("events.jsonl");
        std::fs::write(
            &contract,
            r#"{"required_paths":["a.py"],"verify_commands":["python3 -m py_compile a.py"],"verify_repair_cap":3}"#,
        )
        .unwrap();
        let mut cfg = config(dir.path().to_path_buf());
        cfg.completion_contract_path = Some(contract);
        cfg.eval_events_path = Some(events.clone());
        cfg.max_iterations = 6;
        let mut fake = Fake::new(vec![
            Ok(AssistantReply {
                content: String::new(),
                tool_calls: vec![ToolCall::new(
                    "Write",
                    json!({"path":"a.py","content":"def broken(:\n    pass\n"}),
                )],
                prompt_tokens: None,
                completion_tokens: None,
            }),
            Ok(AssistantReply {
                content: String::new(),
                tool_calls: vec![ToolCall::new("Read", json!({"path":"a.py"}))],
                prompt_tokens: None,
                completion_tokens: None,
            }),
            Ok(AssistantReply {
                content: String::new(),
                tool_calls: vec![ToolCall::new(
                    "Bash",
                    json!({"command":"python3 -m py_compile a.py"}),
                )],
                prompt_tokens: None,
                completion_tokens: None,
            }),
            Ok(AssistantReply {
                content: String::new(),
                tool_calls: vec![ToolCall::new("Read", json!({"path":"a.py"}))],
                prompt_tokens: None,
                completion_tokens: None,
            }),
        ]);
        let mut session = SessionSnapshot::new();
        let err = run_session_with_outcome_with_ui(
            &mut fake,
            &mut session,
            "create a.py",
            &[],
            &cfg,
            &NOOP_UI,
        )
        .unwrap_err()
        .to_string();
        assert!(err.contains("verify repair made no file changes"));
        assert!(
            err.contains("/run-ultra-plan .anvil/plans/recovery-ultra-plan-minimal-loop-"),
            "{err}"
        );
        assert!(err.contains(".yaml"), "{err}");
        let event_text = std::fs::read_to_string(events).unwrap();
        assert!(event_text.contains("\"reason\":\"verify_repair_no_change\""));
        assert!(event_text.contains("\"repair_follow_through\":\"no_change\""));
        assert!(event_text.contains("\"changed_paths_before\":[\"a.py\"]"));
        assert!(event_text.contains("\"changed_paths_after\":[\"a.py\"]"));
        assert!(event_text.contains("\"recovery_yaml_missing\":false"));
        assert!(dir.path().join(".anvil/repairs").is_dir());
        assert!(dir.path().join(".anvil/plans").is_dir());
    }

    #[test]
    fn observe_verify_repair_no_edit_returns_observation_without_handoff() {
        let dir = tempfile::tempdir().unwrap();
        let contract = dir.path().join("contract.json");
        let events = dir.path().join("events.jsonl");
        std::fs::write(
            &contract,
            r#"{"required_paths":["src/app/page.tsx"],"required_evidence":["challenge_or_adversary_evidence"],"verify_repair_cap":2}"#,
        )
        .unwrap();
        let mut cfg = config(dir.path().to_path_buf());
        cfg.completion_contract_path = Some(contract);
        cfg.eval_events_path = Some(events.clone());
        cfg.max_iterations = 6;
        let static_page =
            "export default function Page(){ return <main><canvas>ready</canvas></main>; }";
        let mut fake = Fake::new(vec![
            Ok(AssistantReply {
                content: String::new(),
                tool_calls: vec![ToolCall::new(
                    "Write",
                    json!({"path":"src/app/page.tsx","content":static_page}),
                )],
                prompt_tokens: None,
                completion_tokens: None,
            }),
            Ok(AssistantReply::text(
                "I inspected the page but made no edit.",
            )),
        ]);
        let mut session = SessionSnapshot::new();
        let outcome = run_session_with_outcome_with_options(
            &mut fake,
            &mut session,
            "Create src/app/page.tsx",
            &["src/app/page.tsx".to_string()],
            &cfg,
            &NOOP_UI,
            RunSessionOptions::plan_step_with_enforcement(
                RunSessionStepKind::Implement,
                ContractEnforcement::Observe,
                Some("phase-one".to_string()),
            ),
        )
        .unwrap();

        assert_eq!(
            outcome.stop_reason,
            RunStopReason::CompletionContractObservedIncomplete
        );
        assert_eq!(
            outcome.missing_evidence,
            vec!["challenge_or_adversary_evidence".to_string()]
        );
        assert!(!dir.path().join(".anvil/repairs").exists());
        let events = event_values(&events);
        assert!(events.iter().any(|event| {
            event.get("event").and_then(Value::as_str) == Some("loop_stop")
                && event.get("reason").and_then(Value::as_str)
                    == Some("verify_repair_no_change_observed")
                && event.get("contract_enforcement").and_then(Value::as_str) == Some("observe")
                && event.get("phase_scope").and_then(Value::as_str) == Some("phase-one")
        }));
        assert!(events.iter().any(|event| {
            event.get("event").and_then(Value::as_str) == Some("contract_observation_incomplete")
                && event
                    .get("missing_evidence")
                    .and_then(Value::as_array)
                    .is_some_and(|values| {
                        values
                            .iter()
                            .any(|value| value.as_str() == Some("challenge_or_adversary_evidence"))
                    })
        }));
    }

    #[test]
    fn enforce_verify_repair_no_edit_still_bails_and_saves_handoff() {
        let dir = tempfile::tempdir().unwrap();
        let contract = dir.path().join("contract.json");
        let events = dir.path().join("events.jsonl");
        std::fs::write(
            &contract,
            r#"{"required_paths":["src/app/page.tsx"],"required_evidence":["challenge_or_adversary_evidence"],"verify_repair_cap":2}"#,
        )
        .unwrap();
        let mut cfg = config(dir.path().to_path_buf());
        cfg.completion_contract_path = Some(contract);
        cfg.eval_events_path = Some(events.clone());
        cfg.max_iterations = 6;
        let static_page =
            "export default function Page(){ return <main><canvas>ready</canvas></main>; }";
        let mut fake = Fake::new(vec![
            Ok(AssistantReply {
                content: String::new(),
                tool_calls: vec![ToolCall::new(
                    "Write",
                    json!({"path":"src/app/page.tsx","content":static_page}),
                )],
                prompt_tokens: None,
                completion_tokens: None,
            }),
            Ok(AssistantReply::text(
                "I inspected the page but made no edit.",
            )),
        ]);
        let mut session = SessionSnapshot::new();
        let err = run_session_with_outcome_with_options(
            &mut fake,
            &mut session,
            "Create src/app/page.tsx",
            &["src/app/page.tsx".to_string()],
            &cfg,
            &NOOP_UI,
            RunSessionOptions::plan_step_with_enforcement(
                RunSessionStepKind::Implement,
                ContractEnforcement::Enforce,
                None,
            ),
        )
        .unwrap_err()
        .to_string();

        assert!(err.contains("verify repair made no file changes"), "{err}");
        assert!(dir.path().join(".anvil/repairs").is_dir());
        let events = event_values(&events);
        assert!(events.iter().any(|event| {
            event.get("event").and_then(Value::as_str) == Some("loop_stop")
                && event.get("reason").and_then(Value::as_str) == Some("verify_repair_no_change")
                && event.get("recovery_yaml_missing").and_then(Value::as_bool) == Some(false)
        }));
        assert!(!events.iter().any(|event| {
            event.get("reason").and_then(Value::as_str) == Some("verify_repair_no_change_observed")
        }));
    }

    #[test]
    fn verify_repair_progress_unchanged_after_edit_still_exhausts() {
        let dir = tempfile::tempdir().unwrap();
        let contract = dir.path().join("contract.json");
        std::fs::write(
            &contract,
            r#"{"required_paths":["a.py"],"verify_commands":["python3 -m py_compile a.py"],"verify_repair_cap":2}"#,
        )
        .unwrap();
        let mut cfg = config(dir.path().to_path_buf());
        cfg.completion_contract_path = Some(contract);
        let mut fake = Fake::new(vec![
            Ok(AssistantReply {
                content: String::new(),
                tool_calls: vec![ToolCall::new(
                    "Write",
                    json!({"path":"a.py","content":"def broken(:\n    pass\n"}),
                )],
                prompt_tokens: None,
                completion_tokens: None,
            }),
            Ok(AssistantReply {
                content: String::new(),
                tool_calls: vec![ToolCall::new(
                    "Write",
                    json!({"path":"a.py","content":"def broken(:\n    pass\n"}),
                )],
                prompt_tokens: None,
                completion_tokens: None,
            }),
        ]);
        let mut session = SessionSnapshot::new();
        let err = run_session_with_outcome_with_ui(
            &mut fake,
            &mut session,
            "create a.py",
            &[],
            &cfg,
            &NOOP_UI,
        )
        .unwrap_err()
        .to_string();
        assert!(err.contains("completion contract verify failed"));
    }

    #[test]
    fn identical_failure_without_file_changes_stops_with_progress_reason() {
        let dir = tempfile::tempdir().unwrap();
        let events = dir.path().join("events.jsonl");
        std::fs::write(dir.path().join("a.py"), "def broken(:\n    pass\n").unwrap();
        let contract = CompletionContract {
            required_paths: vec!["a.py".to_string()],
            verify_commands: vec!["python3 -m py_compile a.py".to_string()],
            profile: None,
            goal: None,
            required_capabilities: Vec::new(),
            deterministic_oracles: Vec::new(),
            required_evidence: Vec::new(),
            evidence_hint_tokens: Vec::new(),
            required_obligations: Vec::new(),
            deferred_verify_requirements: Vec::new(),
            verify_repair_cap: 2,
        }
        .validate(dir.path())
        .unwrap();
        let previous_report = contract.verify(dir.path());
        let previous_signature = VerificationSignature::from_report(&previous_report);
        let previous_target = classify_repair_target(&previous_report);
        let mut verify_attempts = 1;
        let err = verify_completion_contract(
            dir.path(),
            Some(&events),
            &contract,
            "fix a.py",
            &mut verify_attempts,
            Some(&previous_signature),
            Some(previous_target),
            &["a.py".to_string()],
            &["a.py".to_string()],
            &[],
            false,
            NodeDependencySetupAuthority::None,
            false,
        )
        .unwrap_err()
        .to_string();
        assert!(err.contains("completion contract verify failed"));
        let event_text = std::fs::read_to_string(events).unwrap();
        assert!(event_text.contains("\"reason\":\"verify_repair_progress_invalid\""));
        assert!(!event_text.contains("\"reason\":\"repair_target_not_followed\""));
        assert!(!event_text.contains("\"reason\":\"repair_unrelated_change\""));
    }

    #[test]
    fn target_not_followed_feedback_anchor_lists_contract_implementation_paths() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("src")).unwrap();
        std::fs::write(
            dir.path().join("src/main.rs"),
            "fn main(){ assert_eq!(1, 1); }\n",
        )
        .unwrap();
        let events = dir.path().join("events.jsonl");
        let contract = CompletionContract {
            required_paths: vec!["src/main.rs".to_string()],
            verify_commands: Vec::new(),
            profile: None,
            goal: None,
            required_capabilities: Vec::new(),
            deterministic_oracles: Vec::new(),
            required_evidence: vec![
                "test_artifact".to_string(),
                "non_zero_test_or_assertion_evidence".to_string(),
            ],
            evidence_hint_tokens: Vec::new(),
            required_obligations: vec!["implementation".to_string()],
            deferred_verify_requirements: Vec::new(),
            verify_repair_cap: 3,
        }
        .validate(dir.path())
        .unwrap();
        let mut previous_report = VerificationReport::pass();
        previous_report.push_profile_failure(
            "missing_required_evidence:test_artifact,non_zero_test_or_assertion_evidence",
        );
        let previous_signature = VerificationSignature::from_report(&previous_report);
        let previous_target = classify_repair_target(&previous_report);
        assert_eq!(previous_target, RepairTarget::RequiredEvidenceMissing);

        let mut verify_attempts = 1;
        let feedback = verify_completion_contract(
            dir.path(),
            Some(&events),
            &contract,
            "write a Rust program",
            &mut verify_attempts,
            Some(&previous_signature),
            Some(previous_target),
            &["src/main.rs".to_string()],
            &["src/main.rs".to_string()],
            &["src/main.rs".to_string()],
            true,
            NodeDependencySetupAuthority::None,
            false,
        )
        .unwrap()
        .expect("anchored feedback");
        assert!(feedback.feedback.starts_with(
            "Previous edit did not address the failure. You must edit one of the following files: src/main.rs"
        ));
        let event_text = std::fs::read_to_string(&events).unwrap();
        assert!(event_text.contains("\"repair_follow_through\":\"target_not_followed\""));
        assert!(event_text.contains("\"failure_kind\":\"repair_target_not_followed\""));
        assert!(!event_text.contains("\"event\":\"loop_stop\""));
    }

    #[test]
    fn target_not_followed_then_correct_entrypoint_edit_can_verify_successfully() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("src/app")).unwrap();
        let events = dir.path().join("events.jsonl");
        let contract = CompletionContract {
            required_paths: vec!["src/app/page.tsx".to_string()],
            verify_commands: Vec::new(),
            profile: None,
            goal: None,
            required_capabilities: Vec::new(),
            deterministic_oracles: Vec::new(),
            required_evidence: Vec::new(),
            evidence_hint_tokens: Vec::new(),
            required_obligations: Vec::new(),
            deferred_verify_requirements: Vec::new(),
            verify_repair_cap: 3,
        }
        .validate(dir.path())
        .unwrap();
        let previous_report = VerificationReport::missing_path("src/app/page.tsx");
        let previous_signature = VerificationSignature::from_report(&previous_report);
        let previous_target = classify_repair_target(&previous_report);
        assert_eq!(previous_target, RepairTarget::MissingEntrypoint);

        std::fs::write(
            dir.path().join("src/app/widget.tsx"),
            "export const Widget = null;\n",
        )
        .unwrap();
        let mut verify_attempts = 1;
        let feedback = verify_completion_contract(
            dir.path(),
            Some(&events),
            &contract,
            "create the app page",
            &mut verify_attempts,
            Some(&previous_signature),
            Some(previous_target),
            &[],
            &["src/app/widget.tsx".to_string()],
            &["src/app/widget.tsx".to_string()],
            true,
            NodeDependencySetupAuthority::None,
            false,
        )
        .unwrap()
        .expect("first repair should be re-anchored, not stopped");
        assert!(feedback.feedback.contains(
            "Previous edit did not address the failure. You must edit one of the following files:"
        ));

        std::fs::write(
            dir.path().join("src/app/page.tsx"),
            "export default function Page(){return <main>ok</main>;}\n",
        )
        .unwrap();
        let outcome = verify_completion_contract(
            dir.path(),
            Some(&events),
            &contract,
            "create the app page",
            &mut verify_attempts,
            Some(&feedback.signature),
            Some(feedback.target),
            &["src/app/widget.tsx".to_string()],
            &[
                "src/app/widget.tsx".to_string(),
                "src/app/page.tsx".to_string(),
            ],
            &["src/app/page.tsx".to_string()],
            true,
            NodeDependencySetupAuthority::None,
            false,
        )
        .unwrap();
        assert!(outcome.is_none());
        let event_text = std::fs::read_to_string(&events).unwrap();
        assert!(event_text.contains("\"repair_follow_through\":\"target_not_followed\""));
        assert!(event_text.contains("\"repair_follow_through\":\"target_matched\""));
        assert!(!event_text.contains("\"reason\":\"repair_target_not_followed\""));
    }

    #[test]
    fn uat_0702_source_scan_gameplay_repair_editing_page_is_target_matched() {
        let dir = tempfile::tempdir().unwrap();
        let contract = dir.path().join("contract.json");
        std::fs::write(
            &contract,
            r#"{"required_paths":["src/app/page.tsx"],"required_evidence":["challenge_or_adversary_evidence","failure_or_collision_evidence","restart_or_recoverable_state_evidence"],"verify_repair_cap":2}"#,
        )
        .unwrap();
        let events = dir.path().join("events.jsonl");
        let mut cfg = config(dir.path().to_path_buf());
        cfg.completion_contract_path = Some(contract);
        cfg.eval_events_path = Some(events.clone());
        let partial_page = r#""use client";
import { useState } from "react";
export default function Page(){
  const [score,setScore] = useState(0);
  return <main><button onClick={() => setScore(score + 1)}>Start</button><p>score {score}</p></main>;
}
"#;
        let repaired_page = r#""use client";
import { useState } from "react";
export default function Page(){
  const [gameOver,setGameOver] = useState(false);
  const [score,setScore] = useState(0);
  const [bullets,setBullets] = useState([{ x: 0, y: 0 }]);
  const [enemies,setEnemies] = useState([{ x: 1, y: 1 }]);
  const fire = () => {
    setBullets((items) => [...items, { x: 1, y: 1 }]);
    bullets.forEach((bullet) => {
      enemies.forEach((enemy) => {
        if (Math.abs(bullet.x - enemy.x) < 10 && Math.abs(bullet.y - enemy.y) < 10) {
          setGameOver(true);
          setScore((value) => value + 10);
        }
      });
    });
  };
  const restart = () => {
    setGameOver(false);
    setScore(0);
    setBullets([]);
    setEnemies([{ x: 1, y: 1 }]);
  };
  return <main><button onClick={fire}>Start</button><button onClick={restart}>Restart</button><button onClick={fire}>Fire</button><p>enemy collision score {score} {gameOver ? "game over" : "playing"}</p></main>;
}
"#;
        let mut fake = Fake::new(vec![
            Ok(AssistantReply {
                content: String::new(),
                tool_calls: vec![ToolCall::new(
                    "Write",
                    json!({"path":"src/app/page.tsx","content":partial_page}),
                )],
                prompt_tokens: None,
                completion_tokens: None,
            }),
            Ok(AssistantReply {
                content: String::new(),
                tool_calls: vec![ToolCall::new(
                    "Write",
                    json!({"path":"src/app/page.tsx","content":repaired_page}),
                )],
                prompt_tokens: None,
                completion_tokens: None,
            }),
        ]);
        let mut session = SessionSnapshot::new();
        let outcome = run_session_with_outcome_with_ui(
            &mut fake,
            &mut session,
            "create a game page",
            &[],
            &cfg,
            &NOOP_UI,
        )
        .unwrap();
        assert_eq!(
            outcome.stop_reason,
            RunStopReason::CompletionContractSatisfied
        );
        let event_text = std::fs::read_to_string(events).unwrap();
        assert!(event_text.contains("\"repair_target\":\"implementation\""));
        assert!(event_text.contains("\"repair_follow_through\":\"target_matched\""));
        assert!(!event_text.contains("repair_target_not_followed"));
    }

    #[test]
    fn no_progress_feedback_includes_declared_profile_verification() {
        let dir = tempfile::tempdir().unwrap();
        let events = dir.path().join("events.jsonl");
        let mut cfg = config(dir.path().to_path_buf());
        cfg.eval_events_path = Some(events.clone());
        let mut fake = Fake::new(vec![
            Ok(AssistantReply::text("I will verify it.")),
            Ok(AssistantReply::text("I will run the check now.")),
            Ok(AssistantReply::text("I will run it next.")),
            Ok(AssistantReply::text("Verification is complete.")),
        ]);
        let mut session = SessionSnapshot::new();
        let prompt = "Execute exactly one StepPlan step.\n\nCurrent step id:\nsetup-scripts\n\nCurrent step instruction:\nConfirm scripts.\n\nVerification commands for this step:\n- node -p \"strict port check\"\n\nExpected verification result:\npass";

        let outcome = run_session_with_outcome_with_options(
            &mut fake,
            &mut session,
            prompt,
            &[],
            &cfg,
            &NOOP_UI,
            RunSessionOptions::plan_step(RunSessionStepKind::Setup),
        )
        .unwrap();

        assert_eq!(outcome.final_text, "Verification is complete.");
        let event_text = std::fs::read_to_string(events).unwrap();
        assert!(event_text.contains("\"event\":\"no_progress_feedback\""));
        assert!(event_text.contains(r#"node -p \"strict port check\""#));
        assert!(event_text.contains("already satisfied"));
    }
}
