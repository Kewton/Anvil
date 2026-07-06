use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};

use anvilminimal::config::{Action, Config, Provider};
use anvilminimal::planner::step_plan::{PlanStep, StepPlan};
use anvilminimal::planner::ultra_plan::{UltraPhase, UltraPlan, render_ultra_plan};
use anvilminimal::providers::{AssistantReply, ChatClient};
use anvilminimal::state::{ConversationMessage, ToolCall};
use anvilminimal::tools::registry::ToolSpec;
use anvilminimal::tui::status::UiStatus;
use anvilminimal::tui::{InteractionUi, UiGuard};
use serde_json::{Value, json};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum MatrixScenario {
    Nextjs,
    PythonCli,
    GenericStatic,
    GenericPromoted,
}

impl MatrixScenario {
    fn name(self) -> &'static str {
        match self {
            Self::Nextjs => "nextjs",
            Self::PythonCli => "python_cli",
            Self::GenericStatic => "generic-static",
            Self::GenericPromoted => "generic-promoted",
        }
    }

    fn configured_profile(self) -> &'static str {
        match self {
            Self::Nextjs => "nextjs",
            Self::PythonCli => "python-cli",
            Self::GenericStatic | Self::GenericPromoted => "generic",
        }
    }

    fn profile_explicit(self) -> bool {
        !matches!(self, Self::GenericPromoted)
    }

    fn plan(self) -> UltraPlan {
        match self {
            Self::Nextjs => UltraPlan {
                goal: "Create a route page".to_string(),
                profile: "nextjs".to_string(),
                style: "default".to_string(),
                intent: "create".to_string(),
                phases: vec![
                    UltraPhase {
                        id: "scaffold".to_string(),
                        prompt: "Create the project scaffold and release evidence.".to_string(),
                    },
                    UltraPhase {
                        id: "verify-surface".to_string(),
                        prompt: "Refresh the route page deterministically.".to_string(),
                    },
                ],
            },
            Self::PythonCli => UltraPlan {
                goal: "CSVファイルを読み込み、数値列の合計・平均・最大・最小を集計して表形式で標準出力するCLIツールをPythonで開発してください。".to_string(),
                profile: "python-cli".to_string(),
                style: "default".to_string(),
                intent: "create".to_string(),
                phases: vec![
                    UltraPhase {
                        id: "python-cli".to_string(),
                        prompt: "Create and verify the Python CLI.".to_string(),
                    },
                    UltraPhase {
                        id: "verify-cli".to_string(),
                        prompt: "Refresh the Python CLI deterministically.".to_string(),
                    },
                ],
            },
            Self::GenericStatic => UltraPlan {
                goal: "ちょっとしたメモアプリを作って".to_string(),
                profile: "generic".to_string(),
                style: "default".to_string(),
                intent: "create".to_string(),
                phases: vec![
                    UltraPhase {
                        id: "scaffold".to_string(),
                        prompt: "Create a generic interactive source artifact.".to_string(),
                    },
                    UltraPhase {
                        id: "finish".to_string(),
                        prompt: "Record a static-tier marker without framework manifest.".to_string(),
                    },
                ],
            },
            Self::GenericPromoted => UltraPlan {
                goal: "Create a route page".to_string(),
                profile: "generic".to_string(),
                style: "default".to_string(),
                intent: "create".to_string(),
                phases: vec![
                    UltraPhase {
                        id: "setup-framework".to_string(),
                        prompt: "Create the package manifest.".to_string(),
                    },
                    UltraPhase {
                        id: "implement-ui".to_string(),
                        prompt: "Create the promoted route page.".to_string(),
                    },
                ],
            },
        }
    }
}

#[derive(Debug)]
struct Trace {
    scenario: MatrixScenario,
    events: Vec<Value>,
    summary: String,
    output: String,
}

#[test]
fn conformance_matrix_runs_ultra_lifecycle_paths() {
    for scenario in [
        MatrixScenario::Nextjs,
        MatrixScenario::PythonCli,
        MatrixScenario::GenericStatic,
        MatrixScenario::GenericPromoted,
    ] {
        let trace = run_matrix_scenario(scenario);
        assert!(
            trace.output.contains("ultra-plan-run complete"),
            "{} output:\n{}",
            scenario.name(),
            trace.output
        );
        assert_has_event(&trace, "tui_command_start");
        assert_has_event(&trace, "ultra_phase_start");
        assert_has_event(&trace, "ultra_final_acceptance");
        assert_has_event(&trace, "ultra_plan_complete");
        assert_has_event(&trace, "tui_command_stop");
        assert_eq!(
            events_named(&trace.events, "tui_command_stop").len(),
            1,
            "{} events:\n{}",
            scenario.name(),
            render_events(&trace.events)
        );
        assert!(
            trace
                .summary
                .starts_with(&format!("{}\n", anvilminimal::build_info::summary_line())),
            "{} summary:\n{}",
            scenario.name(),
            trace.summary
        );
        if matches!(scenario, MatrixScenario::GenericPromoted) {
            assert_has_event(&trace, "profile_reinferred");
        }
        assert_conformance_contracts(&trace);
    }
}

#[test]
fn conformance_negative_monotonic_rebind_catches_smaller_rebind() {
    let trace = Trace {
        scenario: MatrixScenario::GenericPromoted,
        events: vec![
            json!({
                "event": "generic_contract_bound",
                "required_capabilities": ["stateful_interaction", "user_input_or_action"],
                "required_evidence": ["user_input_handler_evidence"],
                "required_obligations": ["implementation"]
            }),
            json!({
                "event": "profile_reinferred",
                "contract_origin": "promoted_union",
                "id": "nextjs"
            }),
            json!({
                "event": "ultra_final_acceptance",
                "contract_origin": "promoted_union",
                "required_capabilities": ["stateful_interaction"],
                "required_evidence": [],
                "required_obligations": ["implementation"]
            }),
        ],
        summary: format!(
            "{}\nStatus: completed\n",
            anvilminimal::build_info::summary_line()
        ),
        output: String::new(),
    };

    assert_contract_fails("monotonic_rebind", check_monotonic_rebind(&trace));
}

#[test]
fn conformance_negative_earned_assurance_catches_disconnected_gate() {
    let trace = Trace {
        scenario: MatrixScenario::Nextjs,
        events: vec![json!({
            "event": "ultra_final_acceptance",
            "assurance_level": "full",
            "final_acceptance_status": "full_success",
            "release_gate_status": "pass",
            "browser_readiness_applicable": true,
            "browser_readiness_execution_status": "not_applicable",
            "browser_readiness_status": "not_applicable",
            "interaction_evidence_applicable": true,
            "interaction_evidence_execution_status": "not_applicable",
            "interaction_evidence_status": "skipped"
        })],
        summary: format!(
            "{}\nStatus: completed\n",
            anvilminimal::build_info::summary_line()
        ),
        output: String::new(),
    };

    assert_contract_fails("earned_assurance", check_earned_assurance(&trace));
}

