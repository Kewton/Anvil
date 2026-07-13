#[cfg(test)]
mod moved {
    use super::super::*;

    #[test]
    fn ultra_final_acceptance_event_carries_generic_static_assurance() {
        let dir = tempfile::tempdir().unwrap();
        let events = dir.path().join("events.jsonl");
        let mut cfg = config(dir.path().to_path_buf());
        cfg.eval_events_path = Some(events.clone());
        cfg.profile = "generic".to_string();
        std::fs::write(
            dir.path().join("memo.jsx"),
            r#"
import { useState } from "react";
export default function Memo() {
  const [notes, setNotes] = useState([]);
  return <form onSubmit={() => setNotes([...notes, "x"])}><input /><button>Add</button></form>;
}
"#,
        )
        .unwrap();
        let plan = UltraPlan::deterministic(
            "ちょっとしたメモアプリを作って",
            "generic",
            "default",
            "create",
        );

        let report = ultra_final_acceptance_report(&plan, &cfg).unwrap();

        assert!(report.is_pass(), "{report:?}");
        let final_acceptance = latest_event(&events, "ultra_final_acceptance");
        assert_eq!(
            final_acceptance
                .get("assurance_level")
                .and_then(Value::as_str),
            Some("static")
        );
        assert_eq!(
            final_acceptance
                .get("assurance_reason")
                .and_then(Value::as_str),
            Some(eval_events::GENERIC_STATIC_ASSURANCE_REASON)
        );
    }

    #[test]
    fn ambiguous_generic_app_promotion_keeps_union_contract_and_earns_full_after_gates() {
        let dir = tempfile::tempdir().unwrap();
        let events = dir.path().join("events.jsonl");
        let mut cfg = config(dir.path().to_path_buf());
        cfg.eval_events_path = Some(events.clone());
        write_fake_npm_dependency_installer(dir.path());
        let goal = "ちょっとしたメモアプリを作って";
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
        assert_eq!(
            promotion.get("contract_origin").and_then(Value::as_str),
            Some("promoted_union")
        );
        let final_acceptance = latest_event(&events, "ultra_final_acceptance");
        assert_eq!(
            final_acceptance
                .get("effective_profile")
                .and_then(Value::as_str),
            Some("nextjs")
        );
        assert_eq!(
            final_acceptance
                .get("contract_origin")
                .and_then(Value::as_str),
            Some("promoted_union")
        );
        for evidence in GENERIC_INTERACTIVE_EVIDENCE_KEYS {
            assert!(
                event_array_contains(&final_acceptance, "required_evidence", evidence),
                "{evidence} missing from {final_acceptance}"
            );
        }
        for evidence in [
            "nextjs_route_evidence",
            "build_command_or_dependency_missing_boundary",
        ] {
            assert!(
                event_array_contains(&final_acceptance, "required_evidence", evidence),
                "{evidence} missing from {final_acceptance}"
            );
        }
        for capability in [
            "stateful_interaction",
            "user_input_or_action",
            "visible_state_change",
        ] {
            assert!(
                event_array_contains(&final_acceptance, "required_capabilities", capability),
                "{capability} missing from {final_acceptance}"
            );
        }
        assert_eq!(
            final_acceptance
                .get("browser_readiness_applicable")
                .and_then(Value::as_bool),
            Some(true)
        );
        assert_eq!(
            final_acceptance
                .get("browser_readiness_execution_status")
                .and_then(Value::as_str),
            Some("performed")
        );
        assert_eq!(
            final_acceptance
                .get("interaction_evidence_applicable")
                .and_then(Value::as_bool),
            Some(true)
        );
        assert_eq!(
            final_acceptance
                .get("interaction_evidence_execution_status")
                .and_then(Value::as_str),
            Some("performed")
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
                .get("assurance_level")
                .and_then(Value::as_str),
            Some("full")
        );
    }

    #[test]
    fn applicable_interaction_gates_disconnected_force_loud_failure() {
        let required_capabilities = vec![
            "stateful_interaction".to_string(),
            "user_input_or_action".to_string(),
            "visible_state_change".to_string(),
        ];
        let required_evidence = vec![
            "user_input_handler_evidence".to_string(),
            "stateful_update_evidence".to_string(),
        ];
        let mut release_gate = ReleaseGateSummary {
            status: "not_applicable".to_string(),
            reasons: Vec::new(),
            browser_readiness_status: "not_applicable".to_string(),
            browser_readiness_evidence_path: String::new(),
            interaction_evidence_status: "skipped".to_string(),
            interaction_evidence_path: String::new(),
        };
        let telemetry = acceptance_gate_telemetry(
            "nextjs",
            "Create an interactive browser app",
            &required_capabilities,
            &required_evidence,
            &release_gate,
        );

        let reason = acceptance_gates_disconnected_reason(&telemetry, &release_gate)
            .expect("disconnected gates should be loud");
        release_gate.status = "failed".to_string();
        release_gate.reasons.push(reason.clone());
        let final_acceptance_status = release_gate_final_acceptance_status(&release_gate);
        let (assurance_level, assurance_reason) = earned_assurance_for_completion(
            "nextjs",
            &required_capabilities,
            true,
            final_acceptance_status,
            &release_gate,
            &telemetry,
            None,
        );

        assert_eq!(final_acceptance_status, "incomplete");
        assert_eq!(assurance_level, "partial");
        assert!(reason.contains("acceptance_gates_disconnected"));
        assert!(reason.contains("browser_readiness_status=not_applicable"));
        assert!(reason.contains("interaction_evidence_status=skipped"));
        assert!(assurance_reason.contains("acceptance_gates_disconnected"));
    }

