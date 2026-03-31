mod common;

use anvil::agent::{AgentEvent, AgentRuntime, AgentRuntimeScript};
use anvil::contracts::{AppEvent, RuntimeState};
use anvil::tui::Tui;

#[test]
fn runtime_turn_pauses_for_single_tool_call_approval_and_resumes_to_done() {
    let mut app = common::build_app();
    let tui = Tui::new();
    let runtime = AgentRuntime::from_script(AgentRuntimeScript::new(vec![
        AgentEvent::Thinking {
            status: "Thinking. model=local-default".to_string(),
            plan_items: vec![
                "inspect repository structure".to_string(),
                "edit session model".to_string(),
            ],
            active_index: Some(1),
            reasoning_summary: vec!["approval is required before write".to_string()],
            elapsed_ms: 120,
        },
        AgentEvent::ApprovalRequested {
            status: "Awaiting approval for 1 tool call".to_string(),
            tool_name: "Write".to_string(),
            summary: "Update src/session/mod.rs".to_string(),
            risk: "Confirm".to_string(),
            tool_call_id: "call_001".to_string(),
            elapsed_ms: 260,
        },
        AgentEvent::Working {
            status: "Working on tool execution".to_string(),
            plan_items: vec![
                "inspect repository structure".to_string(),
                "edit session model".to_string(),
            ],
            active_index: Some(1),
            tool_logs: vec![(
                "Write".to_string(),
                "update".to_string(),
                "src/session/mod.rs".to_string(),
            )],
            elapsed_ms: 540,
        },
        AgentEvent::Done {
            status: "Done. session saved".to_string(),
            assistant_message: "session flow is now runtime-driven".to_string(),
            completion_summary: "Updated the session model and saved the session.".to_string(),
            saved_status: "session saved".to_string(),
            tool_logs: vec![(
                "Write".to_string(),
                "update".to_string(),
                "src/session/mod.rs".to_string(),
            )],
            elapsed_ms: 920,
            inference_performance: None,
        },
    ]));

    let paused = app
        .run_runtime_turn("wire runtime flow", &runtime, &tui)
        .expect("runtime turn should pause for approval");

    assert_eq!(
        app.state_machine().snapshot().state,
        RuntimeState::AwaitingApproval
    );
    assert!(app.has_pending_runtime_events());
    assert!(
        paused
            .iter()
            .any(|frame| frame.contains("[A] anvil > approval"))
    );
    assert_eq!(
        app.session()
            .messages
            .last()
            .expect("message should exist")
            .content,
        "wire runtime flow"
    );
    assert!(matches!(
        app.run_runtime_turn("new request before approval", &runtime, &tui),
        Err(anvil::app::AppError::PendingApprovalRequired)
    ));

    let resumed = app
        .approve_and_continue(&runtime, &tui)
        .expect("runtime turn should resume after approval");

    assert_eq!(app.state_machine().snapshot().state, RuntimeState::Done);
    assert!(!app.has_pending_runtime_events());
    assert!(
        resumed
            .iter()
            .any(|frame| frame.contains("[A] anvil > result"))
    );
    // Assistant message is excluded from frame rendering (streamed to stderr,
    // Issue #1). Verify it is in session but not in the frame.
    assert!(
        resumed
            .last()
            .expect("done frame should exist")
            .contains("[A] anvil > result"),
        "done frame should contain result section"
    );
    assert!(
        app.session()
            .messages
            .iter()
            .any(|m| m.content == "session flow is now runtime-driven"),
        "assistant message should be in session history"
    );
}

#[test]
fn runtime_turn_can_interrupt_and_reset_to_ready() {
    let mut app = common::build_app();
    let tui = Tui::new();
    let runtime = AgentRuntime::from_script(AgentRuntimeScript::new(vec![
        AgentEvent::Thinking {
            status: "Thinking. model=local-default".to_string(),
            plan_items: vec!["inspect runtime".to_string()],
            active_index: Some(0),
            reasoning_summary: vec!["user requested stop".to_string()],
            elapsed_ms: 90,
        },
        AgentEvent::Interrupted {
            status: "Interrupted safely".to_string(),
            interrupted_what: "provider turn".to_string(),
            saved_status: "session preserved".to_string(),
            next_actions: vec!["resume work".to_string(), "inspect status".to_string()],
            elapsed_ms: 180,
        },
    ]));

    let frames = app
        .run_runtime_turn("stop after analysis", &runtime, &tui)
        .expect("runtime turn should complete with interruption");

    assert_eq!(
        app.state_machine().snapshot().state,
        RuntimeState::Interrupted
    );
    assert!(
        frames
            .iter()
            .any(|frame| frame.contains("[A] anvil > interrupted"))
    );
    assert_eq!(
        app.session().event_log.last().copied(),
        Some(AppEvent::SessionNormalizedAfterInterrupt)
    );

    let ready = app.reset_to_ready().expect("reset should succeed");
    assert_eq!(ready.state, RuntimeState::Ready);
}