#[test]
fn conformance_negative_bounded_provider_turns_catches_silent_phase_context_window() {
    let trace = Trace {
        scenario: MatrixScenario::GenericStatic,
        events: vec![
            json!({
                "event": "provider_turn_duration",
                "caller_scope": "planner_ultra",
                "duration_ms": 10,
                "timed_out": false,
            }),
            json!({
                "event": "ultra_phase_context_attached",
                "phase_id": "scaffold",
            }),
            json!({
                "event": "ultra_phase_failed",
                "phase_id": "scaffold",
                "failure_kind": "phase_step_planner_timeout",
            }),
            terminal_stop("failed"),
        ],
        summary: terminal_summary("failed"),
        output: String::new(),
    };

    assert_contract_fails(
        "bounded_provider_turns",
        check_bounded_provider_turns(&trace),
    );
}

#[test]
fn conformance_honest_terminal_covers_simulated_panic_exit() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    let events_path = root.join(".anvil/runs/conformance-panic/events.jsonl");
    let plan = UltraPlan {
        goal: "Create a panic probe artifact".to_string(),
        profile: "generic".to_string(),
        style: "default".to_string(),
        intent: "create".to_string(),
        phases: vec![
            UltraPhase {
                id: "panic".to_string(),
                prompt: "Create the panic probe.".to_string(),
            },
            UltraPhase {
                id: "unused".to_string(),
                prompt: "This phase is never reached.".to_string(),
            },
        ],
    };
    std::fs::write(root.join("ultra.yaml"), render_ultra_plan(&plan)).unwrap();
    let mut cfg = config(root.to_path_buf());
    cfg.eval_events_path = Some(events_path.clone());
    let mut planner = FakeClient::new(
        "planner",
        vec![AssistantReply::text(step_plan_json(
            "Create panic probe",
            "implement",
            vec!["panic.txt".to_string()],
            Vec::new(),
        ))],
    );
    let mut execution = PanicClient::new("execution", "simulated conformance panic");
    let ui = FakeUi::default();

    let panic = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let _ = anvilminimal::tui::slash::handle_command(
            "/run-ultra-plan ultra.yaml",
            &cfg,
            &mut planner,
            &mut execution,
            &ui,
        );
    }));
    assert!(panic.is_err());

    let trace = Trace {
        scenario: MatrixScenario::GenericStatic,
        events: read_events(&events_path),
        summary: std::fs::read_to_string(events_path.parent().unwrap().join("summary.md"))
            .unwrap_or_default(),
        output: String::new(),
    };
    assert_has_event(&trace, "panic_caught");
    check_honest_terminal(&trace).unwrap_or_else(|err| panic!("{err}"));
}

#[test]
fn conformance_boundedness_covers_hanging_dependency_setup() {
    let trace = Trace {
        scenario: MatrixScenario::Nextjs,
        events: vec![
            json!({
                "event": "provider_turn_duration",
                "elapsed_ms": 10,
                "timed_out": false,
            }),
            json!({
                "event": "dependency_build_lifecycle",
                "setup_status": "timed_out",
                "setup_attempted": true,
                "setup_timeout_classification": "dependency_setup_timeout",
                "setup_duration_ms": 100,
                "setup_timeout_ms": 100,
                "final_status": "blocked",
            }),
            terminal_stop("failed"),
        ],
        summary: terminal_summary("failed"),
        output: String::new(),
    };

    check_honest_terminal(&trace).unwrap_or_else(|err| panic!("{err}"));
    check_bounded_child_processes(&trace).unwrap_or_else(|err| panic!("{err}"));
}

#[test]
fn conformance_boundedness_covers_hanging_bash_tool() {
    let trace = Trace {
        scenario: MatrixScenario::GenericStatic,
        events: vec![
            json!({
                "event": "provider_turn_duration",
                "elapsed_ms": 10,
                "timed_out": false,
            }),
            json!({
                "event": "tool_validation_error",
                "name": "Bash",
                "error_kind": "command_timeout",
                "duration_ms": 100,
                "repeat_count": 1,
            }),
            terminal_stop("partial"),
        ],
        summary: terminal_summary("partial"),
        output: String::new(),
    };

    check_honest_terminal(&trace).unwrap_or_else(|err| panic!("{err}"));
    check_bounded_child_processes(&trace).unwrap_or_else(|err| panic!("{err}"));
}

fn assert_conformance_contracts(trace: &Trace) {
    for (name, result) in [
        ("earned_assurance", check_earned_assurance(trace)),
        ("monotonic_rebind", check_monotonic_rebind(trace)),
        ("authority_symmetry", check_authority_symmetry(trace)),
        ("detect_repair_pairing", check_detect_repair_pairing(trace)),
        ("honest_terminal", check_honest_terminal(trace)),
        (
            "known_profile_contract_bound",
            check_known_profile_contract_bound(trace),
        ),
        (
            "bounded_provider_turns",
            check_bounded_provider_turns(trace),
        ),
        (
            "bounded_verify_commands",
            check_bounded_verify_commands(trace),
        ),
        (
            "bounded_child_processes",
            check_bounded_child_processes(trace),
        ),
        ("oracle_tristate", check_oracle_tristate(trace)),
        ("degradation_labeling", check_degradation_labeling(trace)),
    ] {
        result.unwrap_or_else(|err| panic!("{} failed for {}: {err}", name, trace.scenario.name()));
    }
}

fn assert_contract_fails(name: &str, result: Result<(), String>) {
    assert!(
        result.is_err(),
        "{name} checker accepted a deliberately invalid trace"
    );
}

fn check_earned_assurance(trace: &Trace) -> Result<(), String> {
    for event in completion_events(trace) {
        if string_field(event, "assurance_level") != Some("full") {
            continue;
        }
        let event_name = string_field(event, "event").unwrap_or("<unknown>");
        if let Some(status) = string_field(event, "final_acceptance_status")
            && !matches!(status, "full_success" | "pass")
        {
            return Err(format!(
                "earned_assurance: {event_name} has full assurance but final_acceptance_status={status}"
            ));
        }
        if let Some(status) = string_field(event, "release_gate_status")
            && matches!(
                status,
                "partial" | "failed" | "not_applicable" | "not_checked"
            )
        {
            let applicable = bool_field(event, "browser_readiness_applicable").unwrap_or(false)
                || bool_field(event, "interaction_evidence_applicable").unwrap_or(false);
            if applicable {
                return Err(format!(
                    "earned_assurance: {event_name} has full assurance but release_gate_status={status}"
                ));
            }
        }
        for gate in ["browser_readiness", "interaction_evidence"] {
            let applicable_key = format!("{gate}_applicable");
            if bool_field(event, &applicable_key) != Some(true) {
                continue;
            }
            let status_key = format!("{gate}_status");
            let execution_key = format!("{gate}_execution_status");
            let status = string_field(event, &status_key).unwrap_or("");
            let execution = string_field(event, &execution_key).unwrap_or("");
            if disconnected_gate_status(status) || status.starts_with("unavailable:") {
                return Err(format!(
                    "earned_assurance: {event_name} has full assurance with disconnected {status_key}={status}"
                ));
            }
            if !matches!(execution, "performed" | "performed_failed") {
                return Err(format!(
                    "earned_assurance: {event_name} has full assurance with non-executed {execution_key}={execution}"
                ));
            }
            if status != "passed" {
                return Err(format!(
                    "earned_assurance: {event_name} has full assurance but {status_key}={status}"
                ));
            }
        }
        if string_array(event, "release_gate_reasons")
            .iter()
            .any(|reason| reason.contains("acceptance_gates_disconnected"))
        {
            return Err(format!(
                "earned_assurance: {event_name} accepted full assurance despite acceptance_gates_disconnected"
            ));
        }
    }
    Ok(())
}

