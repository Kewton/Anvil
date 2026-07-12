#[cfg(test)]
mod moved {
    use super::super::*;

    #[test]
    fn ultra_run_setup_authority_falls_back_for_later_verify_and_contract_steps() {
        let plan = StepPlan {
            goal: "verify promoted app".to_string(),
            steps: vec![
                PlanStep {
                    id: "implement".to_string(),
                    kind: "implement".to_string(),
                    expected_result: "pass".to_string(),
                    instruction: "Implement the app".to_string(),
                    expected_paths: Vec::new(),
                    verify: Vec::new(),
                },
                PlanStep {
                    id: "verify".to_string(),
                    kind: "verify".to_string(),
                    expected_result: "pass".to_string(),
                    instruction: "Run build".to_string(),
                    expected_paths: Vec::new(),
                    verify: vec!["npm run build".to_string()],
                },
            ],
        };
        let implement = &plan.steps[0];
        let verify = &plan.steps[1];

        assert_eq!(
            step_verify_setup_authority(&plan, verify, NodeDependencySetupAuthority::None),
            NodeDependencySetupAuthority::None
        );
        assert_eq!(
            step_verify_setup_authority(&plan, verify, NodeDependencySetupAuthority::PlanSetupStep),
            NodeDependencySetupAuthority::PlanSetupStep
        );
        assert_eq!(
            step_contract_setup_authority(
                &plan,
                implement,
                None,
                NodeDependencySetupAuthority::PlanSetupStep
            ),
            NodeDependencySetupAuthority::PlanSetupStep
        );
    }

    #[test]
    fn ultra_phase_prompt_prefix_covers_profile_contract_before_phase_fields() {
        let dir = tempfile::tempdir().unwrap();
        let plan = UltraPlan {
            goal: "Create an interactive browser game on port 3011".to_string(),
            profile: "nextjs".to_string(),
            style: "default".to_string(),
            intent: "create".to_string(),
            phases: vec![
                crate::planner::ultra_plan::UltraPhase {
                    id: "setup".to_string(),
                    prompt: "Set up the app shell".to_string(),
                },
                crate::planner::ultra_plan::UltraPhase {
                    id: "gameplay".to_string(),
                    prompt: "Implement keyboard gameplay".to_string(),
                },
            ],
        };
        let context = UltraRunContext::new(vec!["src/app/page.tsx".to_string()]);
        let first = ultra_phase_prompt(
            &plan,
            &plan.phases[0],
            &config(dir.path().to_path_buf()),
            &context,
        );
        let second = ultra_phase_prompt(
            &plan,
            &plan.phases[1],
            &config(dir.path().to_path_buf()),
            &context,
        );

        let prefix = common_prefix(&first, &second);

        assert!(prefix.contains("Profile generation rules:"), "{prefix}");
        assert!(prefix.contains("Profile runtime contract:"), "{prefix}");
        assert!(
            prefix.contains("Deterministic verification preference:"),
            "{prefix}"
        );
        assert!(prefix.contains("Required final artifacts:"), "{prefix}");
        assert!(prefix.ends_with("Phase id: "), "{prefix}");
    }

    #[test]
    fn preset_nextjs_implementation_phase_converts_duplicate_setup_step() {
        let dir = tempfile::tempdir().unwrap();
        write_nextjs_profile_workspace(dir.path(), None, None, None);
        let events = dir.path().join("events.jsonl");
        let mut cfg = config(dir.path().to_path_buf());
        cfg.profile = "nextjs".to_string();
        cfg.eval_events_path = Some(events.clone());
        let goal = "Original ultra goal: Build a browser game on port 3011\n\
Profile: nextjs\n\
Intent: create\n\
Phase id: core-implementation\n\
Phase task: Implement game logic, player control, collision, score, and canvas behavior";
        let plan_json = serde_json::to_string(&StepPlan {
            goal: "Implement gameplay".to_string(),
            steps: vec![
                PlanStep {
                    id: "setup-nextjs".to_string(),
                    kind: "setup".to_string(),
                    expected_result: "pass".to_string(),
                    instruction:
                        "Ensure package.json and src/app/page.tsx exist before build verification."
                            .to_string(),
                    expected_paths: vec![
                        "package.json".to_string(),
                        "src/app/page.tsx".to_string(),
                    ],
                    verify: Vec::new(),
                },
                PlanStep {
                    id: "implement-gameplay".to_string(),
                    kind: "implement".to_string(),
                    expected_result: "pass".to_string(),
                    instruction: "Implement the route-bound gameplay surface.".to_string(),
                    expected_paths: vec!["src/app/page.tsx".to_string()],
                    verify: Vec::new(),
                },
                PlanStep {
                    id: "verify-build".to_string(),
                    kind: "verify".to_string(),
                    expected_result: "pass".to_string(),
                    instruction: "Verify the Next.js build.".to_string(),
                    expected_paths: Vec::new(),
                    verify: vec!["npm run build".to_string()],
                },
            ],
        })
        .unwrap();
        let mut planner = FakeClient::new(vec![AssistantReply::text(plan_json)]);

        let plan = generate_step_plan_with_ui_for_phase(
            &mut planner,
            goal,
            &cfg,
            &NOOP_UI,
            Some("core-implementation"),
            true,
        )
        .unwrap();

        assert_eq!(planner.messages().len(), 1);
        assert_eq!(plan.steps[0].id, "setup-nextjs");
        assert_eq!(plan.steps[0].kind, "verify");
        assert!(setup_step_policy::step_short_circuit_precheck_applicable(
            &plan.steps[0]
        ));
        let event_text = std::fs::read_to_string(events).unwrap();
        assert!(!event_text.contains("\"event\":\"deterministic_step_plan_used\""));
        assert!(event_text.contains("\"event\":\"planner_raw_output_shape\""));
        assert!(event_text.contains("\"event\":\"preset_step_converted\""));
    }

    #[test]
    fn preset_nextjs_implementation_phase_converts_implement_port_step_only() {
        let dir = tempfile::tempdir().unwrap();
        write_nextjs_profile_workspace(dir.path(), None, None, None);
        let events = dir.path().join("events.jsonl");
        let mut cfg = config(dir.path().to_path_buf());
        cfg.profile = "nextjs".to_string();
        cfg.eval_events_path = Some(events.clone());
        let goal = "Original ultra goal: Build Breakout on port 3011\n\
Profile: nextjs\n\
Intent: create\n\
Phase id: core-implementation\n\
Phase task: Implement paddle movement, collisions, score, and canvas behavior";
        let plan_json = serde_json::to_string(&StepPlan {
            goal: "Implement Breakout".to_string(),
            steps: vec![
                PlanStep {
                    id: "ensure-port-scripts".to_string(),
                    kind: "implement".to_string(),
                    expected_result: "pass".to_string(),
                    instruction: "Update package.json scripts to use port 3011.".to_string(),
                    expected_paths: Vec::new(),
                    verify: Vec::new(),
                },
                PlanStep {
                    id: "implement-gameplay".to_string(),
                    kind: "implement".to_string(),
                    expected_result: "pass".to_string(),
                    instruction: "Implement paddle movement, collision, and score behavior."
                        .to_string(),
                    expected_paths: vec!["src/app/page.tsx".to_string()],
                    verify: vec!["npm run build".to_string()],
                },
            ],
        })
        .unwrap();
        let mut planner = FakeClient::new(vec![AssistantReply::text(plan_json)]);

        let plan = generate_step_plan_with_ui_for_phase(
            &mut planner,
            goal,
            &cfg,
            &NOOP_UI,
            Some("core-implementation"),
            true,
        )
        .unwrap();

        assert_eq!(plan.steps[0].id, "ensure-port-scripts");
        assert_eq!(plan.steps[0].kind, "verify");
        assert_eq!(plan.steps[1].id, "implement-gameplay");
        assert_eq!(plan.steps[1].kind, "implement");
        let event_text = std::fs::read_to_string(events).unwrap();
        assert!(event_text.contains("\"event\":\"preset_step_converted\""));
        assert!(event_text.contains("\"step_id\":\"ensure-port-scripts\""));
        assert!(!event_text.contains("\"step_id\":\"implement-gameplay\""));
    }