#[test]
fn runtime_turn_can_deny_approval_and_return_to_ready() {
    let mut app = common::build_app();
    let tui = Tui::new();
    let runtime = AgentRuntime::from_script(AgentRuntimeScript::new(vec![
        AgentEvent::Thinking {
            status: "Thinking. model=local-default".to_string(),
            plan_items: vec!["prepare write".to_string()],
            active_index: Some(0),
            reasoning_summary: vec!["write needs confirmation".to_string()],
            elapsed_ms: 100,
        },
        AgentEvent::ApprovalRequested {
            status: "Awaiting approval for 1 tool call".to_string(),
            tool_name: "Write".to_string(),
            summary: "Update ANVIL.md".to_string(),
            risk: "Confirm".to_string(),
            tool_call_id: "call_deny_001".to_string(),
            elapsed_ms: 220,
        },
        AgentEvent::Done {
            status: "Done. session saved".to_string(),
            assistant_message: "this should not be emitted".to_string(),
            completion_summary: "unexpected completion".to_string(),
            saved_status: "session saved".to_string(),
            tool_logs: Vec::new(),
            elapsed_ms: 400,
            inference_performance: None,
        },
    ]));

    let _ = app
        .run_runtime_turn("deny this write", &runtime, &tui)
        .expect("runtime turn should pause for approval");

    let denied = app
        .deny_and_abort(&tui)
        .expect("deny should return to ready");

    assert_eq!(app.state_machine().snapshot().state, RuntimeState::Ready);
    assert!(!app.has_pending_runtime_events());
    assert!(
        denied
            .last()
            .expect("ready frame should exist")
            .contains("Approval denied")
    );
}

#[test]
fn runtime_turn_supports_multiple_approvals_in_one_turn() {
    let mut app = common::build_app();
    let tui = Tui::new();
    let runtime = AgentRuntime::from_script(AgentRuntimeScript::new(vec![
        AgentEvent::Thinking {
            status: "Thinking. model=local-default".to_string(),
            plan_items: vec![
                "prepare first write".to_string(),
                "prepare second write".to_string(),
            ],
            active_index: Some(0),
            reasoning_summary: vec!["two writes require confirmation".to_string()],
            elapsed_ms: 100,
        },
        AgentEvent::ApprovalRequested {
            status: "Awaiting approval for 1 tool call".to_string(),
            tool_name: "Write".to_string(),
            summary: "Update src/app/mod.rs".to_string(),
            risk: "Confirm".to_string(),
            tool_call_id: "call_multi_001".to_string(),
            elapsed_ms: 180,
        },
        AgentEvent::Working {
            status: "Working on first tool execution".to_string(),
            plan_items: vec![
                "prepare first write".to_string(),
                "prepare second write".to_string(),
            ],
            active_index: Some(0),
            tool_logs: vec![(
                "Write".to_string(),
                "update".to_string(),
                "src/app/mod.rs".to_string(),
            )],
            elapsed_ms: 260,
        },
        AgentEvent::Thinking {
            status: "Thinking after first approval".to_string(),
            plan_items: vec![
                "prepare first write".to_string(),
                "prepare second write".to_string(),
            ],
            active_index: Some(1),
            reasoning_summary: vec!["second write still needs confirmation".to_string()],
            elapsed_ms: 300,
        },
        AgentEvent::ApprovalRequested {
            status: "Awaiting approval for 1 tool call".to_string(),
            tool_name: "Write".to_string(),
            summary: "Update src/session/mod.rs".to_string(),
            risk: "Confirm".to_string(),
            tool_call_id: "call_multi_002".to_string(),
            elapsed_ms: 320,
        },
        AgentEvent::Working {
            status: "Working on second tool execution".to_string(),
            plan_items: vec![
                "prepare first write".to_string(),
                "prepare second write".to_string(),
            ],
            active_index: Some(1),
            tool_logs: vec![(
                "Write".to_string(),
                "update".to_string(),
                "src/session/mod.rs".to_string(),
            )],
            elapsed_ms: 420,
        },
        AgentEvent::Done {
            status: "Done. session saved".to_string(),
            assistant_message: "both approvals were processed".to_string(),
            completion_summary: "Completed the requested writes.".to_string(),
            saved_status: "session saved".to_string(),
            tool_logs: vec![(
                "Write".to_string(),
                "update".to_string(),
                "src/session/mod.rs".to_string(),
            )],
            elapsed_ms: 640,
            inference_performance: None,
        },
    ]));

    let _ = app
        .run_runtime_turn("apply two writes", &runtime, &tui)
        .expect("first approval pause should succeed");
    assert_eq!(
        app.state_machine().snapshot().state,
        RuntimeState::AwaitingApproval
    );
    assert!(app.has_pending_runtime_events());

    let resumed_once = app
        .approve_and_continue(&runtime, &tui)
        .expect("second approval pause should succeed");
    assert!(
        resumed_once
            .iter()
            .any(|frame| frame.contains("call_multi_002"))
    );
    assert_eq!(
        app.state_machine().snapshot().state,
        RuntimeState::AwaitingApproval
    );
    assert!(app.has_pending_runtime_events());

    let resumed_twice = app
        .approve_and_continue(&runtime, &tui)
        .expect("final completion should succeed");
    assert_eq!(app.state_machine().snapshot().state, RuntimeState::Done);
    assert!(!app.has_pending_runtime_events());
    // Assistant message is excluded from frame rendering (streamed to stderr,
    // Issue #1). Verify it is in session but not in the frame.
    assert!(
        resumed_twice
            .last()
            .expect("done frame should exist")
            .contains("[A] anvil > result"),
        "done frame should contain result section"
    );
    assert!(
        app.session()
            .messages
            .iter()
            .any(|m| m.content == "both approvals were processed"),
        "assistant message should be in session history"
    );
}