fn check_monotonic_rebind(trace: &Trace) -> Result<(), String> {
    let Some(promotion_index) = trace
        .events
        .iter()
        .position(|event| string_field(event, "event") == Some("profile_reinferred"))
    else {
        return Ok(());
    };

    let mut prior_capabilities = Vec::new();
    let mut prior_evidence = Vec::new();
    let mut prior_obligations = Vec::new();
    for event in &trace.events[..promotion_index] {
        if matches!(
            string_field(event, "event"),
            Some("generic_contract_bound" | "completion_contract_bound")
        ) {
            merge_strings(
                &mut prior_capabilities,
                string_array(event, "required_capabilities"),
            );
            merge_strings(
                &mut prior_evidence,
                string_array(event, "required_evidence"),
            );
            merge_strings(
                &mut prior_obligations,
                string_array(event, "required_obligations"),
            );
        }
    }
    if prior_capabilities.is_empty() && prior_evidence.is_empty() && prior_obligations.is_empty() {
        return Ok(());
    }
    let final_contract = trace.events[promotion_index + 1..]
        .iter()
        .rev()
        .find(|event| {
            matches!(
                string_field(event, "event"),
                Some(
                    "ultra_final_acceptance" | "ultra_plan_complete" | "completion_contract_bound"
                )
            ) && (event.get("required_capabilities").is_some()
                || event.get("required_evidence").is_some()
                || event.get("required_obligations").is_some())
        })
        .ok_or_else(|| {
            "monotonic_rebind: promoted trace has no final contract event".to_string()
        })?;

    assert_subset(
        "monotonic_rebind",
        "required_capabilities",
        &prior_capabilities,
        &string_array(final_contract, "required_capabilities"),
    )?;
    assert_subset(
        "monotonic_rebind",
        "required_evidence",
        &prior_evidence,
        &string_array(final_contract, "required_evidence"),
    )?;
    assert_subset(
        "monotonic_rebind",
        "required_obligations",
        &prior_obligations,
        &string_array(final_contract, "required_obligations"),
    )
}

fn check_authority_symmetry(trace: &Trace) -> Result<(), String> {
    for (index, event) in trace.events.iter().enumerate() {
        if string_field(event, "event") != Some("dependency_build_lifecycle") {
            continue;
        }
        let requires_setup = bool_field(event, "requires_dependency_setup").unwrap_or(false);
        let setup_attempted = bool_field(event, "setup_attempted").unwrap_or(false);
        let setup_authority = string_field(event, "setup_authority").unwrap_or("");
        let setup_status = string_field(event, "setup_status").unwrap_or("");
        let final_status = string_field(event, "final_status").unwrap_or("");
        if setup_attempted && matches!(setup_authority, "" | "none") {
            return Err(format!(
                "authority_symmetry: setup attempted without sanctioned authority in {event}"
            ));
        }
        if setup_attempted && matches!(setup_status, "setup_authority_missing" | "blocked") {
            return Err(format!(
                "authority_symmetry: setup attempted but setup_status={setup_status} in {event}"
            ));
        }
        if requires_setup
            && dependency_blocked_status(final_status)
            && !has_later_handoff_or_repair(&trace.events[index + 1..])
        {
            return Err(format!(
                "authority_symmetry: dependency need ended as {final_status} without install lifecycle or handoff"
            ));
        }
    }

    if trace
        .events
        .iter()
        .any(|event| string_field(event, "event") == Some("profile_reinferred"))
    {
        let has_reconciliation = trace.events.iter().any(|event| {
            string_field(event, "event") == Some("dependency_setup_reconciliation")
                && matches!(
                    string_field(event, "status"),
                    Some("passed" | "blocked" | "failed")
                )
        });
        if trace.scenario == MatrixScenario::GenericPromoted && !has_reconciliation {
            return Err(
                "authority_symmetry: promoted pathway did not record dependency reconciliation"
                    .to_string(),
            );
        }
    }
    Ok(())
}

fn check_detect_repair_pairing(trace: &Trace) -> Result<(), String> {
    for (index, event) in trace.events.iter().enumerate() {
        if !classification_requires_pairing(event) {
            continue;
        }
        if event_has_repair_or_handoff(event)
            || has_later_handoff_or_repair(&trace.events[index + 1..])
        {
            continue;
        }
        return Err(format!(
            "detect_repair_pairing: failure classification stranded at event {event}"
        ));
    }
    Ok(())
}

fn check_honest_terminal(trace: &Trace) -> Result<(), String> {
    let stops = events_named(&trace.events, "tui_command_stop");
    if stops.len() != 1 {
        return Err(format!(
            "honest_terminal: expected exactly one tui_command_stop, saw {}",
            stops.len()
        ));
    }
    let stop = stops[0];
    let status = string_field(stop, "status").unwrap_or("");
    if !matches!(
        status,
        "completed" | "partial" | "failed" | "aborted" | "interrupted"
    ) {
        return Err(format!(
            "honest_terminal: status {status:?} outside closed set"
        ));
    }
    if status == "running" || status.is_empty() {
        return Err("honest_terminal: terminal status is absent/running".to_string());
    }
    for key in [
        "build_commit",
        "build_timestamp",
        "effective_profile",
        "contract_origin",
    ] {
        if string_field(stop, key).unwrap_or("").trim().is_empty() {
            return Err(format!(
                "honest_terminal: missing {key} on tui_command_stop"
            ));
        }
    }
    if !trace
        .summary
        .starts_with(&anvilminimal::build_info::summary_line())
    {
        return Err("honest_terminal: summary is missing Build stamp".to_string());
    }
    if !trace.summary.contains("Status: ") || trace.summary.contains("Status: running") {
        return Err("honest_terminal: summary has absent/running status".to_string());
    }
    Ok(())
}

fn check_known_profile_contract_bound(trace: &Trace) -> Result<(), String> {
    if !matches!(
        trace.scenario,
        MatrixScenario::Nextjs | MatrixScenario::PythonCli
    ) {
        return Ok(());
    }
    let expected_profile = trace.scenario.configured_profile();
    let Some(contract) = trace.events.iter().find(|event| {
        string_field(event, "event") == Some("completion_contract_bound")
            && bool_field(event, "completion_contract_generated") == Some(true)
    }) else {
        return Err(format!(
            "known_profile_contract_bound: explicit known profile {expected_profile} did not bind a generated completion contract"
        ));
    };
    let required_capabilities = string_array(contract, "required_capabilities");
    let required_evidence = string_array(contract, "required_evidence");
    let required_obligations = string_array(contract, "required_obligations");
    if required_capabilities.is_empty()
        && required_evidence.is_empty()
        && required_obligations.is_empty()
    {
        return Err(format!(
            "known_profile_contract_bound: explicit known profile {expected_profile} bound an empty completion contract"
        ));
    }
    Ok(())
}