    #[test]
    fn data_assurance_is_earned_from_the_observed_profile_probe_level() {
        assert_eq!(
            assurance_for_completion("data", &[]),
            ("static", "data_profile_probe_not_run")
        );
        let release_gate = ReleaseGateSummary {
            status: "pass".to_string(),
            reasons: Vec::new(),
            browser_readiness_status: "not_applicable".to_string(),
            browser_readiness_evidence_path: String::new(),
            interaction_evidence_status: "not_applicable".to_string(),
            interaction_evidence_path: String::new(),
        };
        let telemetry = AcceptanceGateTelemetry {
            browser_readiness_applicable: false,
            browser_readiness_execution_status: "not_applicable".to_string(),
            interaction_evidence_applicable: false,
            interaction_evidence_execution_status: "not_applicable".to_string(),
        };
        for (status, expected) in [
            ("pass", "full"),
            ("partial", "partial"),
            ("static", "static"),
            ("failed", "failed"),
        ] {
            let probe = ProfileBehaviorProbeReport {
                status,
                reasons: Vec::new(),
                evidence_path: Some("evidence/data-assurance.json".to_string()),
            };
            let (level, _) = earned_assurance_for_completion(
                "data",
                &[],
                true,
                "full_success",
                &release_gate,
                &telemetry,
                Some(&probe),
            );
            assert_eq!(level, expected, "status={status}");
        }
        let (unearned, _) = earned_assurance_for_completion(
            "data",
            &[],
            true,
            "full_success",
            &release_gate,
            &telemetry,
            None,
        );
        assert_eq!(unearned, "static");
    }

    #[test]
    fn python_cli_failed_behavior_probe_forces_failed_gate_fields() {
        let dir = tempfile::tempdir().unwrap();
        let events = dir.path().join("events.jsonl");
        let mut cfg = config(dir.path().to_path_buf());
        cfg.profile = "python-cli".to_string();
        cfg.eval_events_path = Some(events.clone());
        std::fs::create_dir_all(dir.path().join("src/anvil_app")).unwrap();
        std::fs::write(
            dir.path().join("pyproject.toml"),
            r#"[project]
name = "anvil-app"
version = "0.1.0"

[project.scripts]
csv-stats = "anvil_app.main:main"
"#,
        )
        .unwrap();
        let entrypoint = dir.path().join("src/anvil_app/main.py");
        std::fs::write(
            &entrypoint,
            r#"#!/usr/bin/env python3
import sys

def main() -> None:
    _path = sys.argv[1] if len(sys.argv) > 1 else ""
    print("csv processed")

if __name__ == "__main__":
    main()
"#,
        )
        .unwrap();
        #[cfg(unix)]
        {
            let mut permissions = std::fs::metadata(&entrypoint).unwrap().permissions();
            std::os::unix::fs::PermissionsExt::set_mode(&mut permissions, 0o755);
            std::fs::set_permissions(&entrypoint, permissions).unwrap();
        }
        let plan = UltraPlan {
            goal: "Build a Python CLI that reads a CSV file path argument and prints sum, average, max, and min for numeric columns.".to_string(),
            profile: "python-cli".to_string(),
            style: "default".to_string(),
            intent: "create".to_string(),
            phases: vec![crate::planner::ultra_plan::UltraPhase {
                id: "final".to_string(),
                prompt: "Final acceptance".to_string(),
            }],
        };

        let report = ultra_final_acceptance_report(&plan, &cfg).unwrap();

        assert!(!report.is_pass(), "{report:?}");
        assert!(
            report
                .primary_reason()
                .contains("python_cli_behavior_probe_failed"),
            "{report:?}"
        );
        let final_acceptance = latest_event(&events, "ultra_final_acceptance");
        assert_eq!(
            final_acceptance
                .get("profile_behavior_probe_status")
                .and_then(Value::as_str),
            Some("failed")
        );
        assert_eq!(
            final_acceptance
                .get("runtime_acceptance_passed")
                .and_then(Value::as_bool),
            Some(false)
        );
        assert_eq!(
            final_acceptance
                .get("runtime_acceptance_status")
                .and_then(Value::as_str),
            Some("failed")
        );
        assert_eq!(
            final_acceptance
                .get("release_gate_status")
                .and_then(Value::as_str),
            Some("failed")
        );
        assert_eq!(
            final_acceptance
                .get("final_acceptance_status")
                .and_then(Value::as_str),
            Some("incomplete")
        );
        assert_ne!(
            final_acceptance
                .get("assurance_level")
                .and_then(Value::as_str),
            Some("full")
        );
        let release_gate_reasons = final_acceptance
            .get("release_gate_reasons")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        assert!(
            release_gate_reasons.iter().any(|reason| {
                reason
                    .as_str()
                    .is_some_and(|reason| reason.contains("profile_behavior_probe_failed"))
            }),
            "{release_gate_reasons:?}"
        );
        assert!(
            release_gate_reasons.iter().any(|reason| {
                reason.as_str().is_some_and(|reason| {
                    reason.contains(
                        "profile_behavior_probe_evidence:.anvil/evidence/python-cli-behavior.json",
                    )
                })
            }),
            "{release_gate_reasons:?}"
        );
        let behavior_probe = latest_event(&events, "profile_behavior_probe");
        assert_eq!(
            behavior_probe.get("status").and_then(Value::as_str),
            Some("failed")
        );
    }