#[test]
fn runtime_turn_supports_working_back_to_thinking_before_done() {
    let mut app = common::build_app();
    let tui = Tui::new();
    let runtime = AgentRuntime::from_script(AgentRuntimeScript::new(vec![
        AgentEvent::Thinking {
            status: "Thinking. model=local-default".to_string(),
            plan_items: vec!["inspect".to_string(), "summarize".to_string()],
            active_index: Some(0),
            reasoning_summary: vec!["starting analysis".to_string()],
            elapsed_ms: 80,
        },
        AgentEvent::Working {
            status: "Working on repository scan".to_string(),
            plan_items: vec!["inspect".to_string(), "summarize".to_string()],
            active_index: Some(0),
            tool_logs: vec![(
                "Read".to_string(),
                "open".to_string(),
                "src/app/mod.rs".to_string(),
            )],
            elapsed_ms: 160,
        },
        AgentEvent::Thinking {
            status: "Thinking after tool results".to_string(),
            plan_items: vec!["inspect".to_string(), "summarize".to_string()],
            active_index: Some(1),
            reasoning_summary: vec!["tool output is sufficient".to_string()],
            elapsed_ms: 240,
        },
        AgentEvent::Done {
            status: "Done. session saved".to_string(),
            assistant_message: "analysis resumed after tool execution".to_string(),
            completion_summary: "Summarized the repository scan.".to_string(),
            saved_status: "session saved".to_string(),
            tool_logs: Vec::new(),
            elapsed_ms: 360,
            inference_performance: None,
        },
    ]));

    let frames = app
        .run_runtime_turn("scan and summarize", &runtime, &tui)
        .expect("runtime turn should complete");

    assert_eq!(app.state_machine().snapshot().state, RuntimeState::Done);
    assert!(
        frames
            .iter()
            .any(|frame| frame.contains("Thinking after tool results"))
    );
}

#[test]
fn runtime_turn_can_fail_into_error_state() {
    let mut app = common::build_app();
    let tui = Tui::new();
    let runtime = AgentRuntime::from_script(AgentRuntimeScript::new(vec![
        AgentEvent::Thinking {
            status: "Thinking. model=local-default".to_string(),
            plan_items: vec!["inspect runtime".to_string()],
            active_index: Some(0),
            reasoning_summary: vec!["provider output malformed".to_string()],
            elapsed_ms: 70,
        },
        AgentEvent::Failed {
            status: "Error. runtime turn failed".to_string(),
            error_summary: "provider emitted malformed tool call".to_string(),
            recommended_actions: vec!["retry turn".to_string(), "inspect provider".to_string()],
            elapsed_ms: 140,
        },
    ]));

    let frames = app
        .run_runtime_turn("trigger failure", &runtime, &tui)
        .expect("runtime turn should reach error state");

    assert_eq!(app.state_machine().snapshot().state, RuntimeState::Error);
    assert!(
        frames
            .last()
            .expect("error frame should exist")
            .contains("[A] anvil > error")
    );
}

#[test]
fn pending_approval_survives_app_reload() {
    let root = common::unique_test_dir("pending_reload");
    let mut app = common::build_app_in(root.clone());
    let tui = Tui::new();
    let runtime = AgentRuntime::from_script(AgentRuntimeScript::new(vec![
        AgentEvent::Thinking {
            status: "Thinking. model=local-default".to_string(),
            plan_items: vec!["prepare write".to_string()],
            active_index: Some(0),
            reasoning_summary: vec!["write needs confirmation".to_string()],
            elapsed_ms: 100,
        },
        AgentEvent::ApprovalRequested {
            status: "Awaiting approval for 1 tool call".to_string(),
            tool_name: "Write".to_string(),
            summary: "Update src/app/mod.rs".to_string(),
            risk: "Confirm".to_string(),
            tool_call_id: "call_resume_001".to_string(),
            elapsed_ms: 200,
        },
        AgentEvent::Working {
            status: "Working on approved tool execution".to_string(),
            plan_items: vec!["prepare write".to_string()],
            active_index: Some(0),
            tool_logs: vec![(
                "Write".to_string(),
                "update".to_string(),
                "src/app/mod.rs".to_string(),
            )],
            elapsed_ms: 260,
        },
        AgentEvent::Done {
            status: "Done. session saved".to_string(),
            assistant_message: "approval resumed after reload".to_string(),
            completion_summary: "Resumed the pending approval path.".to_string(),
            saved_status: "session saved".to_string(),
            tool_logs: Vec::new(),
            elapsed_ms: 320,
            inference_performance: None,
        },
    ]));

    let _ = app
        .run_runtime_turn("persist pending approval", &runtime, &tui)
        .expect("runtime turn should pause");
    assert!(app.has_pending_runtime_events());

    let mut reloaded = common::build_app_in(root);
    assert!(reloaded.has_pending_runtime_events());

    let resumed = reloaded
        .approve_and_continue(&runtime, &tui)
        .expect("reloaded app should resume");
    assert_eq!(
        reloaded.state_machine().snapshot().state,
        RuntimeState::Done
    );
    // Assistant message is excluded from frame rendering (streamed to stderr,
    // Issue #1). Verify it is in session but not in the frame.
    assert!(
        resumed
            .last()
            .expect("done frame should exist")
            .contains("[A] anvil > result"),
        "done frame should contain result section"
    );
    assert!(
        reloaded
            .session()
            .messages
            .iter()
            .any(|m| m.content == "approval resumed after reload"),
        "assistant message should be in session history"
    );
}

// -----------------------------------------------------------------------
// Phase 4.3: System prompt includes image support
// -----------------------------------------------------------------------