fn check_bounded_provider_turns(trace: &Trace) -> Result<(), String> {
    let duration_events = events_named(&trace.events, "provider_turn_duration");
    if duration_events.is_empty() {
        return Err(
            "bounded_provider_turns: no provider turn duration telemetry emitted".to_string(),
        );
    }
    for (index, event) in trace.events.iter().enumerate() {
        if string_field(event, "event") != Some("ultra_phase_context_attached") {
            continue;
        }
        let mut saw_duration = false;
        for next in trace.events.iter().skip(index + 1) {
            match string_field(next, "event") {
                Some("provider_turn_duration") => {
                    saw_duration = true;
                    break;
                }
                Some(name) if is_phase_boundary_event(name) => break,
                _ => {}
            }
        }
        if !saw_duration {
            return Err(
                "bounded_provider_turns: ultra phase context attach was followed by a silent provider window"
                    .to_string(),
            );
        }
    }
    for stop in events_named(&trace.events, "tui_command_stop") {
        if string_field(stop, "status") == Some("interrupted")
            || string_field(stop, "command_completion_state") == Some("interrupted")
            || string_field(stop, "task_status") == Some("interrupted")
        {
            return Err(format!(
                "bounded_provider_turns: pathway required human interruption in {stop}"
            ));
        }
    }
    let timeout_events = events_named(&trace.events, "provider_turn_timeout");
    if timeout_events.is_empty() {
        return Ok(());
    }
    if !timeout_events
        .iter()
        .any(|event| bool_field(event, "terminal") == Some(true))
    {
        return Err(
            "bounded_provider_turns: provider timeout did not reach terminal handoff".to_string(),
        );
    }
    if !events_named(&trace.events, "loop_stop")
        .iter()
        .any(|event| string_field(event, "reason") == Some("provider_turn_timeout"))
    {
        return Err("bounded_provider_turns: provider timeout lacks honest loop_stop".to_string());
    }
    Ok(())
}

fn is_phase_boundary_event(name: &str) -> bool {
    matches!(
        name,
        "ultra_phase_start"
            | "ultra_phase_failed"
            | "ultra_phase_scaffold_complete"
            | "ultra_phase_plan_validated"
            | "ultra_phase_execute_complete"
            | "ultra_phase_profile_check"
            | "ultra_phase_complete"
            | "ultra_plan_complete"
            | "loop_stop"
            | "tui_command_stop"
    )
}

fn check_bounded_verify_commands(trace: &Trace) -> Result<(), String> {
    for event in events_named(&trace.events, "verify_command_timeout") {
        if event.to_string().contains("ArtifactFail") {
            return Err(format!(
                "bounded_verify_commands: verify timeout became ArtifactFail in {event}"
            ));
        }
        if string_field(event, "classification") != Some("OracleError") {
            return Err(format!(
                "bounded_verify_commands: verify timeout lacks OracleError in {event}"
            ));
        }
        if string_field(event, "repair_target") == Some("implementation") {
            return Err(format!(
                "bounded_verify_commands: verify timeout targeted implementation in {event}"
            ));
        }
    }
    for stop in events_named(&trace.events, "tui_command_stop") {
        if string_field(stop, "status") == Some("interrupted")
            && stop.to_string().contains("verify_command_timeout")
        {
            return Err(
                "bounded_verify_commands: verify timeout required human interruption".to_string(),
            );
        }
    }
    Ok(())
}

fn check_bounded_child_processes(trace: &Trace) -> Result<(), String> {
    for event in events_named(&trace.events, "dependency_build_lifecycle") {
        if string_field(event, "setup_status") != Some("timed_out") {
            continue;
        }
        if string_field(event, "setup_timeout_classification") != Some("dependency_setup_timeout") {
            return Err(format!(
                "bounded_child_processes: dependency setup timeout lacks dependency_setup_timeout classification in {event}"
            ));
        }
        if u64_field(event, "setup_duration_ms").unwrap_or_default() == 0 {
            return Err(format!(
                "bounded_child_processes: dependency setup timeout lacks duration telemetry in {event}"
            ));
        }
        if u64_field(event, "setup_timeout_ms").unwrap_or_default() == 0 {
            return Err(format!(
                "bounded_child_processes: dependency setup timeout lacks timeout telemetry in {event}"
            ));
        }
    }
    for event in events_named(&trace.events, "tool_validation_error") {
        if string_field(event, "name") != Some("Bash")
            || string_field(event, "error_kind") != Some("command_timeout")
        {
            continue;
        }
        if u64_field(event, "duration_ms").unwrap_or_default() == 0 {
            return Err(format!(
                "bounded_child_processes: Bash command_timeout lacks duration telemetry in {event}"
            ));
        }
    }
    for stop in events_named(&trace.events, "tui_command_stop") {
        if string_field(stop, "status") == Some("interrupted") {
            return Err(format!(
                "bounded_child_processes: hanging child pathway required human interruption in {stop}"
            ));
        }
    }
    Ok(())
}

fn check_oracle_tristate(trace: &Trace) -> Result<(), String> {
    for event in &trace.events {
        let text = event.to_string();
        if text.contains("OracleError")
            && string_field(event, "repair_target") == Some("implementation")
        {
            return Err(format!(
                "oracle_tristate: OracleError produced implementation repair target in {event}"
            ));
        }
        let deterministic_failure = text.contains("deterministic")
            && (bool_field(event, "ok") == Some(false)
                || string_field(event, "status")
                    .is_some_and(|status| matches!(status, "failed" | "incomplete")));
        if deterministic_failure && !(text.contains("ArtifactFail") || text.contains("OracleError"))
        {
            return Err(format!(
                "oracle_tristate: deterministic verify failure lacks ArtifactFail/OracleError in {event}"
            ));
        }
    }
    Ok(())
}

fn check_degradation_labeling(trace: &Trace) -> Result<(), String> {
    for event in completion_events(trace) {
        let degraded_probe = ["browser_readiness_status", "interaction_evidence_status"]
            .iter()
            .filter_map(|key| string_field(event, key))
            .any(|status| {
                status.starts_with("unavailable:")
                    || status.contains("probe_unavailable")
                    || status.contains("probe_dependency_missing")
            });
        if !degraded_probe {
            continue;
        }
        if string_field(event, "assurance_level") == Some("full") {
            return Err(format!(
                "degradation_labeling: probe unavailable path reported full assurance in {event}"
            ));
        }
        let event_text = event.to_string();
        let has_remediation = event_text.contains("remediation")
            || event_text.contains("probe")
            || trace.summary.contains("remediation")
            || trace.summary.contains("probe");
        if !has_remediation {
            return Err(format!(
                "degradation_labeling: degraded probe path lacks remediation text in {event}"
            ));
        }
    }
    Ok(())
}