    #[test]
    fn plan_run_nextjs_interactive_app_records_partial_release_gate() {
        let dir = tempfile::tempdir().unwrap();
        let events = dir.path().join("events.jsonl");
        let mut cfg = config(dir.path().to_path_buf());
        cfg.profile = "nextjs".to_string();
        cfg.eval_events_path = Some(events.clone());
        let plan = StepPlan {
            goal: "Create an interactive browser game\n\nRequired final artifacts:\n- package.json\n- src/app/page.tsx\n- src/app/layout.tsx\n- src/app/global.d.ts".to_string(),
            steps: vec![PlanStep {
                id: "app".to_string(),
                kind: "implement".to_string(),
                expected_result: "pass".to_string(),
                instruction: "Create the interactive Next.js game app".to_string(),
                expected_paths: vec![
                    "package.json".to_string(),
                    "src/app/page.tsx".to_string(),
                    "src/app/layout.tsx".to_string(),
                    "src/app/global.d.ts".to_string(),
                ],
                verify: Vec::new(),
            }],
        };
        let page = interactive_game_page_source();
        let mut fake = FakeClient::new(vec![AssistantReply {
            content: String::new(),
            tool_calls: vec![
                crate::state::ToolCall::new(
                    "Write",
                    serde_json::json!({"path":"package.json","content":"{\"scripts\":{\"build\":\"next build\"},\"dependencies\":{\"next\":\"^14.2.0\",\"react\":\"^18.3.0\",\"react-dom\":\"^18.3.0\"}}"}),
                ),
                crate::state::ToolCall::new(
                    "Write",
                    serde_json::json!({"path":"src/app/page.tsx","content":page}),
                ),
                crate::state::ToolCall::new(
                    "Write",
                    serde_json::json!({"path":"src/app/layout.tsx","content":"export default function Layout({children}:{children:React.ReactNode}){return <html><body>{children}</body></html>;}"}),
                ),
                crate::state::ToolCall::new(
                    "Write",
                    serde_json::json!({"path":"src/app/global.d.ts","content":"declare module \"*.css\";"}),
                ),
            ],
            prompt_tokens: None,
            completion_tokens: None,
        }]);
        let result = run_step_plan(&mut fake, &plan, &cfg).unwrap();
        assert_eq!(result, "plan-run complete: 1 steps");
        let event_text = std::fs::read_to_string(events).unwrap();
        assert!(event_text.contains("\"ok\":true"));
        assert!(event_text.contains("\"nextjs_route_evidence\""));
        assert!(event_text.contains("\"build_command_or_dependency_missing_boundary\""));
        assert!(event_text.contains("\"release_gate_status\":\"partial\""));
        assert!(event_text.contains("\"final_acceptance_status\":\"partial\""));
        assert!(
            event_text.contains("browser_readiness_or_interaction_evidence_required"),
            "{event_text}"
        );
        let readiness_text = std::fs::read_to_string(dir.path().join("browser-readiness.json"))
            .expect("generated browser readiness evidence");
        assert!(readiness_text.contains("\"lifecycle_stages\""));
        assert!(readiness_text.contains("\"probe_environment\""));
        assert!(readiness_text.contains("\"NODE_ENV\""));
        assert!(event_text.contains("\"event\":\"recovery_prompt_saved\""));
        assert!(event_text.contains("\"recovery_handoff_kind\":\"browser_readiness_missing\""));
        assert!(event_text.contains("\"acceptance_layer\":\"release_gate\""));
        assert!(event_text.contains("\"suggested_recovery_yaml_command\""));
        assert!(event_text.contains("\"handoff_saved_not_success\":true"));
        let recovery_plan = assert_single_recovery_ultra_plan(dir.path());
        assert_eq!(recovery_plan.goal, plan.goal);
        let recovery_text = render_ultra_plan(&recovery_plan);
        assert!(recovery_text.contains("Failed acceptance layer or phase"));
        assert!(recovery_text.contains("browser_readiness_missing"));
        assert!(recovery_text.contains("Preferred verify/browser check"));
    }

    #[test]
    fn plan_run_nextjs_browser_http_500_fails_final_contract() {
        let dir = tempfile::tempdir().unwrap();
        let events = dir.path().join("events.jsonl");
        let mut cfg = config(dir.path().to_path_buf());
        cfg.profile = "nextjs".to_string();
        cfg.eval_events_path = Some(events.clone());
        std::fs::write(
            dir.path().join("browser-readiness.json"),
            r#"{"ok":false,"http_status":500,"failure_kind":"browser_http_500"}"#,
        )
        .unwrap();
        let plan = StepPlan {
            goal: "Create an interactive browser game\n\nRequired final artifacts:\n- package.json\n- src/app/page.tsx\n- src/app/layout.tsx\n- src/app/global.d.ts".to_string(),
            steps: vec![PlanStep {
                id: "app".to_string(),
                kind: "implement".to_string(),
                expected_result: "pass".to_string(),
                instruction: "Create the interactive Next.js game app".to_string(),
                expected_paths: vec![
                    "package.json".to_string(),
                    "src/app/page.tsx".to_string(),
                    "src/app/layout.tsx".to_string(),
                    "src/app/global.d.ts".to_string(),
                ],
                verify: Vec::new(),
            }],
        };
        let page = interactive_game_page_source();
        let mut fake = FakeClient::new(vec![AssistantReply {
            content: String::new(),
            tool_calls: nextjs_interactive_app_tool_calls(page),
            prompt_tokens: None,
            completion_tokens: None,
        }]);
        let err = run_step_plan(&mut fake, &plan, &cfg)
            .unwrap_err()
            .to_string();
        assert!(err.contains("plan final contract failed"), "{err}");
        assert!(err.contains("release gate failed"), "{err}");
        assert!(err.contains("browser_readiness_failed:http_500"), "{err}");
        let event_text = std::fs::read_to_string(events).unwrap();
        assert!(event_text.contains("\"release_gate_status\":\"failed\""));
        assert!(event_text.contains("\"browser_readiness_status\":\"failed:http_500\""));
        assert!(event_text.contains("\"interaction_evidence_status\":\"not_exercised:http_500\""));
        assert!(event_text.contains("\"event\":\"recovery_prompt_saved\""));
        assert!(event_text.contains("\"recovery_handoff_kind\":\"browser_readiness_failed\""));
        assert!(event_text.contains("\"acceptance_layer\":\"release_gate\""));
        assert!(event_text.contains("\"recovery_handoff_saved\":true"));
        let recovery_plan = assert_single_recovery_ultra_plan(dir.path());
        assert_eq!(recovery_plan.goal, plan.goal);
        let recovery_text = render_ultra_plan(&recovery_plan);
        assert!(recovery_text.contains("release gate reason"));
        assert!(recovery_text.contains("browser readiness"));
        assert!(recovery_text.contains("Preferred verify/browser check"));
    }