#[test]
fn system_prompt_mentions_image_support_for_file_read() {
    use anvil::agent::{ProjectLanguage, tool_protocol_system_prompt_all_tools};
    let prompt = tool_protocol_system_prompt_all_tools(&[ProjectLanguage::Rust], None);
    assert!(
        prompt.contains("image files"),
        "system prompt should mention image support in file.read"
    );
    assert!(
        prompt.contains("PNG"),
        "system prompt should list supported formats"
    );
    assert!(
        prompt.contains("20MB"),
        "system prompt should mention size limit"
    );
}

// -----------------------------------------------------------------------
// Phase 2-3: Sub-agent system prompt and tool descriptions
// -----------------------------------------------------------------------

#[test]
fn system_prompt_includes_agent_explore_and_plan_descriptions() {
    use anvil::agent::{ProjectLanguage, tool_protocol_system_prompt_all_tools};
    let prompt = tool_protocol_system_prompt_all_tools(&[ProjectLanguage::Rust], None);
    assert!(
        prompt.contains("agent.explore"),
        "system prompt should describe agent.explore tool"
    );
    assert!(
        prompt.contains("agent.plan"),
        "system prompt should describe agent.plan tool"
    );
}

#[test]
fn system_prompt_includes_confirm_class_guidance() {
    use anvil::agent::{ProjectLanguage, tool_protocol_system_prompt_all_tools};
    let prompt = tool_protocol_system_prompt_all_tools(&[ProjectLanguage::Rust], None);
    assert!(
        prompt.contains("Tool approval"),
        "system prompt must include confirm-class guidance section"
    );
    assert!(
        prompt.contains("Do NOT ask the user for permission in natural language"),
        "system prompt must instruct LLM not to double-confirm"
    );
    assert!(
        prompt.contains("denied by user"),
        "system prompt must explain denial handling"
    );
}

#[test]
fn build_subagent_system_prompt_explore_contains_expected_tools() {
    use anvil::agent::subagent::{
        SubAgentKind, SubAgentPromptOptions, build_subagent_system_prompt,
    };
    let opts = SubAgentPromptOptions {
        offline: false,
        ui_language: None,
    };
    let prompt = build_subagent_system_prompt(&SubAgentKind::Explore, &opts);
    assert!(
        prompt.contains("file.read"),
        "Explore prompt should include file.read"
    );
    assert!(
        prompt.contains("file.search"),
        "Explore prompt should include file.search"
    );
    assert!(
        !prompt.contains("web.fetch"),
        "Explore prompt should NOT include web.fetch"
    );
    assert!(
        prompt.contains("ANVIL_FINAL"),
        "Explore prompt should mention ANVIL_FINAL"
    );
    assert!(
        prompt.contains("Explore"),
        "Explore prompt should mention Explore role"
    );
}

#[test]
fn build_subagent_system_prompt_plan_contains_expected_tools() {
    use anvil::agent::subagent::{
        SubAgentKind, SubAgentPromptOptions, build_subagent_system_prompt,
    };
    let opts = SubAgentPromptOptions {
        offline: false,
        ui_language: None,
    };
    let prompt = build_subagent_system_prompt(&SubAgentKind::Plan, &opts);
    assert!(
        prompt.contains("file.read"),
        "Plan prompt should include file.read"
    );
    assert!(
        prompt.contains("file.search"),
        "Plan prompt should include file.search"
    );
    assert!(
        prompt.contains("web.fetch"),
        "Plan prompt should include web.fetch"
    );
    assert!(
        prompt.contains("ANVIL_FINAL"),
        "Plan prompt should mention ANVIL_FINAL"
    );
    assert!(
        prompt.contains("Plan"),
        "Plan prompt should mention Plan role"
    );
}

// -----------------------------------------------------------------------
// Issue #129: Sub-agent system prompts include JSON format instructions
// -----------------------------------------------------------------------

#[test]
fn build_subagent_system_prompt_includes_json_format() {
    use anvil::agent::subagent::{
        SubAgentKind, SubAgentPromptOptions, build_subagent_system_prompt,
    };

    let opts = SubAgentPromptOptions {
        offline: false,
        ui_language: None,
    };
    let explore = build_subagent_system_prompt(&SubAgentKind::Explore, &opts);
    assert!(
        explore.contains("found_files"),
        "Explore prompt should mention found_files JSON field"
    );
    assert!(
        explore.contains("key_findings"),
        "Explore prompt should mention key_findings JSON field"
    );
    assert!(
        explore.contains("raw_summary"),
        "Explore prompt should mention raw_summary JSON field"
    );
    assert!(
        explore.contains("confidence"),
        "Explore prompt should mention confidence JSON field"
    );

    let plan = build_subagent_system_prompt(&SubAgentKind::Plan, &opts);
    assert!(
        plan.contains("found_files"),
        "Plan prompt should mention found_files JSON field"
    );
    assert!(
        plan.contains("key_findings"),
        "Plan prompt should mention key_findings JSON field"
    );
    assert!(
        plan.contains("raw_summary"),
        "Plan prompt should mention raw_summary JSON field"
    );
}

// -----------------------------------------------------------------------
// Issue #114: web.search/web.fetch must be in system prompt for fresh sessions
// -----------------------------------------------------------------------

#[test]
fn system_prompt_includes_web_tools_even_with_empty_used_tools() {
    use anvil::agent::{ProjectLanguage, tool_protocol_system_prompt_basic_only};
    // Simulate a fresh session where no tools have been used yet
    let prompt = tool_protocol_system_prompt_basic_only(&[ProjectLanguage::Rust], None);
    assert!(
        prompt.contains("web.search"),
        "fresh session system prompt must include web.search description (Issue #114)"
    );
    assert!(
        prompt.contains("web.fetch"),
        "fresh session system prompt must include web.fetch description (Issue #114)"
    );
}