fn run_matrix_scenario(scenario: MatrixScenario) -> Trace {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    let events_path = root.join(".anvil/runs/conformance/events.jsonl");
    let plan = scenario.plan();
    std::fs::write(root.join("ultra.yaml"), render_ultra_plan(&plan)).unwrap();
    scenario_prepare_workspace(scenario, root, events_path.parent().unwrap());

    let mut cfg = config(root.to_path_buf());
    cfg.eval_events_path = Some(events_path.clone());
    cfg.profile = scenario.configured_profile().to_string();
    cfg.profile_explicit = scenario.profile_explicit();

    let mut planner = FakeClient::new("planner", scenario_planner_replies(scenario));
    let mut execution = FakeClient::new("execution", scenario_execution_replies(scenario));
    let ui = FakeUi::default();
    let output = match anvilminimal::tui::slash::handle_command(
        "/run-ultra-plan ultra.yaml",
        &cfg,
        &mut planner,
        &mut execution,
        &ui,
    ) {
        Ok(output) => output,
        Err(err) => {
            let events = read_events(&events_path);
            panic!(
                "{} failed: {err}\nevents:\n{}",
                scenario.name(),
                render_events(&events)
            );
        }
    };

    let events = read_events(&events_path);
    let summary = std::fs::read_to_string(events_path.parent().unwrap().join("summary.md"))
        .unwrap_or_default();
    Trace {
        scenario,
        events,
        summary,
        output,
    }
}

fn scenario_prepare_workspace(scenario: MatrixScenario, root: &Path, run_dir: &Path) {
    match scenario {
        MatrixScenario::Nextjs => {
            write_fake_npm_dependency_installer(root);
            write_fake_nextjs_dependencies_ready(root);
            write_browser_release_evidence(run_dir);
        }
        MatrixScenario::GenericPromoted => {
            write_fake_npm_dependency_installer(root);
            write_browser_release_evidence(run_dir);
        }
        MatrixScenario::PythonCli | MatrixScenario::GenericStatic => {}
    }
}

fn scenario_planner_replies(scenario: MatrixScenario) -> Vec<AssistantReply> {
    match scenario {
        MatrixScenario::Nextjs => vec![
            AssistantReply::text(step_plan_json(
                "Create Next.js workspace",
                "implement",
                nextjs_expected_paths(),
                vec!["npm run build".to_string()],
            )),
            AssistantReply::text(step_plan_json(
                "Refresh Next.js workspace",
                "implement",
                nextjs_expected_paths(),
                vec!["npm run build".to_string()],
            )),
        ],
        MatrixScenario::PythonCli => vec![
            AssistantReply::text(step_plan_json(
                "Create Python CSV CLI",
                "implement",
                vec![
                    "pyproject.toml".to_string(),
                    "src/anvil_app/main.py".to_string(),
                ],
                vec!["python -m compileall -q src".to_string()],
            )),
            AssistantReply::text(step_plan_json(
                "Refresh Python CSV CLI",
                "implement",
                vec![
                    "pyproject.toml".to_string(),
                    "src/anvil_app/main.py".to_string(),
                ],
                vec!["python -m compileall -q src".to_string()],
            )),
        ],
        MatrixScenario::GenericStatic => vec![
            AssistantReply::text(step_plan_json(
                "Create generic memo app source",
                "implement",
                vec!["memo.jsx".to_string()],
                Vec::new(),
            )),
            AssistantReply::text(step_plan_json(
                "Record generic static marker",
                "implement",
                vec!["generic-static.txt".to_string()],
                Vec::new(),
            )),
        ],
        MatrixScenario::GenericPromoted => vec![
            AssistantReply::text(step_plan_json(
                "Create package manifest",
                "setup",
                vec!["package.json".to_string()],
                Vec::new(),
            )),
            AssistantReply::text(step_plan_json(
                "Complete promoted Next.js app",
                "implement",
                nextjs_expected_paths()
                    .into_iter()
                    .filter(|path| path != "package.json")
                    .collect(),
                vec!["npm run build".to_string()],
            )),
        ],
    }
}

fn scenario_execution_replies(scenario: MatrixScenario) -> Vec<AssistantReply> {
    match scenario {
        MatrixScenario::Nextjs => vec![
            AssistantReply {
                content: String::new(),
                tool_calls: nextjs_tool_calls()
                    .into_iter()
                    .chain(browser_release_evidence_tool_calls())
                    .collect(),
                prompt_tokens: None,
                completion_tokens: None,
            },
            AssistantReply {
                content: String::new(),
                tool_calls: nextjs_tool_calls()
                    .into_iter()
                    .chain(browser_release_evidence_tool_calls())
                    .collect(),
                prompt_tokens: None,
                completion_tokens: None,
            },
        ],
        MatrixScenario::PythonCli => vec![
            AssistantReply {
                content: String::new(),
                tool_calls: vec![
                    ToolCall::new(
                        "Write",
                        json!({"path":"pyproject.toml","content":python_cli_pyproject()}),
                    ),
                    ToolCall::new(
                        "Write",
                        json!({"path":"src/anvil_app/main.py","content":python_cli_main()}),
                    ),
                ],
                prompt_tokens: None,
                completion_tokens: None,
            },
            AssistantReply {
                content: String::new(),
                tool_calls: vec![
                    ToolCall::new(
                        "Write",
                        json!({"path":"pyproject.toml","content":python_cli_pyproject()}),
                    ),
                    ToolCall::new(
                        "Write",
                        json!({"path":"src/anvil_app/main.py","content":python_cli_main()}),
                    ),
                ],
                prompt_tokens: None,
                completion_tokens: None,
            },
        ],
        MatrixScenario::GenericStatic => vec![
            write_reply("memo.jsx", generic_interactive_source()),
            write_reply("generic-static.txt", "static fallback observed\n"),
        ],
        MatrixScenario::GenericPromoted => vec![
            write_reply("package.json", nextjs_complete_package_json()),
            AssistantReply {
                content: String::new(),
                tool_calls: nextjs_tool_calls()
                    .into_iter()
                    .filter(|call| {
                        call.arguments.get("path").and_then(Value::as_str) != Some("package.json")
                    })
                    .chain(browser_release_evidence_tool_calls())
                    .collect(),
                prompt_tokens: None,
                completion_tokens: None,
            },
        ],
    }
}

fn config(root: PathBuf) -> Config {
    Config {
        workspace_root: root.clone(),
        state_dir: root.join("state"),
        eval_events_path: None,
        completion_contract_path: None,
        yes: true,
        offline: false,
        context_budget: 1000,
        model: "m".to_string(),
        provider: Provider::Ollama,
        planner_model: "pm".to_string(),
        planner_provider: Provider::Gemini,
        ollama_host: "http://localhost:11434".to_string(),
        num_predict: 100,
        max_iterations: 6,
        chat_timeout_secs: 1,
        chat_retries: 1,
        resume: None,
        fresh_session: false,
        no_footer: false,
        profile: "generic".to_string(),
        profile_explicit: false,
        profile_inference: None,
        style: "default".to_string(),
        action: Action::Repl,
    }
}

struct FakeClient {
    label: &'static str,
    replies: Vec<AssistantReply>,
    requests: Vec<Vec<ConversationMessage>>,
}

impl FakeClient {
    fn new(label: &'static str, replies: Vec<AssistantReply>) -> Self {
        Self {
            label,
            replies,
            requests: Vec::new(),
        }
    }
}

impl ChatClient for FakeClient {
    fn label(&self) -> &str {
        self.label
    }

    fn supports_native_tools(&self, _model: &str) -> bool {
        true
    }

    fn chat(
        &mut self,
        _model: &str,
        messages: &[ConversationMessage],
        _tools: &[ToolSpec],
        _native_tools_enabled: bool,
    ) -> anyhow::Result<AssistantReply> {
        self.requests.push(messages.to_vec());
        if self.replies.is_empty() {
            anyhow::bail!("{} fake replies exhausted", self.label);
        }
        Ok(self.replies.remove(0))
    }
}