    #[test]
    fn release_gate_marks_interaction_not_exercised_after_build_readiness_failure() {
        let dir = tempfile::tempdir().unwrap();
        let cfg = config(dir.path().to_path_buf());
        std::fs::write(
            dir.path().join("browser-readiness.json"),
            r#"{"status":"failed","ok":false,"failure_kind":"build_verifier_failed","output_excerpt":"./src/app/page.tsx:6:3\nType error: Expected 3 arguments, but got 4.\n"}"#,
        )
        .unwrap();

        let gate = browser_release_gate(&cfg);

        assert_eq!(gate.status, "failed");
        assert_eq!(
            gate.browser_readiness_status,
            "failed:build_verifier_failed"
        );
        assert_eq!(
            gate.interaction_evidence_status,
            "not_exercised:build_verifier_failed"
        );
        let evidence = release_recovery_failure_evidence(
            "nextjs",
            "Create a browser app",
            &gate,
            "failed",
            "release gate failed",
            None,
        )
        .join("\n");
        assert!(evidence.contains("interaction evidence: not_exercised:build_verifier_failed"));
        assert!(!evidence.contains("probe_unavailable"), "{evidence}");
        assert!(
            !evidence.contains("interaction_evidence_missing"),
            "{evidence}"
        );
    }

    #[test]
    fn plan_run_nextjs_tailwind_dev_route_failure_keeps_failure_kind() {
        let dir = tempfile::tempdir().unwrap();
        let events = dir.path().join("events.jsonl");
        let mut cfg = config(dir.path().to_path_buf());
        cfg.profile = "nextjs".to_string();
        cfg.eval_events_path = Some(events.clone());
        std::fs::write(
            dir.path().join("browser-readiness.json"),
            r#"{"ok":false,"http_status":500,"browser_failure_kind":"tailwind_dev_pipeline_failure","body_excerpt":"Module parse failed: Unexpected character '@' (1:0)\n> @tailwind base;"}"#,
        )
        .unwrap();
        let plan = StepPlan {
            goal: "Create an interactive browser game\n\nRequired final artifacts:\n- package.json\n- src/app/page.tsx\n- src/app/layout.tsx\n- src/app/global.d.ts".to_string(),
            steps: vec![PlanStep {
                id: "app".to_string(),
                kind: "implement".to_string(),
                expected_result: "pass".to_string(),
                instruction: "Create the interactive Next.js game app".to_string(),
                expected_paths: vec![
                    "package.json".to_string(),
                    "src/app/page.tsx".to_string(),
                    "src/app/layout.tsx".to_string(),
                    "src/app/global.d.ts".to_string(),
                ],
                verify: Vec::new(),
            }],
        };
        let page = interactive_game_page_source();
        let mut fake = FakeClient::new(vec![AssistantReply {
            content: String::new(),
            tool_calls: nextjs_interactive_app_tool_calls(page),
            prompt_tokens: None,
            completion_tokens: None,
        }]);
        let err = run_step_plan(&mut fake, &plan, &cfg)
            .unwrap_err()
            .to_string();
        assert!(err.contains("release gate failed"), "{err}");
        assert!(
            err.contains("browser_readiness_failed:tailwind_dev_pipeline_failure"),
            "{err}"
        );
        let event_text = std::fs::read_to_string(events).unwrap();
        assert!(event_text.contains("\"release_gate_status\":\"failed\""));
        assert!(
            event_text
                .contains("\"browser_readiness_status\":\"failed:tailwind_dev_pipeline_failure\""),
            "{event_text}"
        );
    }

    #[test]
    fn plan_run_nextjs_browser_ready_without_interaction_is_partial() {
        let dir = tempfile::tempdir().unwrap();
        let events = dir.path().join("events.jsonl");
        let mut cfg = config(dir.path().to_path_buf());
        cfg.profile = "nextjs".to_string();
        cfg.eval_events_path = Some(events.clone());
        std::fs::write(
            dir.path().join("browser-readiness.json"),
            r#"{"ok":true,"http_status":200,"route_rendered":true}"#,
        )
        .unwrap();
        let plan = StepPlan {
            goal: "Create an interactive browser game\n\nRequired final artifacts:\n- package.json\n- src/app/page.tsx\n- src/app/layout.tsx\n- src/app/global.d.ts".to_string(),
            steps: vec![PlanStep {
                id: "app".to_string(),
                kind: "implement".to_string(),
                expected_result: "pass".to_string(),
                instruction: "Create the interactive Next.js game app".to_string(),
                expected_paths: vec![
                    "package.json".to_string(),
                    "src/app/page.tsx".to_string(),
                    "src/app/layout.tsx".to_string(),
                    "src/app/global.d.ts".to_string(),
                ],
                verify: Vec::new(),
            }],
        };
        let page = interactive_game_page_source();
        let mut fake = FakeClient::new(vec![AssistantReply {
            content: String::new(),
            tool_calls: nextjs_interactive_app_tool_calls(page),
            prompt_tokens: None,
            completion_tokens: None,
        }]);
        let result = run_step_plan(&mut fake, &plan, &cfg).unwrap();
        assert_eq!(result, "plan-run complete: 1 steps");
        let event_text = std::fs::read_to_string(events).unwrap();
        assert!(event_text.contains("\"release_gate_status\":\"partial\""));
        assert!(event_text.contains("\"browser_readiness_status\":\"passed\""));
        assert!(
            event_text.contains(
                "\"interaction_evidence_status\":\"unavailable:playwright_not_installed\""
            ),
            "{event_text}"
        );
        assert!(event_text.contains("interaction_unverified:probe_unavailable"));
        assert!(!event_text.contains("browser_interaction_evidence_required"));
    }