// --- Issue #160: ANVIL_FINAL後のツール呼び出し除外テスト ---

#[test]
fn post_final_tool_excluded() {
    // TC1: ANVIL_TOOL → ANVIL_FINAL → ANVIL_TOOL — only the first tool should be included
    let response = anvil::agent::BasicAgentLoop::parse_structured_response(concat!(
        "```ANVIL_TOOL\n",
        "{\"id\":\"call_001\",\"tool\":\"file.read\",\"path\":\"./src/main.rs\"}\n",
        "```\n",
        "```ANVIL_FINAL\n",
        "Read the file.\n",
        "```\n",
        "```ANVIL_TOOL\n",
        "{\"id\":\"call_002\",\"tool\":\"file.read\",\"path\":\"./src/lib.rs\"}\n",
        "```\n"
    ))
    .expect("parsing should succeed");

    assert_eq!(
        response.tool_calls.len(),
        1,
        "only pre-FINAL tool should remain"
    );
    assert_eq!(response.tool_calls[0].tool_call_id, "call_001");
}

#[test]
fn pre_final_tools_preserved() {
    // TC2: ANVIL_TOOL → ANVIL_TOOL → ANVIL_FINAL — both tools should be included
    let response = anvil::agent::BasicAgentLoop::parse_structured_response(concat!(
        "```ANVIL_TOOL\n",
        "{\"id\":\"call_001\",\"tool\":\"file.read\",\"path\":\"./src/main.rs\"}\n",
        "```\n",
        "```ANVIL_TOOL\n",
        "{\"id\":\"call_002\",\"tool\":\"file.read\",\"path\":\"./src/lib.rs\"}\n",
        "```\n",
        "```ANVIL_FINAL\n",
        "Read both files.\n",
        "```\n"
    ))
    .expect("parsing should succeed");

    assert_eq!(
        response.tool_calls.len(),
        2,
        "both pre-FINAL tools should remain"
    );
    assert_eq!(response.tool_calls[0].tool_call_id, "call_001");
    assert_eq!(response.tool_calls[1].tool_call_id, "call_002");
}

#[test]
fn no_final_existing_compat() {
    // TC3: ANVIL_TOOL → ANVIL_TOOL, no ANVIL_FINAL — both tools should be included
    let response = anvil::agent::BasicAgentLoop::parse_structured_response(concat!(
        "```ANVIL_TOOL\n",
        "{\"id\":\"call_001\",\"tool\":\"file.read\",\"path\":\"./src/main.rs\"}\n",
        "```\n",
        "```ANVIL_TOOL\n",
        "{\"id\":\"call_002\",\"tool\":\"file.read\",\"path\":\"./src/lib.rs\"}\n",
        "```\n"
    ))
    .expect("parsing should succeed");

    assert_eq!(
        response.tool_calls.len(),
        2,
        "all tools should remain without ANVIL_FINAL"
    );
    assert_eq!(response.tool_calls[0].tool_call_id, "call_001");
    assert_eq!(response.tool_calls[1].tool_call_id, "call_002");
}

#[test]
fn unclosed_final_filters() {
    // TC4: ANVIL_TOOL → ANVIL_FINAL (unclosed) → ANVIL_TOOL — only the first tool
    let response = anvil::agent::BasicAgentLoop::parse_structured_response(concat!(
        "```ANVIL_TOOL\n",
        "{\"id\":\"call_001\",\"tool\":\"file.read\",\"path\":\"./src/main.rs\"}\n",
        "```\n",
        "```ANVIL_FINAL\n",
        "Read the file.\n",
        "```ANVIL_TOOL\n",
        "{\"id\":\"call_002\",\"tool\":\"file.read\",\"path\":\"./src/lib.rs\"}\n",
        "```\n"
    ))
    .expect("parsing should succeed");

    assert_eq!(
        response.tool_calls.len(),
        1,
        "only pre-FINAL tool should remain with unclosed FINAL"
    );
    assert_eq!(response.tool_calls[0].tool_call_id, "call_001");
}

#[test]
fn all_tools_after_final() {
    // TC5: ANVIL_FINAL → ANVIL_TOOL — tool_calls should be empty
    let response = anvil::agent::BasicAgentLoop::parse_structured_response(concat!(
        "```ANVIL_FINAL\n",
        "Done with everything.\n",
        "```\n",
        "```ANVIL_TOOL\n",
        "{\"id\":\"call_001\",\"tool\":\"file.read\",\"path\":\"./src/main.rs\"}\n",
        "```\n"
    ))
    .expect("parsing should succeed");

    assert!(
        response.tool_calls.is_empty(),
        "all post-FINAL tools should be excluded"
    );
}

// --- Issue #173: anvil_final_detected flag tests ---

#[test]
fn anvil_final_detected_with_tool_and_final() {
    // ANVIL_TOOL + ANVIL_FINAL → anvil_final_detected should be true
    let response = anvil::agent::BasicAgentLoop::parse_structured_response(concat!(
        "```ANVIL_TOOL\n",
        "{\"id\":\"call_001\",\"tool\":\"file.read\",\"path\":\"./src/main.rs\"}\n",
        "```\n",
        "```ANVIL_FINAL\n",
        "Done.\n",
        "```\n"
    ))
    .expect("parsing should succeed");

    assert!(
        response.anvil_final_detected,
        "anvil_final_detected should be true when ANVIL_FINAL is present"
    );
    assert_eq!(
        response.tool_calls.len(),
        1,
        "pre-FINAL tool should be included"
    );
}