struct PanicClient {
    label: &'static str,
    message: &'static str,
}

impl PanicClient {
    fn new(label: &'static str, message: &'static str) -> Self {
        Self { label, message }
    }
}

impl ChatClient for PanicClient {
    fn label(&self) -> &str {
        self.label
    }

    fn supports_native_tools(&self, _model: &str) -> bool {
        true
    }

    fn chat(
        &mut self,
        _model: &str,
        _messages: &[ConversationMessage],
        _tools: &[ToolSpec],
        _native_tools_enabled: bool,
    ) -> anyhow::Result<AssistantReply> {
        panic!("{}", self.message);
    }
}

#[derive(Default)]
struct FakeUi {
    events: Mutex<Vec<String>>,
    interrupted: AtomicBool,
}

impl InteractionUi for FakeUi {
    fn before_model_call(&self, label: &str) -> UiGuard {
        self.events.lock().unwrap().push(format!("model:{label}"));
        UiGuard::noop()
    }

    fn before_tool_call(&self, name: &str) -> UiGuard {
        self.events.lock().unwrap().push(format!("tool:{name}"));
        UiGuard::noop()
    }

    fn publish_status(&self, status: UiStatus) {
        self.events
            .lock()
            .unwrap()
            .push(format!("status:{}:{}", status.provider, status.model));
    }

    fn interrupted(&self) -> bool {
        self.interrupted.load(Ordering::SeqCst)
    }
}

fn step_plan_json(
    goal: &str,
    kind: &str,
    expected_paths: Vec<String>,
    verify: Vec<String>,
) -> String {
    serde_json::to_string(&StepPlan {
        goal: goal.to_string(),
        steps: vec![PlanStep {
            id: "step-1".to_string(),
            kind: kind.to_string(),
            expected_result: "pass".to_string(),
            instruction: goal.to_string(),
            expected_paths,
            verify,
        }],
    })
    .unwrap()
}

fn write_reply(path: &str, content: &str) -> AssistantReply {
    AssistantReply {
        content: String::new(),
        tool_calls: vec![ToolCall::new(
            "Write",
            json!({"path": path, "content": content}),
        )],
        prompt_tokens: None,
        completion_tokens: None,
    }
}

fn nextjs_tool_calls() -> Vec<ToolCall> {
    vec![
        ToolCall::new(
            "Write",
            json!({"path":"package.json","content":nextjs_complete_package_json()}),
        ),
        ToolCall::new(
            "Write",
            json!({"path":"tsconfig.json","content":nextjs_tsconfig_json()}),
        ),
        ToolCall::new(
            "Write",
            json!({"path":"postcss.config.js","content":nextjs_postcss_config()}),
        ),
        ToolCall::new(
            "Write",
            json!({"path":"tailwind.config.ts","content":nextjs_tailwind_config_ts()}),
        ),
        ToolCall::new(
            "Write",
            json!({"path":"src/app/page.tsx","content":nextjs_page_source()}),
        ),
        ToolCall::new(
            "Write",
            json!({"path":"src/app/layout.tsx","content":nextjs_layout_source()}),
        ),
        ToolCall::new(
            "Write",
            json!({"path":"src/app/globals.css","content":nextjs_globals_css()}),
        ),
        ToolCall::new(
            "Write",
            json!({"path":"src/app/global.d.ts","content":"declare module \"*.css\";\n"}),
        ),
    ]
}

fn browser_release_evidence_tool_calls() -> Vec<ToolCall> {
    vec![
        ToolCall::new(
            "Write",
            json!({"path":"browser-readiness.json","content":r#"{"ok":true,"http_status":200,"route_rendered":true}"#}),
        ),
        ToolCall::new(
            "Write",
            json!({"path":"browser-interaction.json","content":r#"{"ok":true,"status":"passed","interaction_success":true,"interaction_performed":true,"surface_visible":true,"start_transition":true,"input_state_change":true,"input_state_evaluated_after_start":true,"input_event_observed":true,"state_changed":true,"canvas_found":true}"#}),
        ),
    ]
}

fn write_browser_release_evidence(run_dir: &Path) {
    std::fs::create_dir_all(run_dir).unwrap();
    std::fs::write(
        run_dir.join("browser-readiness.json"),
        r#"{"ok":true,"http_status":200,"route_rendered":true}"#,
    )
    .unwrap();
    std::fs::write(
        run_dir.join("browser-interaction.json"),
        r#"{"ok":true,"status":"passed","interaction_success":true,"interaction_performed":true,"surface_visible":true,"start_transition":true,"input_state_change":true,"input_state_evaluated_after_start":true,"input_event_observed":true,"state_changed":true,"canvas_found":true}"#,
    )
    .unwrap();
}

fn nextjs_expected_paths() -> Vec<String> {
    vec![
        "package.json".to_string(),
        "tsconfig.json".to_string(),
        "postcss.config.js".to_string(),
        "tailwind.config.ts".to_string(),
        "src/app/layout.tsx".to_string(),
        "src/app/page.tsx".to_string(),
        "src/app/globals.css".to_string(),
        "src/app/global.d.ts".to_string(),
    ]
}

fn nextjs_complete_package_json() -> &'static str {
    r#"{"dependencies":{"next":"^14.2.0","react":"^18.3.0","react-dom":"^18.3.0"},"devDependencies":{"typescript":"^5.5.0","@types/node":"^20.14.0","@types/react":"^18.3.0","@types/react-dom":"^18.3.0","tailwindcss":"^3.4.19","postcss":"^8.5.15","autoprefixer":"^10.4.20"},"scripts":{"build":"next build","dev":"next dev -p 3011","start":"next start -p 3011"}}"#
}

fn nextjs_tsconfig_json() -> &'static str {
    r#"{"compilerOptions":{"target":"ES2017","lib":["dom","dom.iterable","esnext"],"allowJs":true,"skipLibCheck":true,"strict":true,"noEmit":true,"esModuleInterop":true,"module":"esnext","moduleResolution":"bundler","resolveJsonModule":true,"isolatedModules":true,"jsx":"preserve","incremental":true,"plugins":[{"name":"next"}],"baseUrl":".","paths":{"@/*":["./src/*"]}},"include":["next-env.d.ts","**/*.ts","**/*.tsx",".next/types/**/*.ts"],"exclude":["node_modules"]}"#
}

fn nextjs_postcss_config() -> &'static str {
    "module.exports = { plugins: { tailwindcss: {}, autoprefixer: {} } };\n"
}

fn nextjs_tailwind_config_ts() -> &'static str {
    "import type { Config } from 'tailwindcss';\nconst config: Config = { content: ['./src/pages/**/*.{js,ts,jsx,tsx,mdx}', './src/components/**/*.{js,ts,jsx,tsx,mdx}', './src/app/**/*.{js,ts,jsx,tsx,mdx}'], theme: { extend: {} }, plugins: [] };\nexport default config;\n"
}

fn nextjs_layout_source() -> &'static str {
    "import './globals.css';\nexport default function Layout({children}:{children:React.ReactNode}){return <html><body>{children}</body></html>;}"
}