    #[test]
    #[cfg(unix)]
    fn fake_interaction_probe_success_passes_release_gate() {
        let _probe_guard = dev_server_probe_test_guard();
        let dir = tempfile::tempdir().unwrap();
        let port = free_local_port();
        let events = dir.path().join(".anvil/runs/fake/events.jsonl");
        write_fake_nextjs_dev_workspace(dir.path(), port, false);
        interaction_probe::write_test_availability_override(dir.path(), true);
        interaction_probe::write_test_result_override(
            dir.path(),
            &serde_json::json!({
                "ok": true,
                "status": "passed",
                "start_transition": true,
                "input_state_evaluated_after_start": true,
                "input_state_change": true,
                "state_changed": true,
                "visible_state_changed": true,
                "steps": ["surface_visible", "start_transition", "control_input_dispatched", "input_state_evaluated_after_start", "input_state_change"],
                "before_marker": "menu",
                "after_marker": "running",
                "duration_ms": 11
            }),
        );
        let mut cfg = config(dir.path().to_path_buf());
        cfg.profile = "nextjs".to_string();
        cfg.eval_events_path = Some(events);
        let evidence_path = nextjs_dev_route_evidence_path(&cfg);

        let readiness = run_nextjs_dev_route_probe_with_runtime(
            &cfg,
            &evidence_path,
            true,
            cleanup_dev_server_child,
            BrowserInteractionProbeOptions::default(),
            Some(port),
        );

        assert_eq!(readiness.get("ok").and_then(Value::as_bool), Some(true));
        let interaction_path = evidence_path.with_file_name("browser-interaction.json");
        assert!(interaction_path.is_file(), "run interaction evidence");
        assert!(
            interaction_probe::browser_interaction_evidence_path(dir.path()).is_file(),
            "workspace interaction evidence"
        );
        let gate = browser_release_gate(&cfg);
        assert_eq!(gate.status, "pass", "{gate:?}");
        assert_eq!(gate.interaction_evidence_status, "passed");
    }