#[test]
fn anvil_final_detected_without_final() {
    // ANVIL_TOOL only → anvil_final_detected should be false
    let response = anvil::agent::BasicAgentLoop::parse_structured_response(concat!(
        "```ANVIL_TOOL\n",
        "{\"id\":\"call_001\",\"tool\":\"file.read\",\"path\":\"./src/main.rs\"}\n",
        "```\n"
    ))
    .expect("parsing should succeed");

    assert!(
        !response.anvil_final_detected,
        "anvil_final_detected should be false without ANVIL_FINAL"
    );
}

#[test]
fn anvil_final_detected_final_only() {
    // ANVIL_FINAL only (no tools) → anvil_final_detected should be true, tool_calls empty
    let response = anvil::agent::BasicAgentLoop::parse_structured_response(concat!(
        "```ANVIL_FINAL\n",
        "All done.\n",
        "```\n"
    ))
    .expect("parsing should succeed");

    assert!(
        response.anvil_final_detected,
        "anvil_final_detected should be true for ANVIL_FINAL only"
    );
    assert!(response.tool_calls.is_empty());
}

#[test]
fn anvil_final_detected_unclosed_final() {
    // Unclosed ANVIL_FINAL → anvil_final_detected should be true (cutoff marker found)
    let response = anvil::agent::BasicAgentLoop::parse_structured_response(concat!(
        "```ANVIL_TOOL\n",
        "{\"id\":\"call_001\",\"tool\":\"file.read\",\"path\":\"./src/main.rs\"}\n",
        "```\n",
        "```ANVIL_FINAL\n",
        "Done but unclosed"
    ))
    .expect("parsing should succeed");

    assert!(
        response.anvil_final_detected,
        "anvil_final_detected should be true even with unclosed ANVIL_FINAL"
    );
}

#[test]
fn is_complete_structured_response_lenient_unclosed() {
    // Lenient should detect unclosed ANVIL_FINAL
    let content = concat!("```ANVIL_FINAL\n", "Done but no closing tag");
    assert!(
        !anvil::agent::BasicAgentLoop::is_complete_structured_response(content),
        "strict should NOT detect unclosed ANVIL_FINAL"
    );
    assert!(
        anvil::agent::BasicAgentLoop::is_complete_structured_response_lenient(content),
        "lenient should detect unclosed ANVIL_FINAL"
    );
}

#[test]
fn is_complete_structured_response_lenient_closed() {
    // Lenient should also detect closed ANVIL_FINAL
    let content = concat!("```ANVIL_FINAL\n", "Done.\n", "```\n");
    assert!(
        anvil::agent::BasicAgentLoop::is_complete_structured_response(content),
        "strict should detect closed ANVIL_FINAL"
    );
    assert!(
        anvil::agent::BasicAgentLoop::is_complete_structured_response_lenient(content),
        "lenient should also detect closed ANVIL_FINAL"
    );
}

#[test]
fn structured_response_empty_factory() {
    let response = anvil::agent::StructuredAssistantResponse::empty("test response".to_string());
    assert!(response.tool_calls.is_empty());
    assert_eq!(response.final_response, "test response");
    assert!(!response.anvil_final_detected);
}

// --- Issue #128: Multi-tier parsing tests ---

#[test]
fn parse_json_tool_call_unchanged() {
    let response = anvil::agent::BasicAgentLoop::parse_structured_response(concat!(
        "```ANVIL_TOOL\n",
        "{\"id\":\"call_001\",\"tool\":\"file.read\",\"path\":\"./src/main.rs\"}\n",
        "```\n",
        "```ANVIL_FINAL\n",
        "Read the file.\n",
        "```\n"
    ))
    .expect("JSON parsing should work");

    assert_eq!(response.tool_calls.len(), 1);
    assert_eq!(response.tool_calls[0].tool_name, "file.read");
}

#[test]
fn parse_tag_based_tool_call() {
    let response = anvil::agent::BasicAgentLoop::parse_structured_response(concat!(
        "```ANVIL_TOOL\n",
        "<tool name=\"file.read\" path=\"./src/main.rs\"/>\n",
        "```\n",
        "```ANVIL_FINAL\n",
        "Read the file.\n",
        "```\n"
    ))
    .expect("Tag-based parsing should work");

    assert_eq!(response.tool_calls.len(), 1);
    assert_eq!(response.tool_calls[0].tool_name, "file.read");
    assert_eq!(
        response.tool_calls[0].tool_call_id, "tag_file_read",
        "tag-based tool calls should have tag_ prefixed id"
    );
}

#[test]
fn parse_tag_based_file_edit() {
    let response = anvil::agent::BasicAgentLoop::parse_structured_response(concat!(
        "```ANVIL_TOOL\n",
        "<tool name=\"file.edit\" path=\"./src/main.rs\"><old_string>fn old()</old_string><new_string>fn new()</new_string></tool>\n",
        "```\n",
        "```ANVIL_FINAL\n",
        "Edited the file.\n",
        "```\n"
    ))
    .expect("Tag-based file.edit parsing should work");

    assert_eq!(response.tool_calls.len(), 1);
    assert_eq!(response.tool_calls[0].tool_name, "file.edit");
}