fn nextjs_globals_css() -> &'static str {
    "@tailwind base;\n@tailwind components;\n@tailwind utilities;\n"
}

fn nextjs_page_source() -> &'static str {
    r#""use client";
import { useEffect, useState } from "react";
export default function Page(){
  const [score, setScore] = useState(0);
  const [gameOver, setGameOver] = useState(false);
  const [bullets, setBullets] = useState<{ x: number; y: number }[]>([]);
  const [enemies, setEnemies] = useState([{ x: 10, y: 20 }]);
  const fireBullet = () => setBullets((items) => [...items, { x: 10, y: 90 }]);
  const restart = () => {
    setGameOver(false);
    setScore(0);
    setBullets([]);
    setEnemies([{ x: 10, y: 20 }]);
  };
  useEffect(() => {
    const onKeyDown = (event: KeyboardEvent) => {
      if (event.key === "ArrowLeft") fireBullet();
    };
    const frame = requestAnimationFrame(() => {
      bullets.forEach((bullet) => {
        enemies.forEach((enemy) => {
          if (Math.abs(bullet.x - enemy.x) < 12 && Math.abs(bullet.y - enemy.y) < 12) {
            setGameOver(true);
            setScore((value) => value + 10);
          }
        });
      });
      setEnemies((items) => items.map((enemy) => ({ ...enemy, x: enemy.x + 1 })));
    });
    window.addEventListener("keydown", onKeyDown);
    return () => {
      cancelAnimationFrame(frame);
      window.removeEventListener("keydown", onKeyDown);
    };
  }, [bullets, enemies]);
  return <main data-anvil-state={score}><button data-anvil-action="primary" onClick={fireBullet}>Start</button><button onClick={restart}>Restart</button><canvas /><p>score {score} enemy collision {gameOver ? "game over" : "playing"}</p></main>;
}
"#
}

fn generic_interactive_source() -> &'static str {
    r#"import { useState } from "react";
export default function Memo(){
  const [items, setItems] = useState([]);
  return <form onSubmit={(event) => { event.preventDefault(); setItems([...items, "note"]); }}>
    <input onChange={() => setItems([...items, "draft"])} />
    <button type="submit">Add</button>
    <ul>{items.map((item, index) => <li key={index}>{item}</li>)}</ul>
  </form>;
}
"#
}

fn python_cli_pyproject() -> &'static str {
    r#"[project]
name = "anvil-app"
version = "0.1.0"

[project.scripts]
csv-stats = "anvil_app.main:main"
"#
}

fn python_cli_main() -> &'static str {
    r#"#!/usr/bin/env python3
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
"#
}

#[cfg(unix)]
fn write_fake_npm_dependency_installer(root: &Path) {
    use std::os::unix::fs::PermissionsExt;

    let bin = root.join("node_modules/.bin");
    std::fs::create_dir_all(&bin).unwrap();
    let exe = shell_quote(&std::env::current_exe().unwrap().display().to_string());
    let script = r#"#!/bin/sh
set -eu
install_pkg() {
  name="$1"
  if grep -q "\"$name\"" package.json 2>/dev/null; then
    mkdir -p "node_modules/$name"
    printf '{"name":"%s"}\n' "$name" > "node_modules/$name/package.json"
  fi
}
if [ "$1" = "install" ]; then
  mkdir -p node_modules/.bin
  install_pkg next
  install_pkg react
  install_pkg react-dom
  install_pkg typescript
  install_pkg @types/node
  install_pkg @types/react
  install_pkg @types/react-dom
  install_pkg tailwindcss
  install_pkg postcss
  install_pkg autoprefixer
  if [ -d node_modules/next ]; then
    printf '#!/bin/sh\nexit 0\n' > node_modules/.bin/next
    chmod +x node_modules/.bin/next
  fi
  printf '{"lockfileVersion":3}\n' > package-lock.json
  exit 0
fi
if [ "$1" = "run" ] && [ "$2" = "build" ]; then
  test -x node_modules/.bin/next || { echo "next missing" >&2; exit 1; }
  if grep -q "\"tailwindcss\"" package.json 2>/dev/null; then
    test -d node_modules/tailwindcss || { echo "tailwindcss missing" >&2; exit 1; }
    test -d node_modules/postcss || { echo "postcss missing" >&2; exit 1; }
    test -d node_modules/autoprefixer || { echo "autoprefixer missing" >&2; exit 1; }
  fi
  echo "fake build ok"
  exit 0
fi
if [ "$1" = "run" ] && [ "$2" = "dev" ]; then
  ANVIL_CONFORMANCE_FAKE_DEV_SERVER_CHILD=1 ANVIL_CONFORMANCE_FAKE_DEV_SERVER_PORT=3011 exec __CONFORMANCE_TEST_EXE__ --ignored --exact suite::conformance_fake_dev_server_child --nocapture
fi
if [ "$1" = "run" ] && [ "$2" = "start" ]; then
  ANVIL_CONFORMANCE_FAKE_DEV_SERVER_CHILD=1 ANVIL_CONFORMANCE_FAKE_DEV_SERVER_PORT=3011 exec __CONFORMANCE_TEST_EXE__ --ignored --exact suite::conformance_fake_dev_server_child --nocapture
fi
echo "unexpected fake npm args: $*" >&2
exit 2
"#
    .replace("__CONFORMANCE_TEST_EXE__", &exe);
    let path = bin.join("npm");
    std::fs::write(&path, script).unwrap();
    let mut permissions = std::fs::metadata(&path).unwrap().permissions();
    permissions.set_mode(0o755);
    std::fs::set_permissions(path, permissions).unwrap();
}

#[cfg(unix)]
fn write_fake_nextjs_dependencies_ready(root: &Path) {
    use std::os::unix::fs::PermissionsExt;

    let bin = root.join("node_modules/.bin");
    std::fs::create_dir_all(&bin).unwrap();
    for package in [
        "next",
        "react",
        "react-dom",
        "typescript",
        "@types/node",
        "@types/react",
        "@types/react-dom",
        "tailwindcss",
        "postcss",
        "autoprefixer",
    ] {
        let package_dir = root.join("node_modules").join(package);
        std::fs::create_dir_all(&package_dir).unwrap();
        std::fs::write(package_dir.join("package.json"), "{}\n").unwrap();
    }
    let next_path = bin.join("next");
    std::fs::write(&next_path, "#!/bin/sh\nexit 0\n").unwrap();
    let mut permissions = std::fs::metadata(&next_path).unwrap().permissions();
    permissions.set_mode(0o755);
    std::fs::set_permissions(next_path, permissions).unwrap();
}

#[cfg(unix)]
fn shell_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\"'\"'"))
}