    #[test]
    #[cfg(unix)]
    fn fake_interaction_probe_failure_fails_gate_without_evidence_repair_target() {
        let _probe_guard = dev_server_probe_test_guard();
        let dir = tempfile::tempdir().unwrap();
        let port = free_local_port();
        let events = dir.path().join(".anvil/runs/fake/events.jsonl");
        write_fake_nextjs_dev_workspace(dir.path(), port, false);
        interaction_probe::write_test_availability_override(dir.path(), true);
        interaction_probe::write_test_result_override(
            dir.path(),
            &serde_json::json!({
                "ok": false,
                "status": "failed",
                "steps": ["surface_visible", "control_input_dispatched"],
                "before_marker": "menu",
                "after_marker": "menu",
                "failure_kind": "start_transition_missing",
                "duration_ms": 13
            }),
        );
        let mut cfg = config(dir.path().to_path_buf());
        cfg.profile = "nextjs".to_string();
        cfg.eval_events_path = Some(events);
        let evidence_path = nextjs_dev_route_evidence_path(&cfg);

        let readiness = run_nextjs_dev_route_probe_with_runtime(
            &cfg,
            &evidence_path,
            true,
            cleanup_dev_server_child,
            BrowserInteractionProbeOptions::default(),
            Some(port),
        );

        assert_eq!(readiness.get("ok").and_then(Value::as_bool), Some(true));
        let gate = browser_release_gate(&cfg);
        assert_eq!(gate.status, "failed", "{gate:?}");
        assert_eq!(
            gate.reasons,
            vec!["browser_interaction_failed:start_transition_missing".to_string()]
        );
        assert_eq!(
            release_recovery_repair_targets(&gate, None),
            vec!["start_control_wiring".to_string()]
        );

        let mut report = VerificationReport::pass();
        report.push_profile_failure(format!("release gate failed: {}", gate.reasons.join("; ")));
        append_release_gate_observation_failures(&mut report, &gate);
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
            "capability_missing",
            &["src/app/page.tsx".to_string()],
            &[],
            (1, 2),
            false,
            false,
        );
        assert!(prompt.contains("start_transition_missing"), "{prompt}");
        assert!(!prompt.contains("interaction_evidence_missing"), "{prompt}");
        assert!(
            !prompt.contains("browser_interaction_evidence_required"),
            "{prompt}"
        );
        assert!(!prompt.contains("interaction evidence status"), "{prompt}");
    }

    #[test]
    #[cfg(unix)]
    #[ignore = "existing probe-unavailable partial tests cover this without intermediate phase repair"]
    fn probe_unavailable_environment_remains_partial_without_final_repair() {
        let _probe_guard = dev_server_probe_test_guard();
        let dir = tempfile::tempdir().unwrap();
        let port = free_local_port();
        let events = dir
            .path()
            .join(".anvil/runs/probe-unavailable/events.jsonl");
        enable_dev_server_probe_test_override(dir.path());
        write_probe_nextjs_workspace(dir.path(), port, interactive_game_page_source());
        interaction_probe::write_test_availability_override(dir.path(), false);
        let mut cfg = config(dir.path().to_path_buf());
        cfg.profile = "nextjs".to_string();
        cfg.eval_events_path = Some(events.clone());
        let scaffold_plan = generated_nextjs_fixture_plan_json_with_kind(
            "Create buildable app",
            "check_scaffold.py",
            "setup",
        );
        let mut planner = FakeClient::new(vec![
            AssistantReply::text(scaffold_plan),
            AssistantReply::text(final_marker_implement_step_plan_json()),
        ]);
        let mut execution = FakeClient::new(vec![
            probe_nextjs_scaffold_reply(port, interactive_game_page_source().to_string()),
            probe_nextjs_scaffold_reply(port, interactive_game_page_variant(1)),
            probe_nextjs_scaffold_reply(port, interactive_game_page_variant(2)),
            probe_nextjs_scaffold_reply(port, interactive_game_page_variant(3)),
        ]);
        let plan = UltraPlan {
            goal: "Create an interactive browser game".to_string(),
            profile: "nextjs".to_string(),
            style: "default".to_string(),
            intent: "create".to_string(),
            phases: vec![
                UltraPhase {
                    id: "first".to_string(),
                    prompt: "First implementation pass".to_string(),
                },
                UltraPhase {
                    id: "final".to_string(),
                    prompt: "Final implementation pass".to_string(),
                },
            ],
        };

        let result = run_ultra_plan(&mut planner, &mut execution, &plan, &cfg).unwrap();

        assert_eq!(result, "ultra-plan-run complete: 2 phases");
        let event_text = std::fs::read_to_string(&events).unwrap();
        assert!(event_text.contains("\"final_acceptance_status\":\"partial\""));
        assert!(event_text.contains("interaction_unverified:probe_unavailable"));
        assert!(event_text.contains("/setup-interaction-probe"));
        assert!(!event_text.contains("\"event\":\"final_acceptance_repair_start\""));
    }

    #[test]
    fn interaction_probe_infrastructure_failure_blocks_release_without_static_override() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("src/app")).unwrap();
        std::fs::write(
            dir.path().join("src/app/page.tsx"),
            interactive_game_page_source(),
        )
        .unwrap();
        std::fs::write(
            dir.path().join("browser-readiness.json"),
            r#"{"ok":true,"http_status":200,"route_rendered":true}"#,
        )
        .unwrap();
        std::fs::write(
            dir.path().join("browser-interaction.json"),
            serde_json::to_string_pretty(&serde_json::json!({
                "ok": false,
                "status": "failed",
                "interaction_success": false,
                "stage": "resolving",
                "steps": [],
                "failure_kind": "probe_dependency_missing:playwright_module_missing",
                "remediation": crate::minimal_loop::interaction_probe::INTERACTION_PROBE_SETUP_REMEDIATION
            }))
            .unwrap(),
        )
        .unwrap();
        let mut cfg = config(dir.path().to_path_buf());
        cfg.profile = "nextjs".to_string();

        let gate = browser_release_gate(&cfg);
        assert_eq!(gate.status, "failed", "{gate:?}");
        assert!(
            gate.reasons
                .contains(&"probe_dependency_missing:playwright_module_missing".to_string()),
            "{gate:?}"
        );
        assert!(
            gate.reasons
                .iter()
                .any(|reason| reason.contains("app interaction untested")),
            "{gate:?}"
        );
        assert!(
            gate.reasons
                .iter()
                .any(|reason| reason.contains("/setup-interaction-probe")),
            "{gate:?}"
        );
        assert_eq!(
            release_recovery_repair_targets(&gate, None),
            vec!["release_acceptance".to_string()]
        );

        let report = verify_runtime_acceptance_with_browser_dirs_and_hints(
            dir.path(),
            &["src/app/page.tsx".to_string()],
            &[],
            &["browser_interaction".to_string()],
            &[],
            &[],
            &[],
            &[],
            &[],
        );
        assert!(report.passed, "{report:?}");
        assert!(
            !report
                .missing_evidence
                .iter()
                .any(|evidence| evidence.contains("browser_interaction_failed")),
            "{report:?}"
        );
    }

    #[test]
    #[cfg(unix)]
    fn surface_fit_overflow_is_telemetry_not_gate_failure() {
        let dir = tempfile::tempdir().unwrap();
        let port = free_local_port();
        let events = dir.path().join(".anvil/runs/surface-fit/events.jsonl");
        write_probe_nextjs_workspace(
            dir.path(),
            port,
            &interactive_game_page_with_restart_hook_source(),
        );
        let run_dir = events.parent().unwrap();
        std::fs::create_dir_all(run_dir).unwrap();
        std::fs::write(
            run_dir.join("browser-readiness.json"),
            r#"{"ok":true,"status":"passed","http_status":200,"route_rendered":true}"#,
        )
        .unwrap();
        let mut interaction = interaction_state_changed_probe_result();
        interaction["surface_fit"] = serde_json::json!({
            "surface": "canvas",
            "fits_viewport": false,
            "overflow_top_px": 0,
            "overflow_right_px": 22,
            "overflow_bottom_px": 0,
            "overflow_left_px": 0,
            "viewport_width_px": 390,
            "viewport_height_px": 844,
            "rect_width_px": 412,
            "rect_height_px": 600
        });
        std::fs::write(
            run_dir.join("browser-interaction.json"),
            serde_json::to_string_pretty(&interaction).unwrap(),
        )
        .unwrap();
        let mut cfg = config(dir.path().to_path_buf());
        cfg.profile = "nextjs".to_string();
        cfg.eval_events_path = Some(events.clone());
        let plan = UltraPlan {
            goal: explicit_port_goal("Create an interactive browser game with restart flow", port),
            profile: "nextjs".to_string(),
            style: "default".to_string(),
            intent: "create".to_string(),
            phases: vec![UltraPhase {
                id: "final".to_string(),
                prompt: "Final acceptance".to_string(),
            }],
        };

        let report = ultra_final_acceptance_report(&plan, &cfg).unwrap();

        assert!(report.is_pass(), "{report:?}");
        let event = latest_event(&events, "ultra_final_acceptance");
        assert_eq!(
            event.get("release_gate_status").and_then(Value::as_str),
            Some("pass")
        );
        assert_eq!(
            event
                .get("surface_fit")
                .and_then(|fit| fit.get("fits_viewport"))
                .and_then(Value::as_bool),
            Some(false)
        );
        assert_eq!(
            event.get("surface_fit_guidance").and_then(Value::as_str),
            Some("canvas overflows the viewport by 22px; consider responsive sizing")
        );
    }

    #[test]
    fn non_interactive_contract_does_not_require_interaction_evidence() {
        let dir = tempfile::tempdir().unwrap();
        let cfg = config(dir.path().to_path_buf());
        let report = RuntimeAcceptanceReport {
            passed: true,
            primary_reason: "pass".to_string(),
            ..RuntimeAcceptanceReport::default()
        };

        let gate = final_acceptance_release_gate(
            &cfg,
            "nextjs",
            "Create a static about page",
            &[],
            Some(&report),
            true,
        );

        assert_eq!(gate.status, "pass");
        assert_eq!(gate.browser_readiness_status, "not_applicable");
        assert_eq!(gate.interaction_evidence_status, "not_applicable");
    }

    #[test]
    fn canvas_goal_without_canvas_surface_marker_is_partial_release_gate() {
        let dir = tempfile::tempdir().unwrap();
        let cfg = config(dir.path().to_path_buf());
        std::fs::write(
            dir.path().join("browser-readiness.json"),
            r#"{"ok":true,"http_status":200,"route_rendered":true,"has_canvas":false,"interactive_control_count":1,"title_text_excerpt":"Start"}"#,
        )
        .unwrap();
        let report = RuntimeAcceptanceReport {
            passed: true,
            primary_reason: "pass".to_string(),
            ..RuntimeAcceptanceReport::default()
        };

        let gate = final_acceptance_release_gate(
            &cfg,
            "nextjs",
            "Create a canvas-based interactive browser game",
            &["player_control".to_string()],
            Some(&report),
            true,
        );

        assert_eq!(gate.status, "partial", "{gate:?}");
        assert_eq!(gate.browser_readiness_status, "passed");
        assert!(gate.interaction_evidence_status.starts_with("unavailable:"));
        assert!(
            gate.reasons
                .iter()
                .any(|reason| reason.contains("rendered_without_expected_surface")),
            "{gate:?}"
        );
    }

    #[test]
    fn interaction_dom_surface_supersedes_empty_ssr_surface_marker() {
        let dir = tempfile::tempdir().unwrap();
        let cfg = config(dir.path().to_path_buf());
        std::fs::write(
            dir.path().join("browser-readiness.json"),
            r#"{"ok":true,"http_status":200,"route_rendered":true,"ssr_has_canvas":false,"ssr_interactive_control_count":0,"has_canvas":false,"interactive_control_count":0,"route_rendered_quality":"rendered_without_expected_surface"}"#,
        )
        .unwrap();
        std::fs::write(
            dir.path().join("browser-interaction.json"),
            r#"{"ok":true,"status":"passed","interaction_success":true,"interaction_performed":true,"start_transition":true,"input_state_change":true,"input_event_observed":true,"state_changed":true,"post_js_has_canvas":true,"post_js_interactive_control_count":1,"canvas_found":true}"#,
        )
        .unwrap();
        let report = RuntimeAcceptanceReport {
            passed: true,
            primary_reason: "pass".to_string(),
            ..RuntimeAcceptanceReport::default()
        };

        let gate = final_acceptance_release_gate(
            &cfg,
            "nextjs",
            "Create a canvas-based interactive browser game",
            &["player_control".to_string()],
            Some(&report),
            true,
        );

        assert_eq!(gate.status, "pass", "{gate:?}");
        assert_eq!(gate.interaction_evidence_status, "passed");
        assert!(
            !gate
                .reasons
                .iter()
                .any(|reason| reason.contains("rendered_without_expected_surface")),
            "{gate:?}"
        );
    }

    #[test]
    fn plan_run_nextjs_browser_and_interaction_evidence_passes_release_gate() {
        let dir = tempfile::tempdir().unwrap();
        let events = dir.path().join("events.jsonl");
        let mut cfg = config(dir.path().to_path_buf());
        cfg.profile = "nextjs".to_string();
        cfg.eval_events_path = Some(events.clone());
        std::fs::write(
            dir.path().join("browser-readiness.json"),
            r#"{"ok":true,"http_status":200,"route_rendered":true}"#,
        )
        .unwrap();
        std::fs::write(
            dir.path().join("interaction-evidence.json"),
            r#"{"ok":true,"interaction_performed":true,"start_transition":true,"input_state_change":true,"input_event_observed":true,"state_changed":true,"canvas_found":true}"#,
        )
        .unwrap();
        let plan = StepPlan {
            goal: "Create an interactive browser game\n\nRequired final artifacts:\n- package.json\n- src/app/page.tsx\n- src/app/layout.tsx\n- src/app/global.d.ts".to_string(),
            steps: vec![PlanStep {
                id: "app".to_string(),
                kind: "implement".to_string(),
                expected_result: "pass".to_string(),
                instruction: "Create the interactive Next.js game app".to_string(),
                expected_paths: vec![
                    "package.json".to_string(),
                    "src/app/page.tsx".to_string(),
                    "src/app/layout.tsx".to_string(),
                    "src/app/global.d.ts".to_string(),
                ],
                verify: Vec::new(),
            }],
        };
        let page = interactive_game_page_source();
        let mut fake = FakeClient::new(vec![AssistantReply {
            content: String::new(),
            tool_calls: nextjs_interactive_app_tool_calls(page),
            prompt_tokens: None,
            completion_tokens: None,
        }]);
        let result = run_step_plan(&mut fake, &plan, &cfg).unwrap();
        assert_eq!(result, "plan-run complete: 1 steps");
        let event_text = std::fs::read_to_string(events).unwrap();
        assert!(event_text.contains("\"release_gate_status\":\"pass\""));
        assert!(event_text.contains("\"final_acceptance_status\":\"full_success\""));
        assert!(event_text.contains("\"browser_readiness_status\":\"passed\""));
        assert!(event_text.contains("\"interaction_evidence_status\":\"passed\""));
    }

    #[test]
    fn plan_run_nextjs_browser_ok_without_render_detail_is_partial() {
        let dir = tempfile::tempdir().unwrap();
        let events = dir.path().join("events.jsonl");
        let mut cfg = config(dir.path().to_path_buf());
        cfg.profile = "nextjs".to_string();
        cfg.eval_events_path = Some(events.clone());
        std::fs::write(
            dir.path().join("browser-readiness.json"),
            r#"{"ok":true,"http_status":200}"#,
        )
        .unwrap();
        std::fs::write(
            dir.path().join("interaction-evidence.json"),
            r#"{"ok":true,"interaction_performed":true,"start_transition":true,"input_state_change":true,"state_changed":true,"canvas_found":true}"#,
        )
        .unwrap();
        let plan = StepPlan {
            goal: "Create an interactive browser game\n\nRequired final artifacts:\n- package.json\n- src/app/page.tsx\n- src/app/layout.tsx\n- src/app/global.d.ts".to_string(),
            steps: vec![PlanStep {
                id: "app".to_string(),
                kind: "implement".to_string(),
                expected_result: "pass".to_string(),
                instruction: "Create the interactive Next.js game app".to_string(),
                expected_paths: vec![
                    "package.json".to_string(),
                    "src/app/page.tsx".to_string(),
                    "src/app/layout.tsx".to_string(),
                    "src/app/global.d.ts".to_string(),
                ],
                verify: Vec::new(),
            }],
        };
        let page = interactive_game_page_source();
        let mut fake = FakeClient::new(vec![AssistantReply {
            content: String::new(),
            tool_calls: nextjs_interactive_app_tool_calls(page),
            prompt_tokens: None,
            completion_tokens: None,
        }]);
        let result = run_step_plan(&mut fake, &plan, &cfg).unwrap();
        assert_eq!(result, "plan-run complete: 1 steps");
        let event_text = std::fs::read_to_string(events).unwrap();
        assert!(event_text.contains("\"release_gate_status\":\"partial\""));
        assert!(event_text.contains("\"final_acceptance_status\":\"partial\""));
        assert!(
            event_text.contains(
                "\"browser_readiness_status\":\"unavailable:browser_render_evidence_missing\""
            ),
            "{event_text}"
        );
    }

    #[test]
    fn plan_run_nextjs_canvas_unavailable_fails_release_gate() {
        let dir = tempfile::tempdir().unwrap();
        let events = dir.path().join("events.jsonl");
        let mut cfg = config(dir.path().to_path_buf());
        cfg.profile = "nextjs".to_string();
        cfg.eval_events_path = Some(events.clone());
        std::fs::write(
            dir.path().join("browser-readiness.json"),
            r#"{"ok":true,"http_status":200,"route_rendered":true}"#,
        )
        .unwrap();
        std::fs::write(
            dir.path().join("interaction-evidence.json"),
            r#"{"ok":true,"interaction_performed":true,"start_transition":true,"input_state_change":true,"state_changed":true,"canvas_found":false}"#,
        )
        .unwrap();
        let plan = StepPlan {
            goal: "Create an interactive browser game\n\nRequired final artifacts:\n- package.json\n- src/app/page.tsx\n- src/app/layout.tsx\n- src/app/global.d.ts".to_string(),
            steps: vec![PlanStep {
                id: "app".to_string(),
                kind: "implement".to_string(),
                expected_result: "pass".to_string(),
                instruction: "Create the interactive Next.js game app".to_string(),
                expected_paths: vec![
                    "package.json".to_string(),
                    "src/app/page.tsx".to_string(),
                    "src/app/layout.tsx".to_string(),
                    "src/app/global.d.ts".to_string(),
                ],
                verify: Vec::new(),
            }],
        };
        let page = interactive_game_page_source();
        let mut fake = FakeClient::new(vec![AssistantReply {
            content: String::new(),
            tool_calls: nextjs_interactive_app_tool_calls(page),
            prompt_tokens: None,
            completion_tokens: None,
        }]);
        let err = run_step_plan(&mut fake, &plan, &cfg)
            .unwrap_err()
            .to_string();
        assert!(err.contains("release gate failed"), "{err}");
        assert!(
            err.contains("browser_interaction_failed:canvas_unavailable"),
            "{err}"
        );
        let event_text = std::fs::read_to_string(events).unwrap();
        assert!(event_text.contains("\"release_gate_status\":\"failed\""));
        assert!(event_text.contains("\"final_acceptance_status\":\"incomplete\""));
        assert!(
            event_text.contains("\"interaction_evidence_status\":\"failed:canvas_unavailable\"")
        );
    }
}