#[test]
fn parse_tag_based_file_edit_anchor() {
    let response = anvil::agent::BasicAgentLoop::parse_structured_response(concat!(
        "```ANVIL_TOOL\n",
        "<tool name=\"file.edit_anchor\" path=\"./src/main.rs\"><old_content>fn old()</old_content><new_content>fn new()</new_content></tool>\n",
        "```\n",
        "```ANVIL_FINAL\n",
        "Edited the file with anchor.\n",
        "```\n"
    ))
    .expect("Tag-based file.edit_anchor parsing should work");

    assert_eq!(response.tool_calls.len(), 1);
    assert_eq!(response.tool_calls[0].tool_name, "file.edit_anchor");
}

#[test]
fn parse_malformed_rejected() {
    let result = anvil::agent::BasicAgentLoop::parse_structured_response(concat!(
        "```ANVIL_TOOL\n",
        "this is not valid json or tag format\n",
        "```\n",
        "```ANVIL_FINAL\n",
        "Done.\n",
        "```\n"
    ));

    assert!(result.is_err(), "malformed tool block should be rejected");
}

#[test]
fn tag_protocol_prompt_contains_tag_examples() {
    let prompt = anvil::agent::tool_protocol_system_prompt_tag_based(&[], None);
    assert!(
        prompt.contains("<tool name="),
        "tag-based prompt should contain tag examples"
    );
    assert!(
        prompt.contains("file.read"),
        "tag-based prompt should mention file.read"
    );
    assert!(
        prompt.contains("file.edit_anchor"),
        "tag-based prompt should mention file.edit_anchor"
    );
}

// --- Issue #186: 同一ターン内の重複ツール呼び出し排除テスト ---

#[test]
fn dedup_identical_tool_calls_in_single_turn() {
    // 同一ID・同一内容のツール呼び出しが2回出現 → 1回に削減
    let response = anvil::agent::BasicAgentLoop::parse_structured_response(concat!(
        "```ANVIL_TOOL\n",
        "{\"id\":\"call_001\",\"tool\":\"file.edit\",\"path\":\"./src/main.rs\",\"old_string\":\"old\",\"new_string\":\"new\"}\n",
        "```\n",
        "Now I'll implement the next step...\n",
        "```ANVIL_TOOL\n",
        "{\"id\":\"call_001\",\"tool\":\"file.edit\",\"path\":\"./src/main.rs\",\"old_string\":\"old\",\"new_string\":\"new\"}\n",
        "```\n"
    ))
    .expect("parsing should succeed");

    assert_eq!(
        response.tool_calls.len(),
        1,
        "duplicate tool call should be removed"
    );
}

#[test]
fn dedup_preserves_different_tools() {
    // 異なるツール呼び出しは保持される
    let response = anvil::agent::BasicAgentLoop::parse_structured_response(concat!(
        "```ANVIL_TOOL\n",
        "{\"id\":\"call_001\",\"tool\":\"file.read\",\"path\":\"./src/main.rs\"}\n",
        "```\n",
        "```ANVIL_TOOL\n",
        "{\"id\":\"call_002\",\"tool\":\"file.read\",\"path\":\"./src/lib.rs\"}\n",
        "```\n"
    ))
    .expect("parsing should succeed");

    assert_eq!(
        response.tool_calls.len(),
        2,
        "different tool calls should be preserved"
    );
}

#[test]
fn dedup_same_tool_different_input_preserved() {
    // 同一ツール名でもinputが異なれば保持
    let response = anvil::agent::BasicAgentLoop::parse_structured_response(concat!(
        "```ANVIL_TOOL\n",
        "{\"id\":\"call_001\",\"tool\":\"file.read\",\"path\":\"./src/main.rs\"}\n",
        "```\n",
        "```ANVIL_TOOL\n",
        "{\"id\":\"call_001\",\"tool\":\"file.read\",\"path\":\"./src/lib.rs\"}\n",
        "```\n"
    ))
    .expect("parsing should succeed");

    assert_eq!(
        response.tool_calls.len(),
        2,
        "same tool with different input should be preserved"
    );
}

#[test]
fn dedup_renumbers_duplicate_ids() {
    // 同一IDが異なるツール呼び出しに使用されている場合、リナンバリング
    let response = anvil::agent::BasicAgentLoop::parse_structured_response(concat!(
        "```ANVIL_TOOL\n",
        "{\"id\":\"call_001\",\"tool\":\"file.read\",\"path\":\"./src/main.rs\"}\n",
        "```\n",
        "```ANVIL_TOOL\n",
        "{\"id\":\"call_001\",\"tool\":\"file.read\",\"path\":\"./src/lib.rs\"}\n",
        "```\n"
    ))
    .expect("parsing should succeed");

    assert_eq!(response.tool_calls.len(), 2);
    assert_eq!(response.tool_calls[0].tool_call_id, "call_001");
    assert_ne!(
        response.tool_calls[1].tool_call_id, "call_001",
        "duplicate ID should be renumbered"
    );
}

#[test]
fn dedup_issue_186_reproduction() {
    // Issue #186の再現ケース: z-index.ts edit → TerminalDisplay.tsx edit → 計画テキスト → TerminalDisplay.tsx edit(重複)
    let response = anvil::agent::BasicAgentLoop::parse_structured_response(concat!(
        "```ANVIL_TOOL\n",
        "{\"id\":\"call_001\",\"tool\":\"file.edit\",\"path\":\"./src/config/z-index.ts\",\"old_string\":\"a\",\"new_string\":\"b\"}\n",
        "```\n",
        "```ANVIL_TOOL\n",
        "{\"id\":\"call_001\",\"tool\":\"file.edit\",\"path\":\"./src/components/TerminalDisplay.tsx\",\"old_string\":\"c\",\"new_string\":\"d\"}\n",
        "```\n",
        "Now I'll implement the fullscreen functionality...\n",
        "1. I've added MAXIMIZED_TERMINAL: 56 to z-index.ts\n",
        "2. I need to add fullscreen button\n",
        "```ANVIL_TOOL\n",
        "{\"id\":\"call_001\",\"tool\":\"file.edit\",\"path\":\"./src/components/TerminalDisplay.tsx\",\"old_string\":\"c\",\"new_string\":\"d\"}\n",
        "```\n"
    ))
    .expect("parsing should succeed");

    // z-index.ts edit + TerminalDisplay.tsx edit = 2 (3番目は重複なので除去)
    assert_eq!(
        response.tool_calls.len(),
        2,
        "duplicate TerminalDisplay.tsx edit should be removed"
    );
}