#[cfg(unix)]
#[test]
#[ignore]
fn conformance_fake_dev_server_child() {
    if std::env::var("ANVIL_CONFORMANCE_FAKE_DEV_SERVER_CHILD")
        .ok()
        .as_deref()
        != Some("1")
    {
        return;
    }
    let port = std::env::var("ANVIL_CONFORMANCE_FAKE_DEV_SERVER_PORT")
        .unwrap()
        .parse::<u16>()
        .unwrap();
    let listener = std::net::TcpListener::bind(("127.0.0.1", port)).unwrap();
    for stream in listener.incoming() {
        let mut stream = stream.unwrap();
        let body = r#"<!doctype html><html><head><title>Conformance</title></head><body><main data-anvil-state="{&quot;score&quot;:0}"><button data-anvil-action="primary">Start</button><button>Restart</button><canvas></canvas><p>score 0 enemy collision playing</p></main></body></html>"#;
        let response = format!(
            "HTTP/1.1 200 OK\r\nContent-Type: text/html\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
            body.len(),
            body
        );
        let _ = std::io::Write::write_all(&mut stream, response.as_bytes());
    }
}

#[cfg(not(unix))]
fn write_fake_npm_dependency_installer(root: &Path) {
    let bin = root.join("node_modules/.bin");
    std::fs::create_dir_all(&bin).unwrap();
    let script = r#"@echo off
setlocal
if "%1"=="install" (
  if exist package.json (
    findstr /c:"\"next\"" package.json >nul && mkdir node_modules\next 2>nul
    findstr /c:"\"tailwindcss\"" package.json >nul && mkdir node_modules\tailwindcss 2>nul
    findstr /c:"\"postcss\"" package.json >nul && mkdir node_modules\postcss 2>nul
    findstr /c:"\"autoprefixer\"" package.json >nul && mkdir node_modules\autoprefixer 2>nul
    if exist node_modules\next (
      echo @echo off> node_modules\.bin\next.cmd
      echo exit /b 0>> node_modules\.bin\next.cmd
      echo {"name":"next"}> node_modules\next\package.json
    )
  )
  echo {"lockfileVersion":3}> package-lock.json
  exit /b 0
)
if "%1"=="run" if "%2"=="build" (
  if not exist node_modules\.bin\next.cmd exit /b 1
  echo fake build ok
  exit /b 0
)
echo unexpected fake npm args: %*
exit /b 2
"#;
    std::fs::write(bin.join("npm.cmd"), script).unwrap();
}

#[cfg(not(unix))]
fn write_fake_nextjs_dependencies_ready(root: &Path) {
    let bin = root.join("node_modules/.bin");
    std::fs::create_dir_all(&bin).unwrap();
    for package in ["next", "tailwindcss", "postcss", "autoprefixer"] {
        let package_dir = root.join("node_modules").join(package);
        std::fs::create_dir_all(&package_dir).unwrap();
        std::fs::write(package_dir.join("package.json"), "{}\n").unwrap();
    }
    std::fs::write(bin.join("next.cmd"), "@echo off\r\nexit /b 0\r\n").unwrap();
}

fn read_events(path: &Path) -> Vec<Value> {
    std::fs::read_to_string(path)
        .unwrap_or_default()
        .lines()
        .filter_map(|line| serde_json::from_str::<Value>(line).ok())
        .collect()
}

fn terminal_stop(status: &str) -> Value {
    json!({
        "event": "tui_command_stop",
        "status": status,
        "build_commit": anvilminimal::build_info::COMMIT,
        "build_timestamp": anvilminimal::build_info::TIMESTAMP,
        "effective_profile": "generic",
        "contract_origin": "synthetic_conformance",
    })
}

fn terminal_summary(status: &str) -> String {
    format!(
        "{}\nStatus: {status}\n",
        anvilminimal::build_info::summary_line()
    )
}

fn events_named<'a>(events: &'a [Value], name: &str) -> Vec<&'a Value> {
    events
        .iter()
        .filter(|event| event.get("event").and_then(Value::as_str) == Some(name))
        .collect()
}

fn assert_has_event(trace: &Trace, name: &str) {
    assert!(
        trace
            .events
            .iter()
            .any(|event| event.get("event").and_then(Value::as_str) == Some(name)),
        "{} missing event {name}; events:\n{}",
        trace.scenario.name(),
        render_events(&trace.events)
    );
}

fn completion_events(trace: &Trace) -> Vec<&Value> {
    trace
        .events
        .iter()
        .filter(|event| {
            matches!(
                string_field(event, "event"),
                Some("ultra_final_acceptance" | "ultra_plan_complete" | "tui_command_stop")
            )
        })
        .collect()
}

fn string_field<'a>(event: &'a Value, key: &str) -> Option<&'a str> {
    event.get(key).and_then(Value::as_str)
}

fn bool_field(event: &Value, key: &str) -> Option<bool> {
    event.get(key).and_then(Value::as_bool)
}

fn u64_field(event: &Value, key: &str) -> Option<u64> {
    event.get(key).and_then(Value::as_u64)
}

fn string_array(event: &Value, key: &str) -> Vec<String> {
    event
        .get(key)
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .map(str::to_string)
        .collect()
}

fn disconnected_gate_status(status: &str) -> bool {
    matches!(status, "" | "not_applicable" | "not_checked" | "skipped")
}

fn merge_strings(target: &mut Vec<String>, values: Vec<String>) {
    for value in values {
        if !target.contains(&value) {
            target.push(value);
        }
    }
}

fn assert_subset(
    contract: &str,
    key: &str,
    expected_subset: &[String],
    actual: &[String],
) -> Result<(), String> {
    for expected in expected_subset {
        if !actual.contains(expected) {
            return Err(format!(
                "{contract}: {key} dropped prior requirement {expected:?}; final={actual:?}"
            ));
        }
    }
    Ok(())
}

fn dependency_blocked_status(status: &str) -> bool {
    status.contains("dependency_missing")
        || status.contains("setup_blocked")
        || status.contains("authority_required")
        || status.contains("blocked")
}

fn classification_requires_pairing(event: &Value) -> bool {
    let event_name = string_field(event, "event").unwrap_or("");
    if matches!(
        event_name,
        "ultra_final_acceptance_failed"
            | "step_verify_failure"
            | "final_acceptance_repair_failed"
            | "ultra_phase_failed"
            | "repair_unreachable"
    ) {
        return true;
    }
    if event_name == "tui_command_stop" && bool_field(event, "ok") == Some(false) {
        return true;
    }
    string_field(event, "failure_kind").is_some_and(|kind| !kind.is_empty() && kind != "none")
}

fn event_has_repair_or_handoff(event: &Value) -> bool {
    string_field(event, "repair_target")
        .is_some_and(|value| !value.is_empty() && value != "unknown" && value != "none")
        || string_field(event, "recovery_handoff_kind").is_some_and(|value| !value.is_empty())
        || string_field(event, "recovery_prompt_path").is_some_and(|value| !value.is_empty())
        || string_field(event, "suggested_recovery_yaml_command")
            .is_some_and(|value| !value.is_empty())
        || string_field(event, "reason")
            .is_some_and(|reason| reason.contains("dependency_setup_authority_required"))
}

fn has_later_handoff_or_repair(events: &[Value]) -> bool {
    events.iter().any(|event| {
        matches!(
            string_field(event, "event"),
            Some(
                "repair_unreachable"
                    | "recovery_prompt_saved"
                    | "final_acceptance_repair_start"
                    | "step_verify_repair"
            )
        ) || event_has_repair_or_handoff(event)
    })
}

fn render_events(events: &[Value]) -> String {
    events
        .iter()
        .map(Value::to_string)
        .collect::<Vec<_>>()
        .join("\n")
}