    #[test]
    fn implement_port_step_with_missing_port_runs_executor_instead_of_short_circuiting() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("node_modules/.bin")).unwrap();
        std::fs::create_dir_all(dir.path().join("node_modules/next")).unwrap();
        std::fs::write(dir.path().join("node_modules/.bin/next"), "").unwrap();
        std::fs::write(
            dir.path().join("package.json"),
            r#"{"scripts":{"dev":"next dev","start":"next start","build":"next build"}}"#,
        )
        .unwrap();
        let events = dir.path().join("events.jsonl");
        let mut cfg = config(dir.path().to_path_buf());
        cfg.profile = "nextjs".to_string();
        cfg.offline = true;
        cfg.eval_events_path = Some(events.clone());
        let step = PlanStep {
            id: "ensure-port-scripts".to_string(),
            kind: "implement".to_string(),
            expected_result: "pass".to_string(),
            instruction: "Update package.json scripts to use port 3011.".to_string(),
            expected_paths: Vec::new(),
            verify: Vec::new(),
        };
        let plan = StepPlan {
            goal: "Ensure the Next.js port scripts".to_string(),
            steps: vec![step.clone()],
        };
        let mut fake = FakeClient::new(vec![AssistantReply {
            content: String::new(),
            tool_calls: vec![crate::state::ToolCall::new(
                "Write",
                serde_json::json!({
                    "path": "package.json",
                    "content": "{\"scripts\":{\"dev\":\"next dev -p 3011\",\"start\":\"next start -p 3011\",\"build\":\"next build\"}}"
                }),
            )],
            prompt_tokens: None,
            completion_tokens: None,
        }]);
        let mut session = SessionSnapshot::new();
        let context = StepPromptContext {
            overall_goal: "Build a Next.js app on port 3011".to_string(),
            ..StepPromptContext::default()
        };

        let outcome = run_step(
            &mut fake,
            &mut session,
            &plan,
            &step,
            &context,
            &cfg,
            &NOOP_UI,
            "test",
            ContractEnforcement::Enforce,
            Some("core-implementation"),
            None,
        )
        .unwrap();

        assert_ne!(outcome.stop_reason.as_deref(), Some("StepShortCircuited"));
        assert!(!fake.messages().is_empty());
        let package = std::fs::read_to_string(dir.path().join("package.json")).unwrap();
        assert!(package.contains("next dev -p 3011"), "{package}");
        let event_text = std::fs::read_to_string(events).unwrap();
        let short_circuited_at_start = event_text.lines().any(|line| {
            serde_json::from_str::<serde_json::Value>(line).is_ok_and(|event| {
                event.get("event").and_then(serde_json::Value::as_str)
                    == Some("step_short_circuited")
                    && event.get("at").and_then(serde_json::Value::as_str) == Some("start")
                    && event.get("step_id").and_then(serde_json::Value::as_str)
                        == Some("ensure-port-scripts")
            })
        });
        assert!(!short_circuited_at_start, "{event_text}");
    }

    #[test]
    fn ultra_phase_step_prompt_requires_short_goal() {
        let prompt = build_step_plan_user_prompt(
            "Original ultra goal: Build game\nPhase id: ui\nPhase task: Build the arcade UI.",
            &config(PathBuf::from("/tmp/work")),
        );

        assert!(prompt.contains("StepPlan.goal must be ONE short sentence"));
        assert!(prompt.contains("never copy the phase context"));
        assert!(prompt.contains("details belong in step instructions"));
    }

    #[test]
    fn ultra_plan_prompt_includes_source_parity_rules() {
        let mut cfg = config(PathBuf::from("/tmp/work"));
        cfg.profile = "nextjs".to_string();
        let messages = ultra_plan_generation_messages("Build a Next.js app on port 3011", &cfg);
        assert_eq!(messages.len(), 2);
        assert_eq!(messages[0].role, "system");
        assert!(messages[0].content.contains("You do not execute tools"));
        assert!(messages[0].content.contains("Output YAML only"));
        assert!(messages[0].content.contains("phases:"));
        assert!(messages[0].content.contains("next/react/react-dom"));
        assert!(
            messages[0]
                .content
                .contains("dependency setup before any npm run build")
        );
        assert!(messages[0].content.contains("Tailwind"));
        assert!(
            messages[0]
                .content
                .contains("default Tailwind scaffold coherently, or plain CSS coherently")
        );
        assert_eq!(messages[1].role, "user");
        assert!(
            messages[1]
                .content
                .contains("Build a Next.js app on port 3011")
        );
    }

    #[test]
    fn ultra_plan_prompt_does_not_bake_in_game_scenario_terms() {
        let mut cfg = config(PathBuf::from("/tmp/work"));
        cfg.profile = "nextjs".to_string();
        let system =
            ultra_plan_generation_messages("Build a Space Invaders game on port 3011", &cfg)
                .remove(0)
                .content;
        for term in ["Space Invaders", "enemy", "bullet", "collision", "score"] {
            assert!(!system.contains(term), "{term}: {system}");
        }
    }

    #[test]
    fn ultra_plan_generation_retries_invalid_output() {
        let dir = tempfile::tempdir().unwrap();
        let mut planner = FakeClient::new(vec![
            AssistantReply::text("not yaml"),
            AssistantReply::text(generated_ultra_plan_yaml("goal")),
        ]);
        let plan =
            generate_ultra_plan(&mut planner, "goal", &config(dir.path().to_path_buf())).unwrap();
        assert_eq!(planner.messages().len(), 2);
        assert_eq!(plan.goal, "goal");
        assert_eq!(plan.phases.len(), 2);
    }

    #[test]
    fn ultra_plan_generation_retries_transient_provider_request_error() {
        let dir = tempfile::tempdir().unwrap();
        let mut planner = FlakyClient::new(
            1,
            "transient provider unavailable",
            vec![AssistantReply::text(generated_ultra_plan_yaml("goal"))],
        );

        let plan =
            generate_ultra_plan(&mut planner, "goal", &config(dir.path().to_path_buf())).unwrap();

        assert_eq!(planner.messages().len(), 2);
        assert_eq!(plan.goal, "goal");
        assert_eq!(plan.phases.len(), 2);
    }

    #[test]
    fn ultra_plan_generation_rejects_tool_calls() {
        let dir = tempfile::tempdir().unwrap();
        let mut planner = FakeClient::new(vec![
            AssistantReply {
                content: String::new(),
                tool_calls: vec![crate::state::ToolCall::new(
                    "Write",
                    serde_json::json!({"path":"x","content":"x"}),
                )],
                prompt_tokens: None,
                completion_tokens: None,
            },
            AssistantReply::text(generated_ultra_plan_yaml("goal")),
        ]);
        let plan =
            generate_ultra_plan(&mut planner, "goal", &config(dir.path().to_path_buf())).unwrap();
        assert_eq!(planner.messages().len(), 2);
        assert_eq!(plan.phases[0].id, "scaffold");
    }

    #[test]
    fn ultra_plan_generation_normalizes_metadata() {
        let dir = tempfile::tempdir().unwrap();
        let events = dir.path().join("events.jsonl");
        let mut cfg = config(dir.path().to_path_buf());
        cfg.profile = "nextjs".to_string();
        cfg.style = "default".to_string();
        cfg.eval_events_path = Some(events.clone());
        let plan = UltraPlan {
            goal: "different goal".to_string(),
            profile: "generic".to_string(),
            style: "tdd".to_string(),
            intent: "fix".to_string(),
            phases: vec![
                crate::planner::ultra_plan::UltraPhase {
                    id: "scaffold".to_string(),
                    prompt: "Create the Next.js package and app entrypoint, then verify the files exist.".to_string(),
                },
                crate::planner::ultra_plan::UltraPhase {
                    id: "verify".to_string(),
                    prompt: "Run deterministic Next.js build verification and repair failures.".to_string(),
                },
            ],
        };
        let mut planner = FakeClient::new(vec![AssistantReply::text(render_ultra_plan(&plan))]);
        let generated = generate_ultra_plan(&mut planner, "Build app", &cfg).unwrap();
        assert_eq!(generated.goal, "Build app");
        assert_eq!(generated.profile, "nextjs");
        assert_eq!(generated.style, "default");
        assert_eq!(generated.intent, "create");
        let event_text = std::fs::read_to_string(events).unwrap();
        assert!(event_text.contains("ultra_plan_generation_metadata_normalized"));
    }

    #[test]
    fn invalid_ultra_plan_generation_does_not_save_plan_file() {
        let dir = tempfile::tempdir().unwrap();
        let events = dir.path().join("events.jsonl");
        let mut cfg = config(dir.path().to_path_buf());
        cfg.eval_events_path = Some(events.clone());
        let mut planner = FakeClient::new(vec![
            AssistantReply::text("not yaml"),
            AssistantReply::text("still not yaml"),
            AssistantReply::text("nope"),
        ]);
        let mut execution = FakeClient::new(vec![]);
        let err = generate_and_run_ultra_plan(&mut planner, &mut execution, "goal", &cfg)
            .unwrap_err()
            .to_string();
        assert!(err.contains("invalid generated UltraPlan after corrective retries"));
        assert!(!dir.path().join(".anvil/plans").exists());
        let event_text = std::fs::read_to_string(events).unwrap();
        assert!(event_text.contains("\"event\":\"ultra_plan_generation_failed\""));
        assert!(event_text.contains("\"planner_error_kind\":\"planner_schema_error\""));
        assert!(!event_text.contains("\"event\":\"ultra_plan_generation_succeeded\""));
    }

    #[test]
    fn echoed_ultra_phase_goal_is_sanitized_without_retry() {
        let dir = tempfile::tempdir().unwrap();
        let events = dir.path().join("events.jsonl");
        let mut cfg = config(dir.path().to_path_buf());
        cfg.eval_events_path = Some(events.clone());
        let long_phase_prompt = format!(
            "Original ultra goal: {}\n\
Profile: nextjs\n\
Style: default\n\
Intent: create\n\
Phase id: arcade-ui-and-local-storage\n\
Phase task: Build the arcade UI and local storage persistence. Add details in the steps.\n\n\
Unmet final requirements from earlier phases:\n- restart_or_recoverable_state_evidence\n\n\
Requested features not yet detected: keyboard, score, wave, audio\n\n\
Profile runtime contract:\n- Preserve the workspace as a real Next.js app.\n\n{}",
            "Create a polished arcade game. ".repeat(120),
            "Carry forward profile constraints. ".repeat(120)
        );
        let step_json = serde_json::to_string(&StepPlan {
            goal: long_phase_prompt.clone(),
            steps: vec![PlanStep {
                id: "create-page".to_string(),
                kind: "implement".to_string(),
                expected_result: "pass".to_string(),
                instruction: "Create src/app/page.tsx for the arcade phase.".to_string(),
                expected_paths: vec!["src/app/page.tsx".to_string()],
                verify: Vec::new(),
            }],
        })
        .unwrap();
        let mut planner = FakeClient::new(vec![AssistantReply::text(step_json)]);

        let plan = generate_step_plan(&mut planner, &long_phase_prompt, &cfg).unwrap();

        assert_eq!(planner.messages().len(), 1);
        assert_eq!(
            plan.goal,
            "Build the arcade UI and local storage persistence."
        );
        assert!(!plan.goal.contains("Unmet final requirements"));
        assert!(!plan.goal.contains("Requested features"));
        let event_text = std::fs::read_to_string(events).unwrap();
        assert!(event_text.contains("\"event\":\"planner_plan_sanitized\""));
        assert!(event_text.contains("\"kind\":\"goal_truncated\""));
        assert!(event_text.contains("\"original_len\""));
        assert!(event_text.contains("\"new_len\""));
    }

    #[test]
    fn required_final_artifacts_are_preserved_in_ultra_phase_prompt() {
        let dir = tempfile::tempdir().unwrap();
        let plan = UltraPlan {
            goal: "3011 port app".to_string(),
            profile: "nextjs".to_string(),
            style: "default".to_string(),
            intent: "create".to_string(),
            phases: vec![
                crate::planner::ultra_plan::UltraPhase {
                    id: "scaffold".to_string(),
                    prompt: "Scaffold project".to_string(),
                },
                crate::planner::ultra_plan::UltraPhase {
                    id: "finish".to_string(),
                    prompt: "Finish project".to_string(),
                },
            ],
        };
        let prompt = ultra_phase_prompt(
            &plan,
            &crate::planner::ultra_plan::UltraPhase {
                id: "finish".to_string(),
                prompt: "Finish project".to_string(),
            },
            &config(dir.path().to_path_buf()),
            &UltraRunContext::new(vec!["src/app/page.tsx".to_string()]),
        );
        assert!(prompt.contains("Original ultra goal: 3011 port app"));
        assert!(prompt.contains("Profile: nextjs"));
        assert!(prompt.contains("Phase id: finish"));
        assert!(prompt.contains("Workspace snapshot:"));
        assert!(prompt.contains("Prior ultra context:"));
        assert!(prompt.contains("Pending final artifacts:"));
        assert!(prompt.contains("Profile runtime contract:"));
        assert!(prompt.contains("Keep next/react/react-dom dependencies"));
        assert!(prompt.contains("Do not treat scaffold-only"));
        assert!(prompt.contains("Deterministic verification preference:"));
        assert!(prompt.contains("Required final artifacts:"));
        assert!(prompt.contains("- package.json"));
        assert!(prompt.contains("- src/app/page.tsx"));
    }

    #[test]
    fn ultra_phase_prompt_renders_missing_plan_adherence_guidance() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("src/app")).unwrap();
        std::fs::write(
            dir.path().join("src/app/page.tsx"),
            "// keyboard lives pause\nexport default function Page(){ return <main><p>score</p></main>; }",
        )
        .unwrap();
        let plan = UltraPlan {
            goal: "Create an interactive browser game".to_string(),
            profile: "generic".to_string(),
            style: "default".to_string(),
            intent: "create".to_string(),
            phases: vec![crate::planner::ultra_plan::UltraPhase {
                id: "gameplay".to_string(),
                prompt:
                    "Implement keyboard controls, lives counter, pause overlay, and score display"
                        .to_string(),
            }],
        };

        let prompt = ultra_phase_prompt(
            &plan,
            &plan.phases[0],
            &config(dir.path().to_path_buf()),
            &UltraRunContext::new(Vec::new()),
        );

        let section = prompt_section_lines(&prompt, "Requested features not yet detected:");
        let section_text = section.join("\n");
        assert!(section_text.contains("- keyboard"), "{section_text}");
        assert!(section_text.contains("- lives"), "{section_text}");
        assert!(section_text.contains("- pause"), "{section_text}");
        assert!(!section_text.contains("score"), "{section_text}");
    }

    #[test]
    fn ultra_phase_prompt_elides_unmet_requirements_and_adherence_sections() {
        let dir = tempfile::tempdir().unwrap();
        let mut context = UltraRunContext::new(Vec::new());
        context.pending_capability_evidence = (0..20)
            .map(|index| format!("pending_evidence_{index:02}"))
            .collect();
        let feature_tokens = (0..30)
            .map(|index| format!("needtoken{index:02}"))
            .collect::<Vec<_>>()
            .join(" ");
        let plan = UltraPlan {
            goal: "Create a browser game".to_string(),
            profile: "generic".to_string(),
            style: "default".to_string(),
            intent: "create".to_string(),
            phases: vec![crate::planner::ultra_plan::UltraPhase {
                id: "features".to_string(),
                prompt: feature_tokens,
            }],
        };

        let prompt = ultra_phase_prompt(
            &plan,
            &plan.phases[0],
            &config(dir.path().to_path_buf()),
            &context,
        );

        let unmet = prompt_section_lines(&prompt, "Unmet final requirements from earlier phases:");
        let adherence = prompt_section_lines(&prompt, "Requested features not yet detected:");
        assert!(unmet.len() <= ULTRA_PROMPT_GUIDANCE_MAX_LINES, "{unmet:#?}");
        assert!(
            adherence.len() <= ULTRA_PROMPT_GUIDANCE_MAX_LINES,
            "{adherence:#?}"
        );
        assert!(unmet.iter().any(|line| line.contains("… and ")));
        assert!(adherence.iter().any(|line| line.contains("… and 24 more")));
    }

    #[test]
    fn ultra_phase_prompt_derives_interactive_capability_evidence_from_goal_and_phase() {
        let dir = tempfile::tempdir().unwrap();
        let plan = UltraPlan {
            goal: "Create an interactive browser game".to_string(),
            profile: "nextjs".to_string(),
            style: "default".to_string(),
            intent: "create".to_string(),
            phases: vec![crate::planner::ultra_plan::UltraPhase {
                id: "gameplay".to_string(),
                prompt:
                    "Implement keyboard controls, score progression, collision rules, and restart state."
                        .to_string(),
            }],
        };
        let prompt = ultra_phase_prompt(
            &plan,
            &plan.phases[0],
            &config(dir.path().to_path_buf()),
            &UltraRunContext::new(vec!["src/app/page.tsx".to_string()]),
        );
        assert!(prompt.contains("Required final capabilities:"));
        assert!(prompt.contains("- stateful_interaction"));
        assert!(prompt.contains("- start_or_restart_flow"));
        assert!(prompt.contains("- player_control"));
        assert!(prompt.contains("Required final evidence:"));
        assert!(prompt.contains("- visible_interactive_surface_evidence"));
        assert!(prompt.contains("- user_input_handler_evidence"));
        assert!(prompt.contains("- stateful_update_evidence"));
        assert!(prompt.contains("- score_or_progression_evidence"));
        assert!(prompt.contains("- failure_or_collision_evidence"));
        assert!(prompt.contains("- restart_or_recoverable_state_evidence"));
    }

    #[test]
    fn generic_ultra_promotes_to_nextjs_after_workspace_manifest() {
        let dir = tempfile::tempdir().unwrap();
        let events = dir.path().join("events.jsonl");
        let mut cfg = config(dir.path().to_path_buf());
        cfg.eval_events_path = Some(events.clone());
        write_fake_npm_dependency_installer(dir.path());
        let goal = "Build an interactive browser game on port 3011";
        let plan = two_phase_ultra_plan(goal, "generic");
        let mut planner = FakeClient::new(vec![
            AssistantReply::text(single_write_step_plan_json(
                "Create a package manifest",
                "package.json",
            )),
            AssistantReply::text(generated_nextjs_artifact_plan_json(
                "Complete the promoted Next.js app",
            )),
        ]);
        let mut final_calls = nextjs_interactive_app_tool_calls(interactive_game_page_source());
        final_calls.remove(0);
        final_calls.extend(browser_release_evidence_tool_calls());
        let mut execution = FakeClient::new(vec![
            AssistantReply {
                content: String::new(),
                tool_calls: vec![crate::state::ToolCall::new(
                    "Write",
                    serde_json::json!({
                        "path": "package.json",
                        "content": nextjs_complete_package_json()
                    }),
                )],
                prompt_tokens: None,
                completion_tokens: None,
            },
            AssistantReply {
                content: String::new(),
                tool_calls: final_calls,
                prompt_tokens: None,
                completion_tokens: None,
            },
        ]);

        let result = run_ultra_plan(&mut planner, &mut execution, &plan, &cfg).unwrap();

        assert_eq!(result, "ultra-plan-run complete: 2 phases");
        let promotion = latest_event(&events, "profile_reinferred");
        assert_eq!(promotion.get("id").and_then(Value::as_str), Some("nextjs"));
        assert_eq!(promotion.get("at_phase").and_then(Value::as_u64), Some(1));
        assert_eq!(
            promotion.get("from").and_then(Value::as_str),
            Some("workspace")
        );
        assert!(event_array_contains(
            &promotion,
            "delta_capabilities",
            "stateful_interaction"
        ));
        let phase_two_prompt = planner_request_text(&planner, 1);
        assert!(phase_two_prompt.contains("Profile: nextjs"));
        assert!(phase_two_prompt.contains("Profile generation rules:"));
        assert!(phase_two_prompt.contains("data-anvil-action=\"primary\""));
        assert!(phase_two_prompt.contains("data-anvil-state"));
        assert!(phase_two_prompt.contains("Unmet final requirements from earlier phases:"));
        assert!(phase_two_prompt.contains("- nextjs_route_evidence"));
        let final_acceptance = latest_event(&events, "ultra_final_acceptance");
        assert_eq!(
            final_acceptance.get("profile").and_then(Value::as_str),
            Some("nextjs")
        );
        assert_eq!(
            final_acceptance
                .get("assurance_level")
                .and_then(Value::as_str),
            Some("full")
        );
        assert_eq!(
            final_acceptance
                .get("browser_readiness_status")
                .and_then(Value::as_str),
            Some("passed")
        );
        assert_eq!(
            final_acceptance
                .get("interaction_evidence_status")
                .and_then(Value::as_str),
            Some("passed")
        );
        assert_eq!(
            final_acceptance
                .get("release_gate_status")
                .and_then(Value::as_str),
            Some("pass")
        );
        let depth = latest_event(&events, "depth_profile");
        assert_eq!(
            depth.get("source_event").and_then(Value::as_str),
            Some("ultra_final_acceptance")
        );
        assert!(
            depth
                .get("route_bound_source_line_count")
                .and_then(Value::as_u64)
                .is_some_and(|count| count > 0),
            "{depth}"
        );
        assert!(
            depth
                .get("data_anvil_action_kind_count")
                .and_then(Value::as_u64)
                .is_some(),
            "{depth}"
        );
        assert!(
            depth
                .get("depth_profile_summary")
                .and_then(Value::as_str)
                .is_some_and(|summary| summary.contains("route_bound_source_lines=")
                    && summary.contains("data_anvil_action_kinds=")),
            "{depth}"
        );
        let complete = latest_event(&events, "ultra_plan_complete");
        assert_eq!(
            complete.get("profile").and_then(Value::as_str),
            Some("nextjs")
        );
        assert_eq!(
            complete.get("assurance_level").and_then(Value::as_str),
            Some("full")
        );
    }

    #[test]
    fn generic_ultra_promotes_to_python_cli_after_pyproject_manifest() {
        let dir = tempfile::tempdir().unwrap();
        let events = dir.path().join("events.jsonl");
        let mut cfg = config(dir.path().to_path_buf());
        cfg.eval_events_path = Some(events.clone());
        let goal = "Build a local text transformer";
        let plan = two_phase_ultra_plan(goal, "generic");
        let mut planner = FakeClient::new(vec![
            AssistantReply::text(single_write_step_plan_json(
                "Create Python project metadata",
                "pyproject.toml",
            )),
            AssistantReply::text(single_write_step_plan_json(
                "Implement the promoted Python package",
                "src/text_tool/main.py",
            )),
        ]);
        let pyproject = r#"[project]
name = "text-tool"
version = "0.1.0"
"#;
        let main_py = r#"#!/usr/bin/env python3
import sys

def main() -> None:
    text = sys.stdin.read().strip()
    print(f"transformed:{text.upper()}:{len(text)}")

if __name__ == "__main__":
    main()
"#;
        let mut execution = FakeClient::new(vec![
            AssistantReply {
                content: String::new(),
                tool_calls: vec![crate::state::ToolCall::new(
                    "Write",
                    serde_json::json!({"path":"pyproject.toml","content":pyproject}),
                )],
                prompt_tokens: None,
                completion_tokens: None,
            },
            AssistantReply {
                content: String::new(),
                tool_calls: vec![crate::state::ToolCall::new(
                    "Write",
                    serde_json::json!({"path":"src/text_tool/main.py","content":main_py}),
                )],
                prompt_tokens: None,
                completion_tokens: None,
            },
        ]);

        let result = run_ultra_plan(&mut planner, &mut execution, &plan, &cfg).unwrap();

        assert_eq!(result, "ultra-plan-run complete: 2 phases");
        let promotion = latest_event(&events, "profile_reinferred");
        assert_eq!(
            promotion.get("id").and_then(Value::as_str),
            Some("python-cli")
        );
        let phase_two_prompt = planner_request_text(&planner, 1);
        assert!(phase_two_prompt.contains("Profile: python-cli"));
        assert!(phase_two_prompt.contains("python -m compileall -q src"));
        let final_acceptance = latest_event(&events, "ultra_final_acceptance");
        assert_eq!(
            final_acceptance.get("profile").and_then(Value::as_str),
            Some("python-cli")
        );
        assert_eq!(
            final_acceptance
                .get("assurance_level")
                .and_then(Value::as_str),
            Some("full")
        );
        assert_eq!(
            final_acceptance
                .get("profile_behavior_probe_status")
                .and_then(Value::as_str),
            Some("pass")
        );
    }

    #[test]
    fn generic_ultra_without_manifest_keeps_static_tier() {
        let dir = tempfile::tempdir().unwrap();
        let events = dir.path().join("events.jsonl");
        let mut cfg = config(dir.path().to_path_buf());
        cfg.eval_events_path = Some(events.clone());
        let goal = "Build an interactive memo app with add and delete actions";
        let plan = two_phase_ultra_plan(goal, "generic");
        let mut planner = FakeClient::new(vec![
            AssistantReply::text(single_write_step_plan_json("Write notes", "notes.txt")),
            AssistantReply::text(single_write_step_plan_json(
                "Create generic app source",
                "memo.jsx",
            )),
        ]);
        let mut execution = FakeClient::new(vec![
            AssistantReply {
                content: String::new(),
                tool_calls: vec![crate::state::ToolCall::new(
                    "Write",
                    serde_json::json!({"path":"notes.txt","content":"scaffold notes"}),
                )],
                prompt_tokens: None,
                completion_tokens: None,
            },
            AssistantReply {
                content: String::new(),
                tool_calls: vec![crate::state::ToolCall::new(
                    "Write",
                    serde_json::json!({"path":"memo.jsx","content":generic_interactive_source()}),
                )],
                prompt_tokens: None,
                completion_tokens: None,
            },
        ]);

        let result = run_ultra_plan(&mut planner, &mut execution, &plan, &cfg).unwrap();

        assert_eq!(result, "ultra-plan-run complete: 2 phases");
        let event_text = std::fs::read_to_string(&events).unwrap();
        assert!(!event_text.contains("\"event\":\"profile_reinferred\""));
        let final_acceptance = latest_event(&events, "ultra_final_acceptance");
        assert_eq!(
            final_acceptance.get("profile").and_then(Value::as_str),
            Some("generic")
        );
        assert_eq!(
            final_acceptance
                .get("assurance_level")
                .and_then(Value::as_str),
            Some("static")
        );
    }

    #[test]
    #[cfg(unix)]
    fn ultra_run_level_authority_installs_missing_dependencies_without_current_setup_step() {
        let dir = tempfile::tempdir().unwrap();
        let events = dir.path().join("events.jsonl");
        let mut cfg = config(dir.path().to_path_buf());
        cfg.profile = "nextjs".to_string();
        cfg.eval_events_path = Some(events.clone());
        write_nextjs_profile_workspace(
            dir.path(),
            Some(nextjs_globals_css()),
            Some(nextjs_postcss_config()),
            Some(nextjs_tsconfig_json()),
        );
        crate::planner::profiles::nextjs::repair_manifest_coherence(
            dir.path(),
            "Verify promoted Next.js app",
        )
        .unwrap();
        write_fake_npm_dependency_installer(dir.path());
        let plan = StepPlan {
            goal: "Verify promoted Next.js app".to_string(),
            steps: vec![PlanStep {
                id: "later-build".to_string(),
                kind: "implement".to_string(),
                expected_result: "pass".to_string(),
                instruction: "Verify the existing promoted app build".to_string(),
                expected_paths: vec!["src/app/page.tsx".to_string()],
                verify: vec!["npm run build".to_string()],
            }],
        };
        let mut run_authority = UltraRunSetupAuthorityState::default();
        run_authority.grant("profile_promotion");
        let mut session = SessionSnapshot::new();
        let mut fake = FakeClient::new(vec![
            AssistantReply {
                content: String::new(),
                tool_calls: vec![crate::state::ToolCall::new(
                    "Bash",
                    serde_json::json!({"command":"true"}),
                )],
                prompt_tokens: None,
                completion_tokens: None,
            },
            AssistantReply::text("No source changes needed."),
        ]);

        let outcome = run_step_plan_with_session_with_ui_and_run_authority(
            &mut fake,
            &mut session,
            &plan,
            &cfg,
            &NOOP_UI,
            false,
            "ultra-plan-run",
            Some("later-phase"),
            Some("Verify promoted Next.js app"),
            Some(&mut run_authority),
        )
        .unwrap();

        assert_eq!(outcome.completed_steps, 1);
        assert!(dir.path().join("node_modules/autoprefixer").is_dir());
        let build_lifecycles = events_with_name(&events, "dependency_build_lifecycle");
        assert!(
            build_lifecycles.iter().any(|event| {
                event.get("step_id").and_then(Value::as_str) == Some("later-build")
                    && event.get("setup_status").and_then(Value::as_str) == Some("passed")
                    && event.get("setup_authority").and_then(Value::as_str)
                        == Some("plan_setup_step")
                    && event.get("final_status").and_then(Value::as_str) == Some("passed")
            }),
            "{build_lifecycles:#?}"
        );
        let event_text = std::fs::read_to_string(events).unwrap();
        assert!(!event_text.contains("dependency_setup_authority_required"));
    }

    #[test]
    fn ultra_run_proceeds_when_step_planner_echoes_phase_prompt_into_goal() {
        let dir = tempfile::tempdir().unwrap();
        let events = dir.path().join("events.jsonl");
        let mut cfg = config(dir.path().to_path_buf());
        cfg.eval_events_path = Some(events.clone());
        let plan = UltraPlan {
            goal: "Create a polished arcade game with local storage. ".repeat(120),
            profile: "generic".to_string(),
            style: "default".to_string(),
            intent: "create".to_string(),
            phases: vec![
                crate::planner::ultra_plan::UltraPhase {
                    id: "arcade-ui-and-local-storage".to_string(),
                    prompt: "Build the arcade UI and local storage persistence.".to_string(),
                },
                crate::planner::ultra_plan::UltraPhase {
                    id: "final-marker".to_string(),
                    prompt: "Write the final completion marker.".to_string(),
                },
            ],
        };
        let mut planner = EchoGoalPlanner::new();
        let mut execution = FakeClient::new(vec![
            AssistantReply {
                content: String::new(),
                tool_calls: vec![
                    crate::state::ToolCall::new(
                        "Write",
                        serde_json::json!({"path":"phase-1.txt","content":"phase one\n"}),
                    ),
                    crate::state::ToolCall::new(
                        "Write",
                        serde_json::json!({
                            "path":"arcade.jsx",
                            "content":"import { useState } from \"react\";\nexport default function Arcade(){\n  const [score, setScore] = useState(0);\n  return <form onSubmit={(event) => { event.preventDefault(); setScore(score + 1); }}><input value={score} onChange={() => setScore(score + 1)} /><button type=\"submit\">Advance</button></form>;\n}\n"
                        }),
                    ),
                ],
                prompt_tokens: None,
                completion_tokens: None,
            },
            AssistantReply {
                content: String::new(),
                tool_calls: vec![crate::state::ToolCall::new(
                    "Write",
                    serde_json::json!({"path":"phase-2.txt","content":"phase two\n"}),
                )],
                prompt_tokens: None,
                completion_tokens: None,
            },
        ]);

        let result = run_ultra_plan(&mut planner, &mut execution, &plan, &cfg).unwrap();

        assert_eq!(result, "ultra-plan-run complete: 2 phases");
        assert_eq!(planner.messages().len(), 2);
        assert!(dir.path().join("phase-1.txt").is_file());
        assert!(dir.path().join("phase-2.txt").is_file());
        let event_text = std::fs::read_to_string(events).unwrap();
        assert!(event_text.contains("\"event\":\"ultra_phase_scaffold_complete\""));
        assert!(event_text.contains("\"event\":\"planner_plan_sanitized\""));
        assert!(event_text.contains("\"kind\":\"goal_truncated\""));
        assert!(!event_text.contains("phase_scaffold_error"));
    }

    #[test]
    fn ultra_non_final_contract_observation_carries_forward_and_final_phase_can_satisfy() {
        let dir = tempfile::tempdir().unwrap();
        let events = dir.path().join("events.jsonl");
        let mut cfg = config(dir.path().to_path_buf());
        cfg.eval_events_path = Some(events.clone());
        cfg.completion_contract_path = Some(write_challenge_contract(dir.path()));
        let phase_plan = challenge_implement_step_plan_json();
        let mut planner = FakeClient::new(vec![
            AssistantReply::text(phase_plan.clone()),
            AssistantReply::text(phase_plan),
        ]);
        let static_page =
            "export default function Page(){ return <main><canvas>ready</canvas></main>; }";
        let complete_page = "export default function Page(){ const enemies = ['drone']; return <main>enemy challenge {enemies.length}</main>; }";
        let mut execution = FakeClient::new(vec![
            AssistantReply {
                content: String::new(),
                tool_calls: vec![crate::state::ToolCall::new(
                    "Write",
                    serde_json::json!({"path":"src/app/page.tsx","content":static_page}),
                )],
                prompt_tokens: None,
                completion_tokens: None,
            },
            AssistantReply {
                content: String::new(),
                tool_calls: vec![crate::state::ToolCall::new(
                    "Edit",
                    serde_json::json!({
                        "path":"src/app/page.tsx",
                        "old": static_page,
                        "new": complete_page,
                    }),
                )],
                prompt_tokens: None,
                completion_tokens: None,
            },
        ]);
        let plan = challenge_ultra_plan();

        let result = run_ultra_plan(&mut planner, &mut execution, &plan, &cfg).unwrap();

        assert_eq!(result, "ultra-plan-run complete: 2 phases");
        let event_text = std::fs::read_to_string(events).unwrap();
        assert!(event_text.contains("\"event\":\"contract_observation_incomplete\""));
        assert!(event_text.contains("\"contract_enforcement\":\"observe\""));
        assert!(event_text.contains("\"phase_scope\":\"phase-one\""));
        assert!(
            event_text
                .contains("\"pending_capability_evidence\":[\"challenge_or_adversary_evidence\"]")
        );
        assert!(event_text.contains("\"event\":\"ultra_plan_complete\""));
        let phase_two_prompt = planner_request_text(&planner, 1);
        assert!(phase_two_prompt.contains("Unmet final requirements from earlier phases:"));
        assert!(phase_two_prompt.contains("- challenge_or_adversary_evidence"));
        assert!(phase_two_prompt.contains("Close these requirements when they are in scope"));
    }

    #[test]
    fn ultra_observe_no_edit_contract_repair_starts_next_phase() {
        let dir = tempfile::tempdir().unwrap();
        let events = dir.path().join("events.jsonl");
        let mut cfg = config(dir.path().to_path_buf());
        cfg.eval_events_path = Some(events.clone());
        cfg.completion_contract_path = Some(write_challenge_contract_with_cap(dir.path(), 2));
        cfg.max_iterations = 8;
        let phase_plan = challenge_implement_step_plan_json();
        let mut planner = FakeClient::new(vec![
            AssistantReply::text(phase_plan.clone()),
            AssistantReply::text(phase_plan),
        ]);
        let static_page =
            "export default function Page(){ return <main><canvas>ready</canvas></main>; }";
        let complete_page = "export default function Page(){ const enemies = ['drone']; return <main>enemy challenge {enemies.length}</main>; }";
        let mut execution = FakeClient::new(vec![
            AssistantReply {
                content: String::new(),
                tool_calls: vec![crate::state::ToolCall::new(
                    "Write",
                    serde_json::json!({"path":"src/app/page.tsx","content":static_page}),
                )],
                prompt_tokens: None,
                completion_tokens: None,
            },
            AssistantReply::text("I inspected the page but made no edit."),
            AssistantReply {
                content: String::new(),
                tool_calls: vec![crate::state::ToolCall::new(
                    "Edit",
                    serde_json::json!({
                        "path":"src/app/page.tsx",
                        "old": static_page,
                        "new": complete_page,
                    }),
                )],
                prompt_tokens: None,
                completion_tokens: None,
            },
        ]);
        let plan = challenge_ultra_plan();

        let result = run_ultra_plan(&mut planner, &mut execution, &plan, &cfg).unwrap();

        assert_eq!(result, "ultra-plan-run complete: 2 phases");
        assert_eq!(planner.messages().len(), 2);
        assert!(!dir.path().join(".anvil/repairs").exists());
        let event_text = std::fs::read_to_string(events).unwrap();
        assert!(event_text.contains("\"reason\":\"verify_repair_no_change_observed\""));
        assert!(event_text.contains("\"phase_scope\":\"phase-one\""));
        assert!(event_text.contains("\"event\":\"ultra_phase_execute_complete\""));
        assert!(event_text.contains("\"phase_id\":\"phase-two\""));
        let phase_two_prompt = planner_request_text(&planner, 1);
        assert!(phase_two_prompt.contains("Unmet final requirements from earlier phases:"));
        assert!(phase_two_prompt.contains("- challenge_or_adversary_evidence"));
    }

    #[test]
    fn ultra_final_phase_observes_contract_debt_then_final_acceptance_enforces() {
        let dir = tempfile::tempdir().unwrap();
        let events = dir.path().join("events.jsonl");
        let mut cfg = config(dir.path().to_path_buf());
        cfg.eval_events_path = Some(events.clone());
        cfg.completion_contract_path = Some(write_challenge_contract(dir.path()));
        let mut planner = FakeClient::new(vec![
            AssistantReply::text(challenge_implement_step_plan_json()),
            AssistantReply::text(challenge_setup_step_plan_json()),
        ]);
        let static_page =
            "export default function Page(){ return <main><canvas>ready</canvas></main>; }";
        let mut execution = FakeClient::new(vec![
            AssistantReply {
                content: String::new(),
                tool_calls: vec![crate::state::ToolCall::new(
                    "Write",
                    serde_json::json!({"path":"src/app/page.tsx","content":static_page}),
                )],
                prompt_tokens: None,
                completion_tokens: None,
            },
            AssistantReply {
                content: String::new(),
                tool_calls: vec![crate::state::ToolCall::new(
                    "Write",
                    serde_json::json!({"path":"phase-two.txt","content":"verified final phase without adversary evidence"}),
                )],
                prompt_tokens: None,
                completion_tokens: None,
            },
            AssistantReply::text("final acceptance repair could not add evidence"),
        ]);
        let plan = challenge_ultra_plan();

        let err = run_ultra_plan(&mut planner, &mut execution, &plan, &cfg)
            .unwrap_err()
            .to_string();

        assert!(
            err.contains("ultra final acceptance repair failed"),
            "{err}"
        );
        let event_text = std::fs::read_to_string(events).unwrap();
        assert!(!event_text.contains("\"event\":\"plan_final_contract\""));
        assert!(event_text.contains("\"event\":\"ultra_phase_complete\""));
        assert!(event_text.contains("\"phase_id\":\"phase-two\""));
        assert!(event_text.contains("\"event\":\"ultra_final_acceptance\""));
        assert!(event_text.contains("\"event\":\"ultra_final_acceptance_failed\""));
        assert!(event_text.contains("\"event\":\"final_acceptance_repair_start\""));
        assert!(event_text.contains("challenge_or_adversary_evidence"));
        assert!(!event_text.contains("\"event\":\"ultra_plan_complete\""));
    }

    #[test]
    fn ultra_final_acceptance_repair_short_budget_preadvances_to_write_required() {
        let dir = tempfile::tempdir().unwrap();
        let events = dir.path().join("events.jsonl");
        let mut cfg = config(dir.path().to_path_buf());
        cfg.eval_events_path = Some(events.clone());
        cfg.completion_contract_path = Some(write_challenge_contract(dir.path()));
        cfg.max_iterations = 4;
        let mut planner = FakeClient::new(vec![
            AssistantReply::text(challenge_implement_step_plan_json()),
            AssistantReply::text(challenge_setup_step_plan_json()),
        ]);
        let static_page =
            "export default function Page(){ return <main><canvas>ready</canvas></main>; }";
        let mut execution_replies = vec![
            AssistantReply {
                content: String::new(),
                tool_calls: vec![crate::state::ToolCall::new(
                    "Write",
                    serde_json::json!({"path":"src/app/page.tsx","content":static_page}),
                )],
                prompt_tokens: None,
                completion_tokens: None,
            },
            AssistantReply {
                content: String::new(),
                tool_calls: vec![crate::state::ToolCall::new(
                    "Write",
                    serde_json::json!({"path":"phase-two.txt","content":"phase two"}),
                )],
                prompt_tokens: None,
                completion_tokens: None,
            },
        ];
        execution_replies.extend((0..40).map(|_| read_static_page_reply()));
        let mut execution = FakeClient::new(execution_replies);
        let plan = challenge_ultra_plan();

        let err = run_ultra_plan(&mut planner, &mut execution, &plan, &cfg)
            .unwrap_err()
            .to_string();

        assert!(err.contains("model_stagnation:read_only_loop"), "{err}");
        let stages = events_with_name(&events, "read_only_stagnation_feedback")
            .iter()
            .map(|event| {
                event
                    .get("stage")
                    .and_then(Value::as_str)
                    .unwrap()
                    .to_string()
            })
            .collect::<Vec<_>>();
        assert_eq!(stages, vec!["write_required"]);
        let event_text = std::fs::read_to_string(events).unwrap();
        assert!(event_text.contains("\"lifecycle_stage\":\"final_acceptance_repair\""));
        assert!(event_text.contains("\"event\":\"escalation_carryover\""));
        assert!(event_text.contains("\"pre_advanced\":true"));
    }

    #[test]
    fn ultra_final_acceptance_repair_edit_after_read_only_nudge_proceeds() {
        let dir = tempfile::tempdir().unwrap();
        let events = dir.path().join("events.jsonl");
        let mut cfg = config(dir.path().to_path_buf());
        cfg.eval_events_path = Some(events.clone());
        cfg.completion_contract_path = Some(write_challenge_contract(dir.path()));
        cfg.max_iterations = 4;
        let mut planner = FakeClient::new(vec![
            AssistantReply::text(challenge_implement_step_plan_json()),
            AssistantReply::text(challenge_setup_step_plan_json()),
        ]);
        let static_page =
            "export default function Page(){ return <main><canvas>ready</canvas></main>; }";
        let mut execution = FakeClient::new(vec![
            AssistantReply {
                content: String::new(),
                tool_calls: vec![crate::state::ToolCall::new(
                    "Write",
                    serde_json::json!({"path":"src/app/page.tsx","content":static_page}),
                )],
                prompt_tokens: None,
                completion_tokens: None,
            },
            AssistantReply {
                content: String::new(),
                tool_calls: vec![crate::state::ToolCall::new(
                    "Write",
                    serde_json::json!({"path":"phase-two.txt","content":"phase two"}),
                )],
                prompt_tokens: None,
                completion_tokens: None,
            },
            read_static_page_reply(),
            AssistantReply {
                content: String::new(),
                tool_calls: vec![crate::state::ToolCall::new(
                    "Edit",
                    serde_json::json!({
                        "path":"src/app/page.tsx",
                        "old_string": static_page,
                        "new_string": interactive_game_page_source()
                    }),
                )],
                prompt_tokens: None,
                completion_tokens: None,
            },
        ]);
        let plan = challenge_ultra_plan();

        let result = run_ultra_plan(&mut planner, &mut execution, &plan, &cfg).unwrap();

        assert_eq!(result, "ultra-plan-run complete: 2 phases");
        let event_text = std::fs::read_to_string(events).unwrap();
        assert!(event_text.contains("\"event\":\"read_only_stagnation_feedback\""));
        assert!(event_text.contains("\"event\":\"final_acceptance_repair_complete\""));
        assert!(event_text.contains("\"event\":\"ultra_plan_complete\""));
    }

    #[test]
    #[cfg(unix)]
    fn ultra_final_acceptance_runs_probe_when_runtime_evidence_is_missing() {
        let _probe_guard = dev_server_probe_test_guard();
        let dir = tempfile::tempdir().unwrap();
        let events = dir.path().join("events.jsonl");
        let port = free_local_port();
        enable_dev_server_probe_test_override(dir.path());
        write_fake_nextjs_package_manager(dir.path(), false);
        let mut cfg = config(dir.path().to_path_buf());
        cfg.profile = "nextjs".to_string();
        cfg.eval_events_path = Some(events.clone());
        let scaffold_plan = generated_nextjs_fixture_plan_json_with_kind(
            "Create buildable app",
            "check_scaffold.py",
            "setup",
        );
        let final_plan = final_marker_implement_step_plan_json();
        let mut planner_replies = Vec::new();
        for _ in 0..3 {
            planner_replies.push(AssistantReply::text(scaffold_plan.clone()));
        }
        for _ in 0..6 {
            planner_replies.push(AssistantReply::text(final_plan.clone()));
        }
        let mut planner = FakeClient::new(planner_replies);
        let package = format!(
            r#"{{"scripts":{{"build":"next build","dev":"next dev -p {port}"}},"dependencies":{{"next":"^14.2.0","react":"^18.3.0","react-dom":"^18.3.0"}},"devDependencies":{{"typescript":"^5.5.0","@types/node":"^20.14.0","@types/react":"^18.3.0","@types/react-dom":"^18.3.0"}}}}"#
        );
        let mut app_tool_calls =
            nextjs_interactive_app_tool_calls(interactive_game_page_without_restart_source());
        app_tool_calls[0] = crate::state::ToolCall::new(
            "Write",
            serde_json::json!({"path":"package.json","content":package}),
        );
        app_tool_calls.push(crate::state::ToolCall::new(
            "Write",
            serde_json::json!({
                "path":"interaction-evidence.json",
                "content":"{\"ok\":true,\"interaction_performed\":true,\"start_transition\":true,\"input_state_change\":true,\"input_event_observed\":true,\"state_changed\":true,\"canvas_found\":true}"
            }),
        ));
        app_tool_calls.push(crate::state::ToolCall::new(
            "Write",
            serde_json::json!({"path":"check_scaffold.py","content":"x = 1\n"}),
        ));
        let mut execution = FakeClient::new(vec![
            AssistantReply {
                content: String::new(),
                tool_calls: app_tool_calls,
                prompt_tokens: None,
                completion_tokens: None,
            },
            AssistantReply {
                content: String::new(),
                tool_calls: vec![crate::state::ToolCall::new(
                    "Write",
                    serde_json::json!({
                        "path":"src/app/page.tsx",
                        "content":interactive_game_page_without_restart_source()
                    }),
                )],
                prompt_tokens: None,
                completion_tokens: None,
            },
            AssistantReply::text("final acceptance repair could not add restart behavior"),
        ]);
        let plan = UltraPlan {
            goal: explicit_port_goal("Create an interactive browser game", port),
            profile: "nextjs".to_string(),
            style: "default".to_string(),
            intent: "create".to_string(),
            phases: vec![
                crate::planner::ultra_plan::UltraPhase {
                    id: "build".to_string(),
                    prompt: "Create the app".to_string(),
                },
                crate::planner::ultra_plan::UltraPhase {
                    id: "final".to_string(),
                    prompt: "Final implementation pass".to_string(),
                },
            ],
        };

        let result = run_ultra_plan(&mut planner, &mut execution, &plan, &cfg).unwrap();

        assert_eq!(result, "ultra-plan-run complete: 2 phases");
        let event_text = std::fs::read_to_string(&events).unwrap();
        let final_acceptance = latest_event(&events, "ultra_final_acceptance");
        assert!(event_text.contains("\"event\":\"ultra_phase_complete\""));
        assert!(event_text.contains("\"phase_id\":\"final\""));
        assert!(event_text.contains("\"event\":\"ultra_final_acceptance\""));
        assert!(event_text.contains("\"runtime_acceptance_status\":\"partial\""));
        assert!(event_text.contains("\"final_acceptance_status\":\"partial\""));
        assert!(event_text.contains("\"release_gate_status\":\"partial\""));
        assert!(event_text.contains("\"restart_or_recoverable_state_evidence\""));
        assert!(
            final_acceptance
                .get("unverified_evidence")
                .and_then(Value::as_array)
                .is_some_and(|items| items.iter().any(|item| item.as_str()
                    == Some("restart_or_recoverable_state_evidence:unverified:probe_unavailable")))
        );
        assert!(
            event_text.contains("\"browser_readiness_status\":\"passed\""),
            "{event_text}"
        );
        assert!(
            event_text.contains("\"interaction_evidence_status\":\"passed\""),
            "{event_text}"
        );
        assert!(
            !event_text.contains("\"browser_readiness_status\":\"not_checked\""),
            "{event_text}"
        );
        assert!(event_text.contains("\"event\":\"dev_server_lifecycle\""));
        assert!(event_text.contains("\"stage\":\"probe\""));
        assert!(!event_text.contains("\"event\":\"final_acceptance_repair_start\""));
        assert!(
            events
                .parent()
                .unwrap()
                .join("browser-readiness.json")
                .is_file()
        );
        assert!(!execution.messages().iter().flatten().any(|message| {
            message
                .content
                .contains("Repair the final acceptance failure")
        }));
    }

    #[test]
    fn ultra_probe_unavailable_weak_restart_is_partial_without_repair() {
        let dir = tempfile::tempdir().unwrap();
        let events = dir.path().join("events.jsonl");
        let mut cfg = config(dir.path().to_path_buf());
        cfg.profile = "nextjs".to_string();
        cfg.eval_events_path = Some(events.clone());
        write_fake_npm_dependency_installer(dir.path());
        let phase_plan = generated_nextjs_artifact_plan_json("Create and verify the game app");
        let mut planner_replies = Vec::new();
        for _ in 0..8 {
            planner_replies.push(AssistantReply::text(phase_plan.clone()));
        }
        let mut planner = FakeClient::new(planner_replies);
        let mut tool_calls = nextjs_interactive_app_tool_calls(
            cross_file_weak_restart_interactive_game_page_source(),
        );
        tool_calls.push(crate::state::ToolCall::new(
            "Write",
            serde_json::json!({
                "path":"src/app/gameEngine.ts",
                "content":cross_file_weak_restart_game_engine_source()
            }),
        ));
        tool_calls.push(crate::state::ToolCall::new(
            "Write",
            serde_json::json!({
                "path":"browser-readiness.json",
                "content":"{\"ok\":true,\"http_status\":200,\"route_rendered\":true}"
            }),
        ));
        let final_phase_reply = AssistantReply {
            content: String::new(),
            tool_calls: vec![
                crate::state::ToolCall::new(
                    "Write",
                    serde_json::json!({
                        "path":"src/app/page.tsx",
                        "content":cross_file_weak_restart_interactive_game_page_source()
                    }),
                ),
                crate::state::ToolCall::new(
                    "Write",
                    serde_json::json!({
                        "path":"browser-readiness.json",
                        "content":"{\"ok\":true,\"http_status\":200,\"route_rendered\":true}"
                    }),
                ),
            ],
            prompt_tokens: None,
            completion_tokens: None,
        };
        let mut execution_replies = vec![AssistantReply {
            content: String::new(),
            tool_calls,
            prompt_tokens: None,
            completion_tokens: None,
        }];
        for _ in 0..8 {
            execution_replies.push(final_phase_reply.clone());
        }
        let mut execution = FakeClient::new(execution_replies);
        let plan = UltraPlan {
            goal: "Create an interactive browser game".to_string(),
            profile: "nextjs".to_string(),
            style: "default".to_string(),
            intent: "create".to_string(),
            phases: vec![
                UltraPhase {
                    id: "build".to_string(),
                    prompt: "Create the interactive game app".to_string(),
                },
                UltraPhase {
                    id: "final".to_string(),
                    prompt: "Verify the final interactive game app".to_string(),
                },
            ],
        };

        let result = run_ultra_plan(&mut planner, &mut execution, &plan, &cfg).unwrap();

        assert_eq!(result, "ultra-plan-run complete: 2 phases");
        let event_text = std::fs::read_to_string(&events).unwrap();
        let final_acceptance = latest_event(&events, "ultra_final_acceptance");
        assert!(event_text.contains("\"event\":\"ultra_final_acceptance\""));
        assert!(event_text.contains("\"runtime_acceptance_status\":\"partial\""));
        assert!(event_text.contains("\"final_acceptance_status\":\"partial\""));
        assert!(event_text.contains("\"release_gate_status\":\"partial\""));
        assert_eq!(
            final_acceptance
                .get("assurance_level")
                .and_then(Value::as_str),
            Some("partial")
        );
        assert!(
            final_acceptance
                .get("unverified_evidence")
                .and_then(Value::as_array)
                .is_some_and(|items| items.iter().any(|item| item.as_str()
                    == Some("restart_or_recoverable_state_evidence:unverified:probe_unavailable")))
        );
        assert_eq!(
            final_acceptance
                .get("evidence_tiers")
                .and_then(|tiers| tiers.get("restart_or_recoverable_state_evidence"))
                .and_then(Value::as_str),
            Some("unverified:probe_unavailable")
        );
        assert!(
            !final_acceptance
                .get("missing_evidence")
                .and_then(Value::as_array)
                .is_some_and(|items| items
                    .iter()
                    .any(|item| item.as_str() == Some("restart_or_recoverable_state_evidence")))
        );
        assert!(event_text.contains("interaction_unverified:probe_unavailable"));
        assert!(!event_text.contains("\"event\":\"final_acceptance_repair_start\""));
        assert!(!execution.messages().iter().flatten().any(|message| {
            message
                .content
                .contains("Repair the final acceptance failure")
        }));
    }

    #[test]
    fn ultra_final_acceptance_report_enforces_contract_evidence_without_implement_step() {
        let dir = tempfile::tempdir().unwrap();
        let events = dir.path().join("events.jsonl");
        let mut cfg = config(dir.path().to_path_buf());
        cfg.eval_events_path = Some(events.clone());
        cfg.completion_contract_path = Some(write_challenge_contract(dir.path()));
        std::fs::create_dir_all(dir.path().join("src/app")).unwrap();
        std::fs::write(
            dir.path().join("src/app/page.tsx"),
            "export default function Page(){ return <main><canvas>ready</canvas></main>; }",
        )
        .unwrap();
        let plan = challenge_ultra_plan();

        let report = ultra_final_acceptance_report(&plan, &cfg).unwrap();

        assert!(!report.is_pass(), "{report:?}");
        assert!(
            report
                .primary_reason()
                .contains("challenge_or_adversary_evidence"),
            "{report:?}"
        );
        let event_text = std::fs::read_to_string(events).unwrap();
        assert!(event_text.contains("\"event\":\"ultra_final_acceptance\""));
        assert!(event_text.contains("\"runtime_acceptance_passed\":false"));
        assert!(event_text.contains("\"missing_evidence\":[\"challenge_or_adversary_evidence\"]"));
    }

    #[test]
    fn ultra_final_acceptance_reports_plan_adherence_without_gating() {
        let dir = tempfile::tempdir().unwrap();
        let events = dir.path().join("events.jsonl");
        let mut cfg = config(dir.path().to_path_buf());
        cfg.profile = "generic".to_string();
        cfg.eval_events_path = Some(events.clone());
        std::fs::create_dir_all(dir.path().join("src/app")).unwrap();
        std::fs::write(
            dir.path().join("src/app/page.tsx"),
            "// canvas pause\nexport default function Page(){ return <main><button>Start</button><div>Score</div></main>; }",
        )
        .unwrap();
        let plan = UltraPlan {
            goal: "Build a simple page".to_string(),
            profile: "generic".to_string(),
            style: "default".to_string(),
            intent: "create".to_string(),
            phases: vec![UltraPhase {
                id: "gameplay".to_string(),
                prompt: "Implement canvas rendering with pause controls and score display"
                    .to_string(),
            }],
        };

        let report = ultra_final_acceptance_report(&plan, &cfg).unwrap();

        assert!(report.is_pass(), "{report:?}");
        let event_text = std::fs::read_to_string(&events).unwrap();
        assert!(
            event_text.contains("\"plan_adherence_missing\""),
            "{event_text}"
        );
        assert!(event_text.contains("\"canvas\""), "{event_text}");
        assert!(event_text.contains("\"pause\""), "{event_text}");
        assert!(event_text.contains("\"score\""), "{event_text}");
        let snapshot = eval_events::latest_completion_snapshot(Some(&events));
        let projection = eval_events::project_completion(report.is_pass(), &snapshot);
        eval_events::append_completion_summary(
            Some(&events),
            "process",
            None,
            Some("/ultra-plan-run"),
            "completed",
            "",
            &projection,
        );
        let summary = std::fs::read_to_string(events.parent().unwrap().join("summary.md")).unwrap();
        assert!(summary.contains("Plan adherence:"), "{summary}");
        assert!(summary.contains("Missing tokens:"), "{summary}");
        assert!(summary.contains("- canvas"), "{summary}");
        assert!(summary.contains("- pause"), "{summary}");
        assert!(summary.contains("Status: complete"), "{summary}");
    }

    #[test]
    fn ultra_final_acceptance_report_records_browser_probe_failure() {
        let dir = tempfile::tempdir().unwrap();
        let events = dir.path().join("events.jsonl");
        let mut cfg = config(dir.path().to_path_buf());
        cfg.profile = "nextjs".to_string();
        cfg.eval_events_path = Some(events.clone());
        enable_browser_probe_test_override(dir.path());
        let port = write_browser_probe_mock_command(dir.path(), "500");
        std::fs::create_dir_all(dir.path().join("src/app")).unwrap();
        std::fs::write(
            dir.path().join("package.json"),
            format!(
                r#"{{"scripts":{{"build":"next build","dev":"next dev -p {port}","start":"next start -p {port}"}},"dependencies":{{"next":"^14.2.0","react":"^18.3.0","react-dom":"^18.3.0"}},"devDependencies":{{"typescript":"^5.5.0","@types/node":"^20.14.0","@types/react":"^18.3.0","@types/react-dom":"^18.3.0","tailwindcss":"^3.4.19","postcss":"^8.5.15","autoprefixer":"^10.4.20"}}}}"#
            ),
        )
        .unwrap();
        std::fs::write(dir.path().join("tsconfig.json"), nextjs_tsconfig_json()).unwrap();
        std::fs::write(
            dir.path().join("postcss.config.js"),
            nextjs_postcss_config(),
        )
        .unwrap();
        std::fs::write(
            dir.path().join("tailwind.config.ts"),
            nextjs_tailwind_config_ts(),
        )
        .unwrap();
        std::fs::write(
            dir.path().join("src/app/page.tsx"),
            interactive_game_page_source(),
        )
        .unwrap();
        std::fs::write(
            dir.path().join("src/app/layout.tsx"),
            nextjs_layout_source(),
        )
        .unwrap();
        std::fs::write(dir.path().join("src/app/globals.css"), nextjs_globals_css()).unwrap();
        std::fs::write(
            dir.path().join("src/app/global.d.ts"),
            "declare module \"*.css\";",
        )
        .unwrap();
        let plan = UltraPlan {
            goal: explicit_port_goal("Create an interactive browser game", port),
            profile: "nextjs".to_string(),
            style: "default".to_string(),
            intent: "create".to_string(),
            phases: vec![crate::planner::ultra_plan::UltraPhase {
                id: "finish".to_string(),
                prompt: "Finish app".to_string(),
            }],
        };

        let report = ultra_final_acceptance_report(&plan, &cfg).unwrap();

        assert!(!report.is_pass(), "{report:?}");
        let primary = report.primary_reason();
        assert!(
            primary.contains("browser_readiness_failed:http_500"),
            "{primary}"
        );
        assert!(
            report
                .profile_failures
                .iter()
                .any(|failure| failure.contains("browser_probe_output")
                    && failure.contains("Module parse failed")),
            "{report:?}"
        );
        let event_text = std::fs::read_to_string(events).unwrap();
        assert!(event_text.contains("\"event\":\"browser_probe\""));
        assert!(event_text.contains("\"status\":\"failed\""));
        assert!(event_text.contains("\"failure_kind\":\"http_500\""));
        assert!(event_text.contains("\"child_reaped\":true"));
        assert!(
            std::fs::read_to_string(dir.path().join(".anvil/evidence/browser-readiness.json"))
                .unwrap()
                .contains("\"http_status\": 500")
        );
    }

    #[test]
    fn profile_repair_uses_existing_ultra_session_context() {
        let dir = tempfile::tempdir().unwrap();
        let mut session = SessionSnapshot::new();
        session.messages.push(ConversationMessage::user(
            "Prior phase created package.json".to_string(),
        ));
        let mut fake = FakeClient::new(vec![
            AssistantReply {
                content: String::new(),
                tool_calls: vec![crate::state::ToolCall::new(
                    "Write",
                    serde_json::json!({"path":"repair.txt","content":"fixed"}),
                )],
                prompt_tokens: None,
                completion_tokens: None,
            },
            AssistantReply::text("done"),
        ]);
        let outcome = run_profile_repair_with_ultra_session(
            &mut fake,
            &mut session,
            "Create repair.txt as a bounded profile repair.",
            &["repair.txt".to_string()],
            &config(dir.path().to_path_buf()),
            &NOOP_UI,
        )
        .unwrap();
        assert!(dir.path().join("repair.txt").is_file());
        assert!(outcome.changed_paths.contains(&"repair.txt".to_string()));
        let first_request = fake
            .messages()
            .first()
            .expect("profile repair request")
            .iter()
            .map(|message| message.content.as_str())
            .collect::<Vec<_>>()
            .join("\n");
        assert!(first_request.contains("Prior phase created package.json"));
        assert!(first_request.contains("Create repair.txt as a bounded profile repair."));
    }

    #[test]
    fn ultra_final_acceptance_failure_runs_bounded_repair() {
        let dir = tempfile::tempdir().unwrap();
        let events = dir.path().join("events.jsonl");
        let mut cfg = config(dir.path().to_path_buf());
        cfg.eval_events_path = Some(events.clone());
        let mut planner = FakeClient::new(vec![
            AssistantReply::text(generated_nextjs_fixture_plan_json_with_kind(
                "scaffold phase",
                "check_scaffold.py",
                "setup",
            )),
            AssistantReply::text(generated_nextjs_fixture_plan_json_with_kind(
                "finish phase",
                "check_finish.py",
                "setup",
            )),
        ]);
        let package = nextjs_complete_package_json();
        let static_page =
            "export default function Page(){return <main>Press any key to start</main>;}";
        let mut execution = FakeClient::new(vec![
            AssistantReply {
                content: String::new(),
                tool_calls: vec![
                    crate::state::ToolCall::new(
                        "Write",
                        serde_json::json!({"path":"package.json","content":package}),
                    ),
                    crate::state::ToolCall::new(
                        "Write",
                        serde_json::json!({"path":"tsconfig.json","content":nextjs_tsconfig_json()}),
                    ),
                    crate::state::ToolCall::new(
                        "Write",
                        serde_json::json!({"path":"postcss.config.js","content":nextjs_postcss_config()}),
                    ),
                    crate::state::ToolCall::new(
                        "Write",
                        serde_json::json!({"path":"tailwind.config.ts","content":nextjs_tailwind_config_ts()}),
                    ),
                    crate::state::ToolCall::new(
                        "Write",
                        serde_json::json!({"path":"src/app/page.tsx","content":static_page}),
                    ),
                    crate::state::ToolCall::new(
                        "Write",
                        serde_json::json!({"path":"src/app/layout.tsx","content":nextjs_layout_source()}),
                    ),
                    crate::state::ToolCall::new(
                        "Write",
                        serde_json::json!({"path":"src/app/globals.css","content":nextjs_globals_css()}),
                    ),
                    crate::state::ToolCall::new(
                        "Write",
                        serde_json::json!({"path":"src/app/global.d.ts","content":"declare module \"*.css\";"}),
                    ),
                    crate::state::ToolCall::new(
                        "Write",
                        serde_json::json!({"path":"check_scaffold.py","content":"x = 1\n"}),
                    ),
                ],
                prompt_tokens: None,
                completion_tokens: None,
            },
            AssistantReply {
                content: String::new(),
                tool_calls: vec![crate::state::ToolCall::new(
                    "Write",
                    serde_json::json!({"path":"check_finish.py","content":"x = 2\n"}),
                )],
                prompt_tokens: None,
                completion_tokens: None,
            },
            AssistantReply {
                content: String::new(),
                tool_calls: vec![crate::state::ToolCall::new(
                    "Edit",
                    serde_json::json!({
                        "path":"src/app/page.tsx",
                        "old": static_page,
                        "new": interactive_game_page_source()
                    }),
                )],
                prompt_tokens: None,
                completion_tokens: None,
            },
        ]);
        let plan = UltraPlan {
            goal: "Create an interactive browser game".to_string(),
            profile: "nextjs".to_string(),
            style: "default".to_string(),
            intent: "create".to_string(),
            phases: vec![
                crate::planner::ultra_plan::UltraPhase {
                    id: "scaffold".to_string(),
                    prompt: "Scaffold Next.js app".to_string(),
                },
                crate::planner::ultra_plan::UltraPhase {
                    id: "finish".to_string(),
                    prompt: "Finish the app".to_string(),
                },
            ],
        };
        let result = run_ultra_plan(&mut planner, &mut execution, &plan, &cfg).unwrap();
        assert_eq!(result, "ultra-plan-run complete: 2 phases");
        let page = std::fs::read_to_string(dir.path().join("src/app/page.tsx")).unwrap();
        assert!(page.contains("onKeyDown"));
        assert!(page.contains("score"));
        assert!(page.contains("collision"));
        let event_text = std::fs::read_to_string(events).unwrap();
        assert!(event_text.contains("ultra_final_acceptance_failed"));
        assert!(event_text.contains("final_acceptance_repair_start"));
        assert!(event_text.contains("final_acceptance_repair_complete"));
        assert!(event_text.contains("ultra_plan_complete"));
        assert!(event_text.contains("\"release_gate_status\":\"partial\""));
        assert!(event_text.contains("\"event\":\"recovery_prompt_saved\""));
        assert!(event_text.contains("\"handoff_saved_not_success\":true"));
        assert!(event_text.contains("\"recovery_handoff_saved\":true"));
        let repair_prompt = execution
            .messages()
            .iter()
            .map(|messages| {
                messages
                    .iter()
                    .map(|message| message.content.as_str())
                    .collect::<Vec<_>>()
                    .join("\n")
            })
            .find(|prompt| prompt.contains("Repair the final acceptance failure"))
            .expect("final acceptance repair request");
        assert!(repair_prompt.contains("Repair the final acceptance failure"));
        assert!(repair_prompt.contains("attempt: 1/2"));
        assert!(repair_prompt.contains("without weakening verification"));
    }

    #[test]
    fn ultra_final_acceptance_repair_failure_saves_recovery_handoff() {
        let dir = tempfile::tempdir().unwrap();
        let events = dir.path().join("events.jsonl");
        let mut cfg = config(dir.path().to_path_buf());
        cfg.eval_events_path = Some(events.clone());
        let mut planner = FakeClient::new(vec![
            AssistantReply::text(generated_nextjs_fixture_plan_json_with_kind(
                "scaffold phase",
                "check_scaffold.py",
                "setup",
            )),
            AssistantReply::text(generated_nextjs_fixture_plan_json_with_kind(
                "finish phase",
                "check_finish.py",
                "setup",
            )),
        ]);
        let package = nextjs_complete_package_json();
        let static_page =
            "export default function Page(){return <main>Press any key to start</main>;}";
        let mut execution = FakeClient::new(vec![
            AssistantReply {
                content: String::new(),
                tool_calls: vec![
                    crate::state::ToolCall::new(
                        "Write",
                        serde_json::json!({"path":"package.json","content":package}),
                    ),
                    crate::state::ToolCall::new(
                        "Write",
                        serde_json::json!({"path":"tsconfig.json","content":nextjs_tsconfig_json()}),
                    ),
                    crate::state::ToolCall::new(
                        "Write",
                        serde_json::json!({"path":"postcss.config.js","content":nextjs_postcss_config()}),
                    ),
                    crate::state::ToolCall::new(
                        "Write",
                        serde_json::json!({"path":"tailwind.config.ts","content":nextjs_tailwind_config_ts()}),
                    ),
                    crate::state::ToolCall::new(
                        "Write",
                        serde_json::json!({"path":"src/app/page.tsx","content":static_page}),
                    ),
                    crate::state::ToolCall::new(
                        "Write",
                        serde_json::json!({"path":"src/app/layout.tsx","content":nextjs_layout_source()}),
                    ),
                    crate::state::ToolCall::new(
                        "Write",
                        serde_json::json!({"path":"src/app/globals.css","content":nextjs_globals_css()}),
                    ),
                    crate::state::ToolCall::new(
                        "Write",
                        serde_json::json!({"path":"src/app/global.d.ts","content":"declare module \"*.css\";"}),
                    ),
                    crate::state::ToolCall::new(
                        "Write",
                        serde_json::json!({"path":"check_scaffold.py","content":"x = 1\n"}),
                    ),
                ],
                prompt_tokens: None,
                completion_tokens: None,
            },
            AssistantReply {
                content: String::new(),
                tool_calls: vec![crate::state::ToolCall::new(
                    "Write",
                    serde_json::json!({"path":"check_finish.py","content":"x = 2\n"}),
                )],
                prompt_tokens: None,
                completion_tokens: None,
            },
        ]);
        let plan = UltraPlan {
            goal: "Create an interactive browser game".to_string(),
            profile: "nextjs".to_string(),
            style: "default".to_string(),
            intent: "create".to_string(),
            phases: vec![
                crate::planner::ultra_plan::UltraPhase {
                    id: "scaffold".to_string(),
                    prompt: "Scaffold Next.js app".to_string(),
                },
                crate::planner::ultra_plan::UltraPhase {
                    id: "finish".to_string(),
                    prompt: "Finish the app".to_string(),
                },
            ],
        };
        let err = run_ultra_plan(&mut planner, &mut execution, &plan, &cfg)
            .unwrap_err()
            .to_string();
        assert!(err.contains("ultra final acceptance repair failed"));
        assert!(err.contains("Recovery artifact check"));
        assert!(
            err.contains("/run-ultra-plan .anvil/plans/recovery-ultra-plan-"),
            "{err}"
        );
        assert!(err.contains(".yaml"), "{err}");
        let repairs_dir = dir.path().join(".anvil/repairs");
        assert!(repairs_dir.is_dir());
        assert!(std::fs::read_dir(&repairs_dir).unwrap().next().is_some());
        let recovery_plan = assert_single_recovery_ultra_plan(dir.path());
        assert_eq!(recovery_plan.goal, plan.goal);
        assert_eq!(recovery_plan.profile, "nextjs");
        assert!(recovery_plan.phases.iter().any(|phase| {
            phase
                .prompt
                .contains("Missing capability or artifact signals")
        }));
        let event_text = std::fs::read_to_string(events).unwrap();
        assert!(event_text.contains("final_acceptance_repair_start"));
        assert!(event_text.contains("final_acceptance_repair_failed"));
        assert!(event_text.contains("recovery_prompt_saved"));
        assert!(event_text.contains("recovery_ultra_plan_path"));
        assert!(event_text.contains("\"recovery_prompt_parse_ok\":true"));
        assert!(event_text.contains("\"recovery_yaml_parse_ok\":true"));
        assert!(event_text.contains("\"recovery_command_targets_valid\":true"));
        assert!(event_text.contains("suggested_recovery_yaml_command"));
        assert!(event_text.contains("suggested_recovery_command"));
        let summary = std::fs::read_to_string(dir.path().join("summary.md")).unwrap();
        assert!(summary.contains("Status: incomplete"));
        assert!(summary.contains("Completed phases:\n- scaffold"));
        assert!(summary.contains("Failed phase:\n- finish"));
        assert!(summary.contains("Pending phases:\n- none"));
        assert!(summary.contains("Recovery next action:"));
        assert!(summary.contains("Recovery UltraPlan YAML saved:"));
        assert!(summary.contains("Suggested YAML command:"));
        assert!(summary.contains("Recovery artifact check:"));
    }

    #[test]
    fn ultra_phase_scaffold_failure_saves_recovery_yaml_and_incomplete_handoff() {
        let dir = tempfile::tempdir().unwrap();
        let events = dir.path().join("events.jsonl");
        let mut cfg = config(dir.path().to_path_buf());
        cfg.profile = "nextjs".to_string();
        cfg.eval_events_path = Some(events.clone());
        let mut planner = FakeClient::new(vec![
            AssistantReply::text("not a step plan"),
            AssistantReply::text("still not a step plan"),
            AssistantReply::text("no valid step plan"),
        ]);
        let mut execution = FakeClient::new(Vec::new());
        let plan = UltraPlan {
            goal: "Build an interactive web game".to_string(),
            profile: "nextjs".to_string(),
            style: "default".to_string(),
            intent: "create".to_string(),
            phases: vec![
                crate::planner::ultra_plan::UltraPhase {
                    id: "web-audio-synth-and-ui".to_string(),
                    prompt: "Add audio, HUD, overlays, and deterministic verification".to_string(),
                },
                crate::planner::ultra_plan::UltraPhase {
                    id: "final-verify".to_string(),
                    prompt: "Verify the recovered interactive app".to_string(),
                },
            ],
        };
        let err = run_ultra_plan(&mut planner, &mut execution, &plan, &cfg)
            .unwrap_err()
            .to_string();
        assert!(err.contains("phase scaffold failed"), "{err}");
        assert!(err.contains("incomplete"), "{err}");
        assert!(err.contains("Recovery UltraPlan YAML saved"), "{err}");
        assert!(err.contains(".yaml"), "{err}");
        assert!(err.contains("Recovery artifact check"), "{err}");
        assert!(
            err.contains("/run-ultra-plan .anvil/plans/recovery-ultra-plan-"),
            "{err}"
        );
        let recovery_plan = assert_single_recovery_ultra_plan(dir.path());
        assert_eq!(recovery_plan.goal, "Build an interactive web game");
        assert_eq!(recovery_plan.profile, "nextjs");
        let rendered = render_ultra_plan(&recovery_plan);
        assert_eq!(parse_ultra_plan(&rendered).unwrap(), recovery_plan);
        assert!(
            recovery_plan
                .phases
                .iter()
                .any(|phase| phase.prompt.contains("web-audio-synth-and-ui"))
        );
        assert!(recovery_plan.phases.iter().any(|phase| {
            phase
                .prompt
                .contains("Missing capability or artifact signals")
        }));
        assert!(
            recovery_plan
                .phases
                .iter()
                .any(|phase| phase.prompt.contains("Verify preference"))
        );
        let event_text = std::fs::read_to_string(events).unwrap();
        assert!(event_text.contains("\"planner_error_kind\":\"phase_scaffold_error\""));
        assert!(!event_text.contains("\"event\":\"planner_fallback_plan\""));
        assert!(event_text.contains("\"event\":\"recovery_prompt_saved\""));
        assert!(event_text.contains("\"status\":\"incomplete\""));
        assert!(event_text.contains("\"recovery_yaml_missing\":false"));
        assert!(event_text.contains("\"recovery_prompt_parse_ok\":true"));
        assert!(event_text.contains("\"recovery_yaml_parse_ok\":true"));
        assert!(event_text.contains("\"recovery_command_targets_valid\":true"));
        assert!(event_text.contains("\"recovery_ultra_plan_path\""));
        assert!(event_text.contains("\"suggested_recovery_yaml_command\""));
        let summary = std::fs::read_to_string(dir.path().join("summary.md")).unwrap();
        assert!(summary.contains("Status: incomplete"));
        assert!(summary.contains("Completed phases:\n- none"));
        assert!(summary.contains("Failed phase:\n- web-audio-synth-and-ui"));
        assert!(summary.contains("Pending phases:\n- final-verify"));
        assert!(summary.contains("Recovery next action:"));
        assert!(summary.contains("Recovery UltraPlan YAML saved:"));
        assert!(summary.contains("Suggested YAML command:"));
        assert!(summary.contains("Recovery artifact check:"));
    }

    #[test]
    fn ultra_phase_planner_provider_error_names_terminal_reason() {
        let dir = tempfile::tempdir().unwrap();
        let events = dir.path().join("events.jsonl");
        let mut cfg = config(dir.path().to_path_buf());
        cfg.profile = "nextjs".to_string();
        cfg.eval_events_path = Some(events.clone());
        let mut planner = FlakyClient::new(
            PLANNER_PROVIDER_REQUEST_ATTEMPTS,
            "transient provider unavailable",
            Vec::new(),
        );
        let mut execution = FakeClient::new(Vec::new());
        let plan = UltraPlan {
            goal: "Build an interactive web game".to_string(),
            profile: "nextjs".to_string(),
            style: "default".to_string(),
            intent: "create".to_string(),
            phases: vec![
                crate::planner::ultra_plan::UltraPhase {
                    id: "web-game-implementation".to_string(),
                    prompt: "Implement game logic, player control, collision, score, and canvas behavior".to_string(),
                },
                crate::planner::ultra_plan::UltraPhase {
                    id: "final-verify".to_string(),
                    prompt: "Verify the interactive web game".to_string(),
                },
            ],
        };

        let err = run_ultra_plan(&mut planner, &mut execution, &plan, &cfg)
            .unwrap_err()
            .to_string();

        assert_eq!(planner.messages().len(), PLANNER_PROVIDER_REQUEST_ATTEMPTS);
        assert!(err.contains("phase scaffold failed"), "{err}");
        assert!(
            err.contains("provider request failed after 2 attempts"),
            "{err}"
        );
        assert!(err.contains("transient provider unavailable"), "{err}");
        let event_text = std::fs::read_to_string(events).unwrap();
        assert!(event_text.contains("\"planner_error_kind\":\"phase_scaffold_error\""));
        assert!(event_text.contains("transient provider unavailable"));
    }

    #[test]
    fn ultra_phase_execute_failure_saves_complete_recovery_yaml_in_stop_reason() {
        let dir = tempfile::tempdir().unwrap();
        let events = dir.path().join("events.jsonl");
        let mut cfg = config(dir.path().to_path_buf());
        cfg.eval_events_path = Some(events.clone());
        let phase_step_plan = StepPlan {
            goal: "phase one".to_string(),
            steps: vec![PlanStep {
                id: "write-phase-one".to_string(),
                kind: "implement".to_string(),
                expected_result: "phase-one.txt exists".to_string(),
                instruction: "Create phase-one.txt".to_string(),
                expected_paths: vec!["phase-one.txt".to_string()],
                verify: Vec::new(),
            }],
        };
        let mut planner = FakeClient::new(vec![AssistantReply::text(
            serde_json::to_string(&phase_step_plan).unwrap(),
        )]);
        let mut execution = FakeClient::new(Vec::new());
        let plan = UltraPlan {
            goal: "Do two phases".to_string(),
            profile: "generic".to_string(),
            style: "default".to_string(),
            intent: "create".to_string(),
            phases: vec![
                UltraPhase {
                    id: "phase-one".to_string(),
                    prompt: "Create first artifact".to_string(),
                },
                UltraPhase {
                    id: "phase-two".to_string(),
                    prompt: "Finish second artifact".to_string(),
                },
            ],
        };

        let err = run_ultra_plan(&mut planner, &mut execution, &plan, &cfg)
            .unwrap_err()
            .to_string();

        assert!(err.contains("phase phase-one failed"), "{err}");
        assert!(err.contains("Paths:"), "{err}");
        assert!(
            err.contains("/run-ultra-plan .anvil/plans/recovery-ultra-plan-"),
            "{err}"
        );
        assert!(err.contains(".yaml"), "{err}");
        let summary = std::fs::read_to_string(dir.path().join("summary.md")).unwrap();
        assert!(summary.contains("Failed phase:\n- phase-one"), "{summary}");
        assert!(
            summary.contains("Pending phases:\n- phase-two"),
            "{summary}"
        );
        assert!(
            summary.contains("Recovery UltraPlan YAML saved:"),
            "{summary}"
        );
        assert!(summary.contains(".yaml"), "{summary}");
        let event_text = std::fs::read_to_string(events).unwrap();
        assert!(event_text.contains("\"recovery_handoff_kind\":\"phase_execute_error\""));
        assert!(event_text.contains("\"recovery_ultra_plan_path\":\".anvil/plans/"));
    }

    #[test]
    fn ultra_phase_summary_notes_missing_browser_evidence_when_probe_available_unexercised() {
        let dir = tempfile::tempdir().unwrap();
        let events = dir.path().join("events.jsonl");
        interaction_probe::write_test_availability_override(dir.path(), true);
        let mut cfg = config(dir.path().to_path_buf());
        cfg.eval_events_path = Some(events);
        cfg.profile = "nextjs".to_string();
        let plan = UltraPlan {
            goal: "Create an interactive browser app".to_string(),
            profile: "nextjs".to_string(),
            style: "default".to_string(),
            intent: "create".to_string(),
            phases: vec![
                UltraPhase {
                    id: "phase-one".to_string(),
                    prompt: "Create the app shell".to_string(),
                },
                UltraPhase {
                    id: "final-acceptance".to_string(),
                    prompt: "Run final browser acceptance".to_string(),
                },
            ],
        };
        let missing_paths: Vec<String> = Vec::new();
        let missing_signals: Vec<String> = Vec::new();
        let repair_targets = vec!["verifier_command".to_string()];

        let _handoff = save_ultra_phase_recovery_handoff(
            &cfg,
            &plan,
            &plan.phases[0],
            UltraPhaseRecoveryRequest {
                failure_kind: "deterministic_verify_command_bug",
                reason: "verify command malformed",
                missing_paths: &missing_paths,
                missing_signals: &missing_signals,
                repair_targets: &repair_targets,
            },
        )
        .expect("handoff saved");

        let summary = std::fs::read_to_string(dir.path().join("summary.md")).unwrap();
        assert!(
            summary.contains(
                "Browser evidence missing: run failed before final acceptance (interaction probe installed but not exercised)."
            ),
            "{summary}"
        );
    }

    #[test]
    fn ultra_phase_recovery_handoff_renders_observed_missing_evidence_keys() {
        let dir = tempfile::tempdir().unwrap();
        let events = dir.path().join("events.jsonl");
        let mut cfg = config(dir.path().to_path_buf());
        cfg.eval_events_path = Some(events.clone());
        let plan = challenge_ultra_plan();
        let phase = &plan.phases[0];

        let parts = save_ultra_phase_recovery_handoff(
            &cfg,
            &plan,
            phase,
            UltraPhaseRecoveryRequest {
                failure_kind: "phase_execute_error",
                reason: "missing_required_evidence:challenge_or_adversary_evidence",
                missing_paths: &[],
                missing_signals: &["challenge_or_adversary_evidence".to_string()],
                repair_targets: &["completion_contract".to_string()],
            },
        )
        .expect("recovery handoff should be saved");
        let message = eval_events::render_stop_reason(&parts);

        assert!(
            message.contains("Recovery UltraPlan YAML saved"),
            "{message}"
        );
        assert!(
            message.contains(".anvil/plans/recovery-ultra-plan-"),
            "{message}"
        );
        assert!(message.contains(".yaml"), "{message}");
        let repairs_dir = dir.path().join(".anvil/repairs");
        let repair_text = std::fs::read_dir(&repairs_dir)
            .unwrap()
            .map(|entry| std::fs::read_to_string(entry.unwrap().path()).unwrap())
            .find(|text| text.contains("Missing capabilities:"))
            .expect("recovery prompt");
        assert!(
            repair_text.contains("- challenge_or_adversary_evidence"),
            "{repair_text}"
        );
        assert!(!repair_text.contains("Missing capabilities:\n- none"));
        assert!(repair_text.contains("Repair targets:\n- completion_contract"));
        let recovery_plan = assert_single_recovery_ultra_plan(dir.path());
        assert!(
            recovery_plan
                .phases
                .iter()
                .any(|phase| { phase.prompt.contains("- challenge_or_adversary_evidence") })
        );
        assert!(recovery_plan.phases.iter().any(|phase| {
            phase
                .prompt
                .contains("Repair targets:\n- completion_contract")
        }));
        let event_text = std::fs::read_to_string(events).unwrap();
        assert!(event_text.contains("\"recovery_prompt_path\":\".anvil/repairs/"));
        assert!(event_text.contains("\"recovery_ultra_plan_path\":\".anvil/plans/"));
    }

    #[test]
    fn ultra_phase_recovery_handoff_uses_contract_error_observations() {
        let dir = tempfile::tempdir().unwrap();
        let events = dir.path().join("events.jsonl");
        let mut cfg = config(dir.path().to_path_buf());
        cfg.eval_events_path = Some(events.clone());
        cfg.completion_contract_path = Some(write_challenge_contract(dir.path()));
        let plan = challenge_ultra_plan();
        let mut planner = FakeClient::new(vec![
            AssistantReply::text(challenge_setup_step_plan_json()),
            AssistantReply::text(challenge_implement_step_plan_json()),
        ]);
        let static_page =
            "export default function Page(){ return <main><canvas>ready</canvas></main>; }";
        let mut execution = FakeClient::new(vec![
            AssistantReply {
                content: String::new(),
                tool_calls: vec![crate::state::ToolCall::new(
                    "Write",
                    serde_json::json!({"path":"phase-two.txt","content":"phase one complete"}),
                )],
                prompt_tokens: None,
                completion_tokens: None,
            },
            AssistantReply {
                content: String::new(),
                tool_calls: vec![crate::state::ToolCall::new(
                    "Write",
                    serde_json::json!({"path":"src/app/page.tsx","content":static_page}),
                )],
                prompt_tokens: None,
                completion_tokens: None,
            },
            AssistantReply {
                content: String::new(),
                tool_calls: vec![crate::state::ToolCall::new(
                    "Write",
                    serde_json::json!({"path":"src/app/page.tsx","content":static_page}),
                )],
                prompt_tokens: None,
                completion_tokens: None,
            },
        ]);

        let err = run_ultra_plan(&mut planner, &mut execution, &plan, &cfg)
            .unwrap_err()
            .to_string();

        assert!(
            err.contains("ultra final acceptance repair failed"),
            "{err}"
        );
        assert!(err.contains("challenge_or_adversary_evidence"), "{err}");
        let repairs_dir = dir.path().join(".anvil/repairs");
        let repair_text = std::fs::read_dir(&repairs_dir)
            .unwrap()
            .map(|entry| std::fs::read_to_string(entry.unwrap().path()).unwrap())
            .find(|text| text.contains("- phase: phase-two"))
            .expect("phase recovery prompt");
        assert!(
            repair_text.contains("- challenge_or_adversary_evidence"),
            "{repair_text}"
        );
        assert!(!repair_text.contains("Missing capabilities:\n- none"));
        assert!(!repair_text.contains("Repair targets:\n- none"));
        assert!(repair_text.contains("Repair targets:\n- "), "{repair_text}");
        let recovery_plan = std::fs::read_dir(dir.path().join(".anvil/plans"))
            .unwrap()
            .filter_map(|entry| {
                let path = entry.unwrap().path();
                path.file_name()
                    .and_then(|name| name.to_str())
                    .is_some_and(|name| {
                        name.starts_with("recovery-ultra-plan-") && name.ends_with(".yaml")
                    })
                    .then(|| parse_ultra_plan(&std::fs::read_to_string(path).unwrap()).unwrap())
            })
            .find(|plan| {
                plan.phases.iter().any(|phase| {
                    phase.prompt.contains("- challenge_or_adversary_evidence")
                        && phase.prompt.contains("Repair targets:\n- ")
                        && !phase.prompt.contains("Repair targets:\n- none")
                })
            })
            .expect("phase recovery ultra plan");
        assert!(
            recovery_plan
                .phases
                .iter()
                .any(|phase| phase.prompt.contains("- challenge_or_adversary_evidence"))
        );
        assert!(recovery_plan.phases.iter().any(|phase| {
            phase.prompt.contains("Repair targets:\n- ")
                && !phase.prompt.contains("Repair targets:\n- none")
        }));
        let event_text = std::fs::read_to_string(events).unwrap();
        assert!(event_text.contains("\"event\":\"completion_verify\""));
        assert!(event_text.contains("\"event\":\"ultra_phase_complete\""));
        assert!(!event_text.contains("\"event\":\"ultra_phase_failed\""));
        assert!(event_text.contains("\"event\":\"ultra_final_acceptance_failed\""));
        assert!(event_text.contains("\"event\":\"final_acceptance_repair_start\""));
        assert!(event_text.contains("\"event\":\"final_acceptance_repair_failed\""));
    }

    #[test]
    fn ultra_phase_failure_stop_reason_and_summary_use_relative_recovery_paths() {
        let dir = tempfile::tempdir().unwrap();
        let events = dir.path().join("events.jsonl");
        let mut cfg = config(dir.path().to_path_buf());
        cfg.eval_events_path = Some(events.clone());
        let mut planner = FakeClient::new(vec![
            AssistantReply::text("not a step plan"),
            AssistantReply::text("still not a step plan"),
            AssistantReply::text("no valid step plan"),
        ]);
        let mut execution = FakeClient::new(Vec::new());
        let plan = challenge_ultra_plan();

        let err = run_ultra_plan(&mut planner, &mut execution, &plan, &cfg)
            .unwrap_err()
            .to_string();
        let summary = std::fs::read_to_string(dir.path().join("summary.md")).unwrap();
        let event_text = std::fs::read_to_string(events).unwrap();
        let root = dir.path().display().to_string();

        for text in [&err, &summary, &event_text] {
            assert!(
                !text.contains(&format!("{root}{root}")),
                "workspace root duplicated in:\n{text}"
            );
            assert!(
                text.contains(".anvil/plans/recovery-ultra-plan-"),
                "missing relative recovery yaml path in:\n{text}"
            );
        }
        assert!(
            !err.contains(&format!("{root}/.anvil/plans/recovery-ultra-plan-")),
            "{err}"
        );
        assert!(
            !summary.contains(&format!("{root}/.anvil/plans/recovery-ultra-plan-")),
            "{summary}"
        );
        assert!(
            event_text.contains("\"recovery_ultra_plan_path\":\".anvil/plans/"),
            "{event_text}"
        );
    }

    #[test]
    fn ultra_partial_summary_does_not_cut_recovery_yaml_path_mid_token() {
        let recovery_path = ".anvil/plans/recovery-ultra-plan-project-setup-019f2512-5e1e-7cb3-b18a-e71b939f8810.yaml";
        let command = format!("/run-ultra-plan {recovery_path}");
        let old_cut_offset = command.find(".yaml").unwrap() + ".yam".len();
        let prefix_len = 500usize.saturating_sub(old_cut_offset);
        let reason = format!("{} {command}", "x".repeat(prefix_len.saturating_sub(1)));

        let summary = render_ultra_partial_run_summary(UltraPartialRunSummary {
            completed_phases: &[],
            failed_phase: "project-setup",
            pending_phases: &["final-verify".to_string()],
            failure_kind: "phase_scaffold_error",
            reason: &reason,
            recovery_prompt_path: ".anvil/repairs/repair-project-setup.md",
            recovery_yaml_summary: &format!("Recovery UltraPlan YAML saved: {recovery_path}"),
            prompt_command_summary: "Suggested command: unavailable",
            recovery_yaml_command_summary: &format!("Suggested YAML command: {command}"),
            recovery_artifact_check: "Recovery artifact check: prompt_parse_ok=true, yaml_parse_ok=true, command_targets_valid=true",
            browser_evidence_missing_note: None,
        });
        let failure = summary.split("Failure:\n").nth(1).expect("failure block");

        assert!(
            !failure.contains(
                "recovery-ultra-plan-project-setup-019f2512-5e1e-7cb3-b18a-e71b939f8810.yam"
            ),
            "{failure}"
        );
        for token in failure.split_whitespace() {
            if token.contains("recovery-ultra-plan-project-setup") {
                assert!(token.ends_with(".yaml"), "{token}");
            }
        }
        assert!(
            summary.contains(&format!("Suggested YAML command: {command}")),
            "{summary}"
        );
    }

    #[test]
    fn ultra_plan_final_profile_failure_runs_repair() {
        let dir = tempfile::tempdir().unwrap();
        let step_json = generated_nextjs_artifact_plan_json("Scaffold project");
        let mut planner = FakeClient::new(
            (0..6)
                .map(|_| AssistantReply::text(step_json.clone()))
                .collect(),
        );
        let good_package = nextjs_complete_package_json();
        let bad_package =
            r#"{"dependencies":{},"scripts":{"build":"next build","dev":"next dev -p 3011"}}"#;
        let mut execution = FakeClient::new(vec![
            AssistantReply {
                content: String::new(),
                tool_calls: vec![
                    crate::state::ToolCall::new(
                        "Write",
                        serde_json::json!({"path":"package.json","content":good_package}),
                    ),
                    crate::state::ToolCall::new(
                        "Write",
                        serde_json::json!({"path":"tsconfig.json","content":nextjs_tsconfig_json()}),
                    ),
                    crate::state::ToolCall::new(
                        "Write",
                        serde_json::json!({"path":"postcss.config.js","content":nextjs_postcss_config()}),
                    ),
                    crate::state::ToolCall::new(
                        "Write",
                        serde_json::json!({"path":"tailwind.config.ts","content":nextjs_tailwind_config_ts()}),
                    ),
                    crate::state::ToolCall::new(
                        "Write",
                        serde_json::json!({"path":"src/app/page.tsx","content":"export default function Page(){return <main>Space Invaders</main>;}"}),
                    ),
                    crate::state::ToolCall::new(
                        "Write",
                        serde_json::json!({"path":"src/app/layout.tsx","content":nextjs_layout_source()}),
                    ),
                    crate::state::ToolCall::new(
                        "Write",
                        serde_json::json!({"path":"src/app/globals.css","content":nextjs_globals_css()}),
                    ),
                    crate::state::ToolCall::new(
                        "Write",
                        serde_json::json!({"path":"src/app/global.d.ts","content":"declare module \"*.css\";"}),
                    ),
                ],
                prompt_tokens: None,
                completion_tokens: None,
            },
            AssistantReply {
                content: String::new(),
                tool_calls: vec![crate::state::ToolCall::new(
                    "Write",
                    serde_json::json!({"path":"package.json","content":bad_package}),
                )],
                prompt_tokens: None,
                completion_tokens: None,
            },
            AssistantReply {
                content: String::new(),
                tool_calls: vec![
                    crate::state::ToolCall::new(
                        "Write",
                        serde_json::json!({"path":"package.json","content":good_package}),
                    ),
                    crate::state::ToolCall::new(
                        "Edit",
                        serde_json::json!({
                            "path":"src/app/page.tsx",
                            "old":"Space Invaders",
                            "new":"Space Invaders with keyboard controls, score, waves, and collisions"
                        }),
                    ),
                ],
                prompt_tokens: None,
                completion_tokens: None,
            },
            AssistantReply::text("continuation complete"),
        ]);
        let plan = UltraPlan {
            goal: "3011 port app".to_string(),
            profile: "nextjs".to_string(),
            style: "default".to_string(),
            intent: "create".to_string(),
            phases: vec![
                crate::planner::ultra_plan::UltraPhase {
                    id: "scaffold".to_string(),
                    prompt: "Scaffold project".to_string(),
                },
                crate::planner::ultra_plan::UltraPhase {
                    id: "verify".to_string(),
                    prompt: "Scaffold project".to_string(),
                },
            ],
        };
        let result = run_ultra_plan(
            &mut planner,
            &mut execution,
            &plan,
            &config(dir.path().to_path_buf()),
        )
        .unwrap();
        assert_eq!(result, "ultra-plan-run complete: 2 phases");
        assert!(dir.path().join("src/app/page.tsx").is_file());
        let prompts = execution
            .messages()
            .iter()
            .map(|messages| {
                messages
                    .iter()
                    .map(|message| message.content.as_str())
                    .collect::<Vec<_>>()
                    .join("\n")
            })
            .collect::<Vec<_>>();
        assert!(
            prompts
                .iter()
                .any(|prompt| prompt.contains("Repair the final acceptance failure")),
            "{prompts:#?}"
        );
        assert!(
            prompts
                .iter()
                .any(|prompt| prompt.contains("bounded final acceptance repair")),
            "{prompts:#?}"
        );
        assert!(
            execution.messages().len() >= 3,
            "expected initial phase, follow-up phase, and repair prompts: {prompts:#?}"
        );
    }

    #[test]
    fn ultra_plan_python_cli_profile_runs_compile_repair_and_behavior_probe() {
        let dir = tempfile::tempdir().unwrap();
        let events = dir.path().join("events.jsonl");
        let mut cfg = config(dir.path().to_path_buf());
        cfg.profile = "python-cli".to_string();
        cfg.eval_events_path = Some(events.clone());
        let scaffold_plan = r#"{"goal":"Scaffold Python CSV CLI","steps":[{"id":"scaffold","kind":"setup","expected_result":"pass","instruction":"Create pyproject.toml and src/csv_stats/main.py for a CSV file argument CLI","expected_paths":["pyproject.toml","src/csv_stats/main.py"],"verify":[]}]}"#;
        let deps_plan = r#"{"goal":"Prepare Python CLI dependencies","steps":[{"id":"deps","kind":"verify","expected_result":"pass","instruction":"Verify dependency readiness and syntax","expected_paths":["pyproject.toml","src/csv_stats/main.py"],"verify":["python -m compileall -q src"]}]}"#;
        let implement_plan = r#"{"goal":"Implement Python CSV CLI behavior","steps":[{"id":"implement","kind":"implement","expected_result":"pass","instruction":"Implement the CLI so it reads a CSV path argument, prints aggregate values, and changes output when the input file changes","expected_paths":["src/csv_stats/main.py"],"verify":["python -m compileall -q src"]}]}"#;
        let mut planner = FakeClient::new(vec![
            AssistantReply::text(scaffold_plan),
            AssistantReply::text(deps_plan),
            AssistantReply::text(implement_plan),
        ]);
        let pyproject = r#"[project]
name = "csv-stats"
version = "0.1.0"

[project.scripts]
csv-stats = "csv_stats.main:main"
"#;
        let valid_cli = r#"#!/usr/bin/env python3
import csv
import sys
from pathlib import Path
def fmt(value: float) -> str:
    if value.is_integer():
        return str(int(value))
    return f"{value:.3f}".rstrip("0").rstrip(".")
def main() -> None:
    if len(sys.argv) != 2:
        print("usage: csv-stats <file>", file=sys.stderr)
        raise SystemExit(2)
    path = Path(sys.argv[1])
    if not path.is_file():
        print(f"missing file: {path}", file=sys.stderr)
        raise SystemExit(1)
    with path.open(newline="") as handle:
        rows = list(csv.DictReader(handle))
    numeric = {}
    for column in (rows[0].keys() if rows else []):
        values = []
        for row in rows:
            try:
                values.append(float(row[column]))
            except ValueError:
                pass
        if values:
            numeric[column] = (sum(values), sum(values) / len(values), max(values), min(values))
    if not numeric:
        print("no numeric columns", file=sys.stderr)
        raise SystemExit(1)
    print("column | sum | average | max | min")
    for column in sorted(numeric):
        total, average, maximum, minimum = numeric[column]
        print(f"{column} | {fmt(total)} | {fmt(average)} | {fmt(maximum)} | {fmt(minimum)}")

if __name__ == "__main__":
    main()
"#;
        let invalid_cli = r#"#!/usr/bin/env python3
import sys

def main() -> None:
    path = sys.argv[1]
    if path print(path)

if __name__ == "__main__":
    main()
"#;
        let mut execution = FakeClient::new(vec![
            AssistantReply {
                content: String::new(),
                tool_calls: vec![
                    crate::state::ToolCall::new(
                        "Write",
                        serde_json::json!({"path":"pyproject.toml","content":pyproject}),
                    ),
                    crate::state::ToolCall::new(
                        "Write",
                        serde_json::json!({"path":"src/csv_stats/main.py","content":valid_cli}),
                    ),
                ],
                prompt_tokens: None,
                completion_tokens: None,
            },
            AssistantReply::text("dependency verification complete"),
            AssistantReply {
                content: String::new(),
                tool_calls: vec![crate::state::ToolCall::new(
                    "Write",
                    serde_json::json!({"path":"src/csv_stats/main.py","content":invalid_cli}),
                )],
                prompt_tokens: None,
                completion_tokens: None,
            },
            AssistantReply {
                content: String::new(),
                tool_calls: vec![crate::state::ToolCall::new(
                    "Write",
                    serde_json::json!({"path":"src/csv_stats/main.py","content":valid_cli}),
                )],
                prompt_tokens: None,
                completion_tokens: None,
            },
            AssistantReply {
                content: String::new(),
                tool_calls: vec![crate::state::ToolCall::new(
                    "Write",
                    serde_json::json!({"path":"src/csv_stats/main.py","content":valid_cli}),
                )],
                prompt_tokens: None,
                completion_tokens: None,
            },
            AssistantReply {
                content: String::new(),
                tool_calls: vec![crate::state::ToolCall::new(
                    "Write",
                    serde_json::json!({"path":"src/csv_stats/main.py","content":valid_cli}),
                )],
                prompt_tokens: None,
                completion_tokens: None,
            },
        ]);
        let plan = UltraPlan {
            goal: "Build a Python CLI that reads a CSV file path argument and prints sum, average, max, and min for numeric columns.".to_string(),
            profile: "python-cli".to_string(),
            style: "default".to_string(),
            intent: "create".to_string(),
            phases: vec![
                crate::planner::ultra_plan::UltraPhase {
                    id: "scaffold".to_string(),
                    prompt: "Scaffold the Python CLI".to_string(),
                },
                crate::planner::ultra_plan::UltraPhase {
                    id: "deps".to_string(),
                    prompt: "Prepare dependencies".to_string(),
                },
                crate::planner::ultra_plan::UltraPhase {
                    id: "implement".to_string(),
                    prompt: "Implement CLI behavior".to_string(),
                },
            ],
        };

        let result = run_ultra_plan(&mut planner, &mut execution, &plan, &cfg).unwrap();

        assert_eq!(result, "ultra-plan-run complete: 3 phases");
        let event_text = std::fs::read_to_string(&events).unwrap();
        assert!(event_text.contains("\"event\":\"ultra_phase_scaffold_complete\""));
        assert!(event_text.contains("\"event\":\"ultra_phase_plan_validated\""));
        assert!(event_text.contains("\"event\":\"ultra_phase_execute_complete\""));
        assert!(event_text.contains("\"event\":\"ultra_phase_complete\""));
        assert!(event_text.contains("\"event\":\"dependency_build_lifecycle\""));
        assert!(event_text.contains("\"mode\":\"ultra-plan-run\""));
        assert!(event_text.contains("\"lifecycle_stage\":\"dependency_setup_build\""));
        assert!(event_text.contains("\"lifecycle_stages\""));
        assert!(event_text.contains("\"verification_passed\""));
        assert!(event_text.contains("\"event\":\"step_verify_failure\""));
        assert!(event_text.contains("implementation_compile_error"));
        assert!(event_text.contains("\"event\":\"step_verify_repair\""));
        assert!(event_text.contains("\"event\":\"profile_behavior_probe\""));
        assert!(event_text.contains("\"profile\":\"python-cli\""));
        assert!(event_text.contains("\"profile_behavior_probe_status\":\"pass\""));
        assert!(event_text.contains("\"event\":\"ultra_final_acceptance\""));
        assert!(event_text.contains("\"evidence_arbitration\""));
        assert!(event_text.contains("\"browser_readiness_status\":\"not_applicable\""));
        assert!(event_text.contains("\"interaction_evidence_status\":\"not_applicable\""));
        assert!(event_text.contains("\"event\":\"ultra_plan_complete\""));
        let final_acceptance = latest_event(&events, "ultra_final_acceptance");
        assert_eq!(
            final_acceptance
                .get("profile_behavior_probe_status")
                .and_then(Value::as_str),
            Some("pass")
        );
        let behavior_path = dir.path().join(".anvil/evidence/python-cli-behavior.json");
        let behavior = std::fs::read_to_string(&behavior_path).unwrap();
        let behavior_json: Value = serde_json::from_str(&behavior).unwrap();
        let details = behavior_json.get("details").expect("behavior details");
        assert!(
            behavior.contains("\"changed_by_input\": true"),
            "{behavior}"
        );
        assert_eq!(
            details.get("mode").and_then(Value::as_str),
            Some("csv_file_arg")
        );
        assert_eq!(
            details.get("argv_invocation").and_then(Value::as_bool),
            Some(true)
        );
        let first_stdout = details
            .get("first_stdout")
            .and_then(Value::as_str)
            .unwrap_or_default();
        let second_stdout = details
            .get("second_stdout")
            .and_then(Value::as_str)
            .unwrap_or_default();
        assert!(first_stdout.contains("60"), "{first_stdout}");
        assert!(first_stdout.contains("20"), "{first_stdout}");
        assert!(second_stdout.contains("24"), "{second_stdout}");
        assert!(second_stdout.contains("8"), "{second_stdout}");
        assert!(
            dir.path()
                .join(".anvil/evidence/python-cli-fixtures/input-a.csv")
                .is_file()
        );
        assert!(
            dir.path()
                .join(".anvil/evidence/python-cli-fixtures/input-b.csv")
                .is_file()
        );
    }

    #[test]
    fn ultra_phase_emits_plan_validated_event() {
        let dir = tempfile::tempdir().unwrap();
        let events = dir.path().join("events.jsonl");
        let mut cfg = config(dir.path().to_path_buf());
        cfg.eval_events_path = Some(events.clone());
        let mut planner = FakeClient::new(vec![
            AssistantReply::text(generated_step_plan_json("phase one")),
            AssistantReply::text(generated_step_plan_json("phase two")),
        ]);
        let mut execution = FakeClient::new(vec![]);
        let plan = UltraPlan {
            goal: "Do two phases".to_string(),
            profile: "generic".to_string(),
            style: "default".to_string(),
            intent: "create".to_string(),
            phases: vec![
                crate::planner::ultra_plan::UltraPhase {
                    id: "phase-one".to_string(),
                    prompt: "Phase one".to_string(),
                },
                crate::planner::ultra_plan::UltraPhase {
                    id: "phase-two".to_string(),
                    prompt: "Phase two".to_string(),
                },
            ],
        };
        let result = run_ultra_plan(&mut planner, &mut execution, &plan, &cfg).unwrap();
        assert_eq!(result, "ultra-plan-run complete: 2 phases");
        let event_text = std::fs::read_to_string(events).unwrap();
        assert!(event_text.contains("ultra_phase_plan_validated"));
        assert!(event_text.contains("\"stage\":\"lint\""));
        assert!(event_text.contains("\"step_count\":1"));
    }

    #[test]
    fn ultra_plan_non_final_tailwind_invariant_repair_completes_and_runs_next_phase() {
        let dir = tempfile::tempdir().unwrap();
        let events = dir.path().join("events.jsonl");
        let mut cfg = config(dir.path().to_path_buf());
        cfg.eval_events_path = Some(events.clone());
        write_fake_npm_dependency_installer(dir.path());
        let scaffold_plan = generated_nextjs_fixture_plan_json_with_kind(
            "scaffold phase",
            "check_scaffold.py",
            "setup",
        );
        let finish_plan = generated_nextjs_fixture_plan_json_with_kind(
            "finish phase",
            "check_finish.py",
            "setup",
        );
        let mut planner = FakeClient::new(vec![
            AssistantReply::text(scaffold_plan),
            AssistantReply::text(finish_plan),
        ]);
        let package = nextjs_complete_package_json();
        let mut execution = FakeClient::new(vec![
            AssistantReply {
                content: String::new(),
                tool_calls: vec![
                    crate::state::ToolCall::new(
                        "Write",
                        serde_json::json!({"path":"package.json","content":package}),
                    ),
                    crate::state::ToolCall::new(
                        "Write",
                        serde_json::json!({"path":"tsconfig.json","content":nextjs_tsconfig_json()}),
                    ),
                    crate::state::ToolCall::new(
                        "Write",
                        serde_json::json!({"path":"src/app/page.tsx","content":"export default function Page(){return <main className=\"min-h-screen\">App</main>;}"}),
                    ),
                    crate::state::ToolCall::new(
                        "Write",
                        serde_json::json!({"path":"src/app/layout.tsx","content":"import './globals.css';\nexport default function Layout({children}:{children:React.ReactNode}){return <html><body>{children}</body></html>;}"}),
                    ),
                    crate::state::ToolCall::new(
                        "Write",
                        serde_json::json!({"path":"src/app/global.d.ts","content":"declare module \"*.css\";"}),
                    ),
                    crate::state::ToolCall::new(
                        "Write",
                        serde_json::json!({"path":"src/app/globals.css","content":"@tailwind base;\n@tailwind components;\n@tailwind utilities;\n"}),
                    ),
                    crate::state::ToolCall::new(
                        "Write",
                        serde_json::json!({"path":"tailwind.config.ts","content":nextjs_tailwind_config_ts()}),
                    ),
                    crate::state::ToolCall::new(
                        "Write",
                        serde_json::json!({"path":"postcss.config.js","content":"module.exports = { plugins: { tailwindcss: {} } };\n"}),
                    ),
                    crate::state::ToolCall::new(
                        "Write",
                        serde_json::json!({"path":"check_scaffold.py","content":"x = 1\n"}),
                    ),
                ],
                prompt_tokens: None,
                completion_tokens: None,
            },
            AssistantReply {
                content: String::new(),
                tool_calls: vec![crate::state::ToolCall::new(
                    "Write",
                    serde_json::json!({"path":"check_finish.py","content":"x = 1\n"}),
                )],
                prompt_tokens: None,
                completion_tokens: None,
            },
        ]);
        let plan = UltraPlan {
            goal: "3011 port app".to_string(),
            profile: "nextjs".to_string(),
            style: "default".to_string(),
            intent: "create".to_string(),
            phases: vec![
                crate::planner::ultra_plan::UltraPhase {
                    id: "scaffold".to_string(),
                    prompt: "Scaffold project".to_string(),
                },
                crate::planner::ultra_plan::UltraPhase {
                    id: "finish".to_string(),
                    prompt: "Finish project".to_string(),
                },
            ],
        };

        let result = run_ultra_plan(&mut planner, &mut execution, &plan, &cfg).unwrap();

        assert_eq!(result, "ultra-plan-run complete: 2 phases");
        let postcss = std::fs::read_to_string(dir.path().join("postcss.config.js")).unwrap();
        assert!(postcss.contains("tailwindcss"));
        assert!(postcss.contains("autoprefixer"));
        assert!(dir.path().join("check_finish.py").is_file());
        assert!(!dir.path().join(".anvil/repairs").exists());
        let event_text = std::fs::read_to_string(events).unwrap();
        assert!(event_text.contains("\"event\":\"profile_invariant_repair\""));
        assert!(event_text.contains("\"method\":\"deterministic\""));
        assert!(event_text.contains("\"ok\":true"));
        assert!(event_text.contains("\"event\":\"ultra_plan_complete\""));
    }

    #[test]
    fn ultra_plan_non_final_profile_repair_exhaustion_still_saves_handoff() {
        let dir = tempfile::tempdir().unwrap();
        let events = dir.path().join("events.jsonl");
        let mut cfg = config(dir.path().to_path_buf());
        cfg.profile = "nextjs".to_string();
        cfg.eval_events_path = Some(events.clone());
        write_fake_npm_dependency_installer(dir.path());
        let step_json = generated_nextjs_artifact_plan_json("Scaffold project");
        let mut planner = FakeClient::new(
            (0..3)
                .map(|_| AssistantReply::text(step_json.clone()))
                .collect(),
        );
        let package = nextjs_complete_package_json();
        let mut execution = FakeClient::new(vec![
            AssistantReply {
                content: String::new(),
                tool_calls: vec![
                    crate::state::ToolCall::new(
                        "Write",
                        serde_json::json!({"path":"package.json","content":package}),
                    ),
                    crate::state::ToolCall::new(
                        "Write",
                        serde_json::json!({"path":"postcss.config.js","content":nextjs_postcss_config()}),
                    ),
                    crate::state::ToolCall::new(
                        "Write",
                        serde_json::json!({"path":"tailwind.config.ts","content":nextjs_tailwind_config_ts()}),
                    ),
                    crate::state::ToolCall::new(
                        "Write",
                        serde_json::json!({"path":"src/app/page.tsx","content":"export default function Page(){return <main>App</main>;}"}),
                    ),
                    crate::state::ToolCall::new(
                        "Write",
                        serde_json::json!({"path":"src/app/layout.tsx","content":"export default function Layout({children}:{children:React.ReactNode}){return <html><body>{children}</body></html>;}"}),
                    ),
                    crate::state::ToolCall::new(
                        "Write",
                        serde_json::json!({"path":"src/app/global.d.ts","content":"declare module \"*.css\";"}),
                    ),
                    crate::state::ToolCall::new(
                        "Write",
                        serde_json::json!({"path":"next.config.js","content":"/** @type {import('next').NextConfig} */\nconst nextConfig = {};\n\nmodule.exports = nextConfig;\n"}),
                    ),
                    crate::state::ToolCall::new(
                        "Write",
                        serde_json::json!({"path":"src/app/globals.css","content":"body { margin: 0; }\n"}),
                    ),
                    crate::state::ToolCall::new(
                        "Write",
                        serde_json::json!({"path":"tsconfig.json","content":"{\"compilerOptions\":{\"rootDir\":\"src\"}}\n"}),
                    ),
                ],
                prompt_tokens: None,
                completion_tokens: None,
            },
            AssistantReply::text("model repair made no changes"),
        ]);
        let plan = UltraPlan {
            goal: "3011 port app".to_string(),
            profile: "nextjs".to_string(),
            style: "default".to_string(),
            intent: "create".to_string(),
            phases: vec![
                crate::planner::ultra_plan::UltraPhase {
                    id: "scaffold".to_string(),
                    prompt: "Scaffold project".to_string(),
                },
                crate::planner::ultra_plan::UltraPhase {
                    id: "finish".to_string(),
                    prompt: "Finish project".to_string(),
                },
            ],
        };
        let err = run_ultra_plan(&mut planner, &mut execution, &plan, &cfg)
            .unwrap_err()
            .to_string();
        assert!(
            err.contains("phase scaffold profile invariant verification failed"),
            "{err}"
        );
        assert!(err.contains("tsconfig.rootDir"), "{err}");
        assert!(
            err.contains("/run-ultra-plan .anvil/plans/recovery-ultra-plan-"),
            "{err}"
        );
        assert!(err.contains(".yaml"), "{err}");
        assert!(dir.path().join(".anvil/repairs").is_dir());
        let event_text = std::fs::read_to_string(events).unwrap();
        assert!(event_text.contains("\"event\":\"profile_invariant_repair\""));
        assert!(event_text.contains("\"method\":\"deterministic\""));
        assert!(event_text.contains("\"method\":\"model\""));
        assert!(event_text.contains("\"recovery_handoff_kind\":\"profile_invariant_failure\""));
        assert!(event_text.contains("\"recovery_ultra_plan_path\":\".anvil/plans/"));
    }

    #[test]
    fn ultra_phase_profile_snapshot_runs_before_and_after_phase() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("input")).unwrap();
        std::fs::write(dir.path().join("input/source.csv"), "1234").unwrap();
        let step_json = generated_data_mutation_plan_json("mutate data");
        let mut planner = FakeClient::new(vec![AssistantReply::text(step_json)]);
        let mut execution = FakeClient::new(vec![
            AssistantReply {
                content: String::new(),
                tool_calls: vec![crate::state::ToolCall::new(
                    "Write",
                    serde_json::json!({"path":"input/source.csv","content":"5678"}),
                )],
                prompt_tokens: None,
                completion_tokens: None,
            },
            AssistantReply::text("done"),
        ]);
        let plan = UltraPlan {
            goal: "analyze data".to_string(),
            profile: "data".to_string(),
            style: "default".to_string(),
            intent: "create".to_string(),
            phases: vec![
                crate::planner::ultra_plan::UltraPhase {
                    id: "phase-1".to_string(),
                    prompt: "Mutate data".to_string(),
                },
                crate::planner::ultra_plan::UltraPhase {
                    id: "phase-2".to_string(),
                    prompt: "Report".to_string(),
                },
            ],
        };
        let err = run_ultra_plan(
            &mut planner,
            &mut execution,
            &plan,
            &config(dir.path().to_path_buf()),
        )
        .unwrap_err()
        .to_string();
        assert!(err.contains("profile invariant verification failed"));
        assert!(err.contains("content changed"));
    }

    #[test]
    #[cfg(unix)]
    fn slash_ultra_final_flow_reaches_stop_after_fake_dev_server_cleanup() {
        let _probe_guard = dev_server_probe_test_guard();
        let dir = tempfile::tempdir().unwrap();
        let port = free_local_port();
        let events = dir.path().join(".anvil/runs/fake/events.jsonl");
        enable_dev_server_probe_test_override(dir.path());
        write_fake_nextjs_package_manager(dir.path(), false);
        let mut cfg = config(dir.path().to_path_buf());
        cfg.profile = "nextjs".to_string();
        cfg.eval_events_path = Some(events.clone());
        let plan = UltraPlan {
            goal: explicit_port_goal("Create an interactive browser game", port),
            profile: "nextjs".to_string(),
            style: "default".to_string(),
            intent: "create".to_string(),
            phases: vec![
                UltraPhase {
                    id: "build".to_string(),
                    prompt: "Create the final interactive app".to_string(),
                },
                UltraPhase {
                    id: "final".to_string(),
                    prompt: "Verify final acceptance evidence".to_string(),
                },
            ],
        };
        let mut planner = FakeClient::new(vec![
            AssistantReply::text(render_ultra_plan(&plan)),
            AssistantReply::text(generated_nextjs_artifact_plan_json_with_build_verify(
                "Create the final interactive app",
            )),
            AssistantReply::text(generated_nextjs_artifact_plan_json_with_build_verify(
                "Verify final acceptance evidence",
            )),
        ]);
        let package = format!(
            r#"{{"scripts":{{"build":"next build","dev":"next dev -p {port}","start":"next start -p {port}"}},"dependencies":{{"next":"^14.2.0","react":"^18.3.0","react-dom":"^18.3.0"}},"devDependencies":{{"typescript":"^5.5.0","@types/node":"^20.14.0","@types/react":"^18.3.0","@types/react-dom":"^18.3.0","tailwindcss":"^3.4.19","postcss":"^8.5.15","autoprefixer":"^10.4.20"}}}}"#
        );
        let mut tool_calls = nextjs_interactive_app_tool_calls(interactive_game_page_source());
        tool_calls[0] = crate::state::ToolCall::new(
            "Write",
            serde_json::json!({"path":"package.json","content":package}),
        );
        tool_calls.push(crate::state::ToolCall::new(
            "Write",
            serde_json::json!({
                "path":"interaction-evidence.json",
                "content":"{\"ok\":true,\"interaction_performed\":true,\"start_transition\":true,\"input_state_change\":true,\"input_event_observed\":true,\"state_changed\":true,\"canvas_found\":true}"
            }),
        ));
        let mut execution_replies = vec![
            AssistantReply {
                content: String::new(),
                tool_calls,
                prompt_tokens: None,
                completion_tokens: None,
            },
            AssistantReply {
                content: String::new(),
                tool_calls: vec![crate::state::ToolCall::new(
                    "Write",
                    serde_json::json!({
                        "path":"interaction-evidence.json",
                        "content":"{\"ok\":true,\"interaction_performed\":true,\"start_transition\":true,\"input_state_change\":true,\"input_event_observed\":true,\"state_changed\":true,\"canvas_found\":true}"
                    }),
                )],
                prompt_tokens: None,
                completion_tokens: None,
            },
        ];
        execution_replies.extend(
            (0..8).map(|_| AssistantReply::text("final app artifacts and evidence are ready")),
        );
        let mut execution = FakeClient::new(execution_replies);

        let command = format!(
            "/ultra-plan-run --profile nextjs \"Create an interactive browser game on port {port}\""
        );
        let result = crate::tui::slash::handle_command(
            &command,
            &cfg,
            &mut planner,
            &mut execution,
            &NOOP_UI,
        );
        let output = match result {
            Ok(output) => output,
            Err(err) => {
                let event_text = std::fs::read_to_string(&events).unwrap_or_default();
                panic!(
                    "slash command failed: {err}; planner_requests={}; execution_requests={}; events={event_text}",
                    planner.messages().len(),
                    execution.messages().len()
                );
            }
        };

        assert!(output.contains("ultra-plan-run complete: 2 phases"));
        let event_text = std::fs::read_to_string(&events).unwrap();
        assert!(event_text.contains("\"event\":\"ultra_phase_complete\""));
        assert!(event_text.contains("\"event\":\"dev_server_lifecycle\""));
        assert!(event_text.contains("\"stage\":\"probe\""));
        assert!(event_text.contains("\"stage\":\"cleanup\""));
        assert!(event_text.contains("\"event\":\"ultra_final_acceptance\""));
        assert!(event_text.contains("\"runtime_acceptance_status\":\"pass\""));
        assert!(event_text.contains("\"final_acceptance_status\":\"full_success\""));
        assert!(event_text.contains("\"browser_readiness_status\":\"passed\""));
        assert!(event_text.contains("\"interaction_evidence_status\":\"passed\""));
        assert!(event_text.contains("\"event\":\"tui_command_stop\""));
        assert!(
            dir.path()
                .join(".anvil/runs/fake/browser-readiness.json")
                .is_file()
        );
        let summary = std::fs::read_to_string(dir.path().join(".anvil/runs/fake/summary.md"))
            .expect("summary");
        assert!(summary.contains("Runtime acceptance: pass"));
        assert!(summary.contains("Final acceptance: full_success"));
        assert!(summary.contains("Release gate: pass"));
    }

    #[test]
    #[cfg(unix)]
    fn ultra_final_acceptance_runs_probe_before_behavior_arbitration() {
        let _probe_guard = dev_server_probe_test_guard();
        let dir = tempfile::tempdir().unwrap();
        let port = free_local_port();
        let events = dir.path().join(".anvil/runs/order/events.jsonl");
        enable_dev_server_probe_test_override(dir.path());
        write_probe_nextjs_workspace(dir.path(), port, interactive_game_page_source());
        interaction_probe::write_test_availability_override(dir.path(), true);
        interaction_probe::write_test_result_override(
            dir.path(),
            &interaction_state_missing_probe_result(),
        );
        let mut cfg = config(dir.path().to_path_buf());
        cfg.profile = "nextjs".to_string();
        cfg.eval_events_path = Some(events.clone());
        let plan = UltraPlan {
            goal: explicit_port_goal("Create an interactive browser game", port),
            profile: "nextjs".to_string(),
            style: "default".to_string(),
            intent: "create".to_string(),
            phases: vec![UltraPhase {
                id: "final".to_string(),
                prompt: "Final acceptance".to_string(),
            }],
        };

        let report = ultra_final_acceptance_report(&plan, &cfg).unwrap();

        assert!(!report.is_pass(), "{report:?}");
        assert_eq!(
            classify_repair_target(&report),
            RepairTarget::Implementation
        );
        let event_text = std::fs::read_to_string(&events).unwrap();
        assert!(!event_text.contains("probe_unavailable"), "{event_text}");
        assert!(
            !event_text.contains("/setup-interaction-probe"),
            "{event_text}"
        );
        assert!(
            event_text
                .contains("browser_interaction_failed:input_state_change_missing_after_start")
        );
        let ultra = latest_event(&events, "ultra_final_acceptance");
        assert_eq!(
            ultra
                .get("interaction_evidence_status")
                .and_then(Value::as_str),
            Some("failed:input_state_change_missing_after_start")
        );
        let arbitration = ultra
            .get("evidence_arbitration")
            .and_then(Value::as_object)
            .expect("evidence arbitration");
        let stateful = arbitration
            .get("stateful_update_evidence")
            .and_then(Value::as_object)
            .expect("stateful update arbitration");
        assert_eq!(
            stateful
                .get("behavioral_observation")
                .and_then(Value::as_str),
            Some("input_state_change_missing_after_start")
        );
        assert_eq!(
            stateful.get("final_tier").and_then(Value::as_str),
            Some("absent")
        );
    }

    #[test]
    fn ultra_final_acceptance_binds_generated_completion_contract() {
        let dir = tempfile::tempdir().unwrap();
        let events = dir.path().join("events.jsonl");
        std::fs::create_dir_all(dir.path().join("src/app")).unwrap();
        std::fs::write(
            dir.path().join("package.json"),
            r#"{"scripts":{"build":"next build"},"dependencies":{"next":"latest","react":"latest","react-dom":"latest"}}"#,
        )
        .unwrap();
        std::fs::write(
            dir.path().join("src/app/page.tsx"),
            r#""use client";
import { useEffect, useState } from "react";
export default function Page(){
  const [score, setScore] = useState(0);
  const [gameState, setGameState] = useState("ready");
  const enemies = [{ x: 10, y: 20 }];
  useEffect(() => {
    const onKeyDown = (event: KeyboardEvent) => {
      if (event.key === "ArrowLeft") {
        setGameState("playing");
        setScore((value) => value + 1);
      }
    };
    const frame = requestAnimationFrame(() => {
      const collision = enemies.some((enemy) => enemy.x > 0);
      if (collision) setGameState("gameover");
    });
    window.addEventListener("keydown", onKeyDown);
    return () => {
      cancelAnimationFrame(frame);
      window.removeEventListener("keydown", onKeyDown);
    };
  }, []);
  return <main><button onClick={() => setGameState("playing")}>Start</button><button onClick={() => { setGameState("ready"); setScore(0); }}>Restart</button><canvas /><p>score {score} enemy collision {gameState}</p></main>;
}
"#,
        )
        .unwrap();
        let mut cfg = config(dir.path().to_path_buf());
        cfg.profile = "nextjs".to_string();
        cfg.eval_events_path = Some(events.clone());
        let plan = UltraPlan {
            goal: "Build an interactive browser game".to_string(),
            profile: "nextjs".to_string(),
            style: "default".to_string(),
            intent: "create".to_string(),
            phases: vec![UltraPhase {
                id: "final".to_string(),
                prompt: "Final acceptance".to_string(),
            }],
        };
        let _report = ultra_final_acceptance_report(&plan, &cfg).unwrap();
        let event_text = std::fs::read_to_string(events).unwrap();
        assert!(event_text.contains("\"event\":\"completion_contract_bound\""));
        assert!(event_text.contains("\"session_scope\":\"ultra-plan-run\""));
        assert!(event_text.contains("\"event\":\"ultra_final_acceptance\""));
        assert!(event_text.contains("\"completion_contract_verification_enabled\":true"));
        assert!(event_text.contains("\"external_contract_checked\":true"));
        assert!(event_text.contains("\"completion_contract_generated\":true"));
        assert!(
            dir.path()
                .join("completion-contract-ultra-plan-run.json")
                .is_file()
        );
    }

    #[test]
    fn completion_verify_and_ultra_final_acceptance_emit_matching_evidence_tiers() {
        let dir = tempfile::tempdir().unwrap();
        let events = dir.path().join("events.jsonl");
        let contract = dir.path().join("contract.json");
        std::fs::write(
            &contract,
            serde_json::to_string_pretty(&serde_json::json!({
                "required_paths": ["src/app/page.tsx"],
                "required_evidence": [
                    "implementation_artifact",
                    "visible_interactive_surface_evidence",
                    "user_input_handler_evidence",
                    "stateful_update_evidence",
                    "challenge_or_adversary_evidence",
                    "score_or_progression_evidence",
                    "failure_or_collision_evidence",
                    "restart_or_recoverable_state_evidence"
                ],
                "verify_repair_cap": 1
            }))
            .unwrap(),
        )
        .unwrap();
        let mut cfg = config(dir.path().to_path_buf());
        cfg.eval_events_path = Some(events.clone());
        cfg.completion_contract_path = Some(contract);

        let mut fake = FakeClient::new(vec![AssistantReply {
            content: String::new(),
            tool_calls: vec![crate::state::ToolCall::new(
                "Write",
                serde_json::json!({
                    "path": "src/app/page.tsx",
                    "content": interactive_game_page_source()
                }),
            )],
            prompt_tokens: None,
            completion_tokens: None,
        }]);
        let mut session = SessionSnapshot::new();
        run_session_with_outcome_with_options(
            &mut fake,
            &mut session,
            "Create the contracted game implementation in src/app/page.tsx.",
            &["src/app/page.tsx".to_string()],
            &cfg,
            &NOOP_UI,
            RunSessionOptions::plan_step(RunSessionStepKind::Implement),
        )
        .unwrap();

        let plan = UltraPlan {
            goal: "Create the contracted game implementation".to_string(),
            profile: "generic".to_string(),
            style: "default".to_string(),
            intent: "create".to_string(),
            phases: vec![UltraPhase {
                id: "final".to_string(),
                prompt: "Final acceptance".to_string(),
            }],
        };
        let _report = ultra_final_acceptance_report(&plan, &cfg).unwrap();

        let completion = latest_event(&events, "completion_verify");
        let ultra = latest_event(&events, "ultra_final_acceptance");
        assert_eq!(
            completion.get("evidence_tiers"),
            ultra.get("evidence_tiers"),
            "completion={completion:?}\nultra={ultra:?}"
        );
        assert_eq!(
            ultra
                .get("evidence_tiers")
                .and_then(|tiers| tiers.get("failure_or_collision_evidence"))
                .and_then(Value::as_str),
            Some("strong")
        );
        assert_eq!(
            ultra
                .get("evidence_tiers")
                .and_then(|tiers| tiers.get("restart_or_recoverable_state_evidence"))
                .and_then(Value::as_str),
            Some("strong")
        );
    }

    #[test]
    fn ultra_final_acceptance_records_behavioral_arbitration_per_key() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("src/app")).unwrap();
        std::fs::write(
            dir.path().join("src/app/page.tsx"),
            r#""use client";
import { useState } from "react";
export default function Page() {
  const [score, setScore] = useState(0);
  const [mode, setMode] = useState("ready");
  const fire = () => setScore((value) => value + 1);
  const initGame = () => setMode("ready");
  return <main><button onClick={fire}>Start</button><button onClick={initGame}>Restart</button><canvas />score enemy collision restart {score}{mode}</main>;
}
"#,
        )
        .unwrap();
        let contract = dir.path().join("contract.json");
        std::fs::write(
            &contract,
            serde_json::to_string_pretty(&serde_json::json!({
                "required_paths": ["src/app/page.tsx"],
                "required_evidence": [
                    "implementation_artifact",
                    "visible_interactive_surface_evidence",
                    "interactive_ui_source_evidence",
                    "non_static_screen_evidence",
                    "user_input_handler_evidence",
                    "stateful_update_evidence",
                    "failure_or_collision_evidence",
                    "restart_or_recoverable_state_evidence"
                ],
                "verify_repair_cap": 1
            }))
            .unwrap(),
        )
        .unwrap();
        interaction_probe::write_test_availability_override(dir.path(), true);
        interaction_probe::write_test_result_override(
            dir.path(),
            &serde_json::json!({
                "ok": true,
                "status": "passed",
                "interaction_success": true,
                "interaction_performed": true,
                "input_event_observed": true,
                "state_changed": true,
                "steps": [
                    "surface_visible",
                    "start_transition",
                    "control_input_dispatched",
                    "input_state_change",
                    "recovery_transition"
                ],
                "before_marker": "menu",
                "after_marker": "running",
                "input_before_marker": "running",
                "input_after_marker": "moved",
                "duration_ms": 12
            }),
        );
        let run_dir = dir.path().join(".anvil/runs/behavior");
        let interaction_path = run_dir.join("browser-interaction.json");
        let outcome = interaction_probe::probe_browser_interaction_against_running_server(
            dir.path(),
            34_099,
            &run_dir,
            &interaction_path,
            Duration::from_secs(1),
        );
        assert!(
            outcome
                .observation()
                .is_some_and(|observation| observation.ok),
            "{outcome:?}"
        );

        let events = run_dir.join("events.jsonl");
        let mut cfg = config(dir.path().to_path_buf());
        cfg.eval_events_path = Some(events.clone());
        cfg.completion_contract_path = Some(contract);
        let plan = UltraPlan {
            goal: "Create contracted implementation".to_string(),
            profile: "generic".to_string(),
            style: "default".to_string(),
            intent: "create".to_string(),
            phases: vec![UltraPhase {
                id: "final".to_string(),
                prompt: "Final acceptance".to_string(),
            }],
        };
        let report = ultra_final_acceptance_report(&plan, &cfg).unwrap();
        assert!(!report.is_pass(), "{report:?}");
        let ultra = latest_event(&events, "ultra_final_acceptance");
        assert_eq!(
            ultra
                .get("evidence_arbitration_summary")
                .and_then(Value::as_str),
            Some("behavioral (probe ok)")
        );
        let arbitration = ultra
            .get("evidence_arbitration")
            .and_then(Value::as_object)
            .expect("evidence_arbitration object");
        let collision = arbitration
            .get("failure_or_collision_evidence")
            .and_then(Value::as_object)
            .expect("collision arbitration");
        assert_eq!(
            collision.get("static_tier").and_then(Value::as_str),
            Some("weak")
        );
        assert_eq!(
            collision.get("final_tier").and_then(Value::as_str),
            Some("absent")
        );
        assert_eq!(
            collision.get("decided_by").and_then(Value::as_str),
            Some("behavioral")
        );
        assert_eq!(
            collision
                .get("behavioral_observation")
                .and_then(Value::as_str),
            Some("not_observed_by_probe")
        );
        assert_eq!(
            ultra
                .get("evidence_tiers")
                .and_then(|tiers| tiers.get("failure_or_collision_evidence"))
                .and_then(Value::as_str),
            Some("absent")
        );
        assert_eq!(
            ultra
                .get("missing_evidence")
                .and_then(Value::as_array)
                .and_then(|items| items.first())
                .and_then(Value::as_str),
            Some("failure_or_collision_evidence")
        );
    }

    #[test]
    fn saved_recovery_ultra_plan_can_drive_fixture_recovery_success() {
        let dir = tempfile::tempdir().unwrap();
        let events = dir.path().join("events.jsonl");
        let mut cfg = config(dir.path().to_path_buf());
        cfg.eval_events_path = Some(events.clone());
        let handoff = RecoveryHandoff {
            profile: "generic".to_string(),
            original_goal: "Create app entrypoint".to_string(),
            failed_phase: Some("minimal-loop".to_string()),
            failed_step: Some("completion-verify".to_string()),
            failure_kind: "verify_repair_no_change".to_string(),
            failure_evidence: vec!["src/app/page.tsx is missing".to_string()],
            missing_paths: vec!["src/app/page.tsx".to_string()],
            missing_capabilities: Vec::new(),
            verify_commands: vec!["test -f src/app/page.tsx".to_string()],
            changed_paths: Vec::new(),
            repair_targets: vec!["missing_entrypoint".to_string()],
        };
        let recovery_path =
            save_recovery_ultra_plan(dir.path(), "fixture-recovery", &handoff).unwrap();
        let recovery_plan =
            parse_ultra_plan(&std::fs::read_to_string(&recovery_path).unwrap()).unwrap();
        assert_eq!(
            parse_ultra_plan(&render_ultra_plan(&recovery_plan)).unwrap(),
            recovery_plan
        );
        let inspect_plan =
            serde_json::to_string(&StepPlan::single("Inspect current state")).unwrap();
        let repair_plan = serde_json::to_string(&StepPlan {
            goal: "Repair missing entrypoint".to_string(),
            steps: vec![PlanStep {
                id: "repair-entrypoint".to_string(),
                kind: "implement".to_string(),
                expected_result: "pass".to_string(),
                instruction: "Create src/app/page.tsx".to_string(),
                expected_paths: vec!["src/app/page.tsx".to_string()],
                verify: Vec::new(),
            }],
        })
        .unwrap();
        let verify_plan = serde_json::to_string(&StepPlan {
            goal: "Verify recovered entrypoint".to_string(),
            steps: vec![PlanStep {
                id: "verify-entrypoint".to_string(),
                kind: "verify".to_string(),
                expected_result: "pass".to_string(),
                instruction: "Verify src/app/page.tsx exists".to_string(),
                expected_paths: Vec::new(),
                verify: vec!["test -f src/app/page.tsx".to_string()],
            }],
        })
        .unwrap();
        let mut planner = FakeClient::new(vec![
            AssistantReply::text(inspect_plan),
            AssistantReply::text(repair_plan),
            AssistantReply::text(verify_plan.clone()),
            AssistantReply::text(verify_plan.clone()),
            AssistantReply::text(verify_plan),
        ]);
        let mut execution = FakeClient::new(vec![
            AssistantReply {
                content: String::new(),
                tool_calls: vec![crate::state::ToolCall::new(
                    "Write",
                    serde_json::json!({"path":"src/app/page.tsx","content":"export default function Page(){return <main>recovered</main>;}\n"}),
                )],
                prompt_tokens: None,
                completion_tokens: None,
            },
            AssistantReply {
                content: String::new(),
                tool_calls: vec![crate::state::ToolCall::new(
                    "Bash",
                    serde_json::json!({"command":"true"}),
                )],
                prompt_tokens: None,
                completion_tokens: None,
            },
            AssistantReply::text("verified"),
        ]);
        let result = run_ultra_plan(&mut planner, &mut execution, &recovery_plan, &cfg).unwrap();
        assert_eq!(result, "ultra-plan-run complete: 3 phases");
        assert!(dir.path().join("src/app/page.tsx").is_file());
        let event_text = std::fs::read_to_string(events).unwrap();
        assert!(event_text.contains("\"event\":\"ultra_plan_complete\""));
    }

    #[test]
    fn canvas_surface_gate_uses_ultra_phase_text_and_japanese_tokens() {
        let plan = UltraPlan {
            goal: "最高に面白いブラウザ体験を作る".to_string(),
            profile: "nextjs".to_string(),
            style: "default".to_string(),
            intent: "create".to_string(),
            phases: vec![UltraPhase {
                id: "implement".to_string(),
                prompt: "HTML5 Canvas のプレイ画面を実装する".to_string(),
            }],
        };
        let capabilities = vec!["stateful_interaction".to_string()];

        assert!(requires_canvas_surface(
            &ultra_plan_signal_text(&plan),
            &capabilities
        ));
        assert!(requires_canvas_surface("カンバスを描画する", &capabilities));
    }
}