#[test]
fn dedup_with_anvil_final_combined() {
    // 重複排除とANVIL_FINALカットオフが正しく組み合わさる
    let response = anvil::agent::BasicAgentLoop::parse_structured_response(concat!(
        "```ANVIL_TOOL\n",
        "{\"id\":\"call_001\",\"tool\":\"file.read\",\"path\":\"./src/main.rs\"}\n",
        "```\n",
        "```ANVIL_TOOL\n",
        "{\"id\":\"call_001\",\"tool\":\"file.read\",\"path\":\"./src/main.rs\"}\n",
        "```\n",
        "```ANVIL_FINAL\n",
        "Done.\n",
        "```\n",
        "```ANVIL_TOOL\n",
        "{\"id\":\"call_003\",\"tool\":\"file.read\",\"path\":\"./src/lib.rs\"}\n",
        "```\n"
    ))
    .expect("parsing should succeed");

    // 1st call kept, 2nd is duplicate (removed), 3rd is post-FINAL (excluded)
    assert_eq!(response.tool_calls.len(), 1);
    assert_eq!(response.tool_calls[0].tool_call_id, "call_001");
}

// ==========================================
// Issue #220: Parallel Progress Integration Tests
// ==========================================

#[test]
fn parallel_progress_entry_thread_safe_sharing() {
    use anvil::tooling::{ToolProgressEntry, ToolProgressStatus};
    use std::sync::{Arc, Mutex};

    let progress = Arc::new(Mutex::new(vec![
        ToolProgressEntry {
            tool_call_id: "c1".to_string(),
            tool_name: "file.read".to_string(),
            status: ToolProgressStatus::Pending,
            started_at: None,
            elapsed_ms: None,
        },
        ToolProgressEntry {
            tool_call_id: "c2".to_string(),
            tool_name: "git.status".to_string(),
            status: ToolProgressStatus::Pending,
            started_at: None,
            elapsed_ms: None,
        },
    ]));

    // Simulate concurrent writes from multiple threads
    let handles: Vec<_> = (0..2)
        .map(|i| {
            let progress = progress.clone();
            std::thread::spawn(move || {
                let started = std::time::Instant::now();
                {
                    let mut entries = progress.lock().unwrap();
                    entries[i].status = ToolProgressStatus::Running;
                    entries[i].started_at = Some(started);
                }
                // Simulate some work
                std::thread::sleep(std::time::Duration::from_millis(10));
                {
                    let mut entries = progress.lock().unwrap();
                    entries[i].status = ToolProgressStatus::Completed;
                    entries[i].elapsed_ms = Some(started.elapsed().as_millis());
                }
            })
        })
        .collect();

    for h in handles {
        h.join().unwrap();
    }

    let entries = progress.lock().unwrap();
    assert_eq!(entries[0].status, ToolProgressStatus::Completed);
    assert_eq!(entries[1].status, ToolProgressStatus::Completed);
    assert!(entries[0].elapsed_ms.is_some());
    assert!(entries[1].elapsed_ms.is_some());
}

#[test]
fn parallel_progress_failure_does_not_block_others() {
    use anvil::tooling::{ToolProgressEntry, ToolProgressStatus};
    use std::sync::{Arc, Mutex};

    let progress = Arc::new(Mutex::new(vec![
        ToolProgressEntry {
            tool_call_id: "c1".to_string(),
            tool_name: "file.read".to_string(),
            status: ToolProgressStatus::Pending,
            started_at: None,
            elapsed_ms: None,
        },
        ToolProgressEntry {
            tool_call_id: "c2".to_string(),
            tool_name: "git.status".to_string(),
            status: ToolProgressStatus::Pending,
            started_at: None,
            elapsed_ms: None,
        },
    ]));

    let handles: Vec<_> = (0..2)
        .map(|i| {
            let progress = progress.clone();
            std::thread::spawn(move || {
                {
                    let mut entries = progress.lock().unwrap();
                    entries[i].status = ToolProgressStatus::Running;
                    entries[i].started_at = Some(std::time::Instant::now());
                }
                if i == 0 {
                    // Simulate failure
                    let mut entries = progress.lock().unwrap();
                    entries[i].status = ToolProgressStatus::Failed("simulated error".to_string());
                    entries[i].elapsed_ms = Some(100);
                } else {
                    // Simulate success
                    std::thread::sleep(std::time::Duration::from_millis(10));
                    let mut entries = progress.lock().unwrap();
                    entries[i].status = ToolProgressStatus::Completed;
                    entries[i].elapsed_ms = Some(50);
                }
            })
        })
        .collect();

    for h in handles {
        h.join().unwrap();
    }

    let entries = progress.lock().unwrap();
    assert!(matches!(entries[0].status, ToolProgressStatus::Failed(_)));
    assert_eq!(entries[1].status, ToolProgressStatus::Completed);
}
