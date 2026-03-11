use std::sync::{Arc, Mutex};

use anvil::agent::looping::{
    LoopConfig, LoopDriver, LoopError, LoopEvent, ModelExchange, ModelTurn,
};
use anvil::models::tool_calling::{NativeModelResponse, NativeToolCall, NativeToolSpec};
use tempfile::tempdir;

#[derive(Clone, Default)]
struct ScriptedModel {
    replies: Arc<Mutex<Vec<String>>>,
    prompts: Arc<Mutex<Vec<String>>>,
}

impl ScriptedModel {
    fn new(replies: Vec<String>) -> Self {
        Self {
            replies: Arc::new(Mutex::new(replies.into_iter().rev().collect())),
            prompts: Arc::new(Mutex::new(Vec::new())),
        }
    }

    fn prompts(&self) -> Vec<String> {
        self.prompts.lock().unwrap().clone()
    }
}

#[async_trait::async_trait]
impl ModelExchange for ScriptedModel {
    async fn complete(&self, prompt: &str) -> anyhow::Result<String> {
        self.prompts.lock().unwrap().push(prompt.to_string());
        self.replies
            .lock()
            .unwrap()
            .pop()
            .ok_or_else(|| anyhow::anyhow!("no scripted reply left"))
    }
}

#[derive(Clone, Default)]
struct ScriptedNativeModel {
    replies: Arc<Mutex<Vec<NativeModelResponse>>>,
    prompts: Arc<Mutex<Vec<String>>>,
}

impl ScriptedNativeModel {
    fn new(replies: Vec<NativeModelResponse>) -> Self {
        Self {
            replies: Arc::new(Mutex::new(replies.into_iter().rev().collect())),
            prompts: Arc::new(Mutex::new(Vec::new())),
        }
    }
}

#[async_trait::async_trait]
impl ModelExchange for ScriptedNativeModel {
    async fn complete(&self, prompt: &str) -> anyhow::Result<String> {
        self.prompts.lock().unwrap().push(prompt.to_string());
        Err(anyhow::anyhow!("text path should not be used"))
    }

    async fn complete_with_tools(
        &self,
        prompt: &str,
        _tools: &[NativeToolSpec],
    ) -> anyhow::Result<Option<NativeModelResponse>> {
        self.prompts.lock().unwrap().push(prompt.to_string());
        Ok(Some(self.replies.lock().unwrap().pop().ok_or_else(
            || anyhow::anyhow!("no scripted native reply left"),
        )?))
    }
}

#[tokio::test]
async fn loop_executes_read_then_returns_final_answer() {
    let dir = tempdir().unwrap();
    let file = dir.path().join("README.md");
    std::fs::write(&file, "hello branch\n").unwrap();

    let model = ScriptedModel::new(vec![
        r#"{"type":"tool_calls","calls":[{"tool":"read_file","args":{"path":"README.md"}}]}"#
            .to_string(),
        r#"{"type":"final","content":"README.md says hello branch"}"#.to_string(),
    ]);
    let driver = LoopDriver::new(LoopConfig::default());

    let out = driver
        .run(
            &model,
            dir.path(),
            "Explain the readme",
            Vec::<ModelTurn>::new(),
        )
        .await
        .unwrap();

    assert_eq!(out.final_text, "README.md says hello branch");
    let prompts = model.prompts();
    assert_eq!(prompts.len(), 2);
    assert!(prompts[1].contains("TOOL_RESULT"));
    assert!(prompts[1].contains("hello branch"));
}

#[tokio::test]
async fn loop_accepts_native_final_tool_call() {
    let dir = tempdir().unwrap();
    let model =
        ScriptedNativeModel::new(vec![NativeModelResponse::ToolCalls(vec![NativeToolCall {
            id: None,
            name: "final".to_string(),
            arguments: serde_json::json!({ "content": "native final answer" }),
        }])]);
    let driver = LoopDriver::new(LoopConfig::default());

    let out = driver
        .run(
            &model,
            dir.path(),
            "Explain the branch",
            Vec::<ModelTurn>::new(),
        )
        .await
        .unwrap();

    assert_eq!(out.final_text, "native final answer");
}

#[tokio::test]
async fn loop_rejects_final_before_required_write_and_recovers() {
    let dir = tempdir().unwrap();
    let model = ScriptedModel::new(vec![
        r#"{"type":"final","content":"I need to create the file first."}"#.to_string(),
        r#"{"type":"tool_calls","calls":[{"tool":"write_file","args":{"path":"index.html","content":"<html></html>"}}]}"#
            .to_string(),
        r#"{"type":"final","content":"created index.html"}"#.to_string(),
    ]);
    let driver = LoopDriver::new(LoopConfig::default());

    let out = driver
        .run(
            &model,
            dir.path(),
            "index.html を作成して出力してください",
            Vec::<ModelTurn>::new(),
        )
        .await
        .unwrap();

    assert_eq!(out.final_text, "created index.html");
    let prompts = model.prompts();
    assert!(prompts[1].contains("final_without_action"));
    assert!(std::fs::read_to_string(dir.path().join("index.html")).is_ok());
}

#[tokio::test]
async fn loop_accepts_flat_write_file_tool_call_without_args_wrapper() {
    let dir = tempdir().unwrap();
    let model = ScriptedModel::new(vec![
        r#"{"type":"tool_calls","calls":[{"tool":"write_file","path":"flat.html","content":"<html>flat</html>"}]}"#
            .to_string(),
        r#"{"type":"final","content":"created flat.html"}"#.to_string(),
    ]);
    let driver = LoopDriver::new(LoopConfig::default());

    let out = driver
        .run(
            &model,
            dir.path(),
            "flat.html を作成して出力してください",
            Vec::<ModelTurn>::new(),
        )
        .await
        .unwrap();

    assert_eq!(out.final_text, "created flat.html");
    assert_eq!(
        std::fs::read_to_string(dir.path().join("flat.html")).unwrap(),
        "<html>flat</html>"
    );
}

#[tokio::test]
async fn loop_can_explain_branch_via_git_commands_without_rules() {
    let dir = tempdir().unwrap();
    std::process::Command::new("git")
        .args(["init", "-b", "feature/test"])
        .current_dir(dir.path())
        .output()
        .unwrap();
    std::process::Command::new("git")
        .args(["config", "user.email", "test@example.com"])
        .current_dir(dir.path())
        .output()
        .unwrap();
    std::process::Command::new("git")
        .args(["config", "user.name", "Tester"])
        .current_dir(dir.path())
        .output()
        .unwrap();
    std::fs::write(dir.path().join("app.txt"), "hello\n").unwrap();
    std::process::Command::new("git")
        .args(["add", "."])
        .current_dir(dir.path())
        .output()
        .unwrap();
    std::process::Command::new("git")
        .args(["commit", "-m", "initial"])
        .current_dir(dir.path())
        .output()
        .unwrap();
    std::fs::write(dir.path().join("app.txt"), "hello branch\n").unwrap();

    let model = ScriptedModel::new(vec![
        r#"{"type":"tool_calls","calls":[{"tool":"exec","args":{"argv":["git","branch","--show-current"]}},{"tool":"exec","args":{"argv":["git","status","--short"]}},{"tool":"exec","args":{"argv":["git","log","--oneline","-1"]}}]}"#.to_string(),
        r#"{"type":"final","content":"Current branch is feature/test with one modified file and one initial commit."}"#.to_string(),
    ]);
    let driver = LoopDriver::new(LoopConfig::default());

    let out = driver
        .run(
            &model,
            dir.path(),
            "このブランチを解説して",
            Vec::<ModelTurn>::new(),
        )
        .await
        .unwrap();

    assert!(out.final_text.contains("feature/test"));
    let prompts = model.prompts();
    assert!(prompts[1].contains("feature/test"));
    assert!(prompts[1].contains("app.txt"));
}

#[tokio::test]
async fn loop_accepts_safe_exec_command_and_normalizes_to_argv() {
    let dir = tempdir().unwrap();
    let model = ScriptedModel::new(vec![
        r#"{"type":"tool_calls","calls":[{"tool":"exec","args":{"command":"git status --short"}}]}"#
            .to_string(),
        r#"{"type":"final","content":"done"}"#.to_string(),
    ]);
    let driver = LoopDriver::new(LoopConfig::default());

    let out = driver
        .run(&model, dir.path(), "inspect repo", Vec::<ModelTurn>::new())
        .await
        .unwrap();

    assert_eq!(out.final_text, "done");
    let prompts = model.prompts();
    assert!(prompts[1].contains("status="));
}

#[tokio::test]
async fn loop_rejects_shell_style_exec_command() {
    let dir = tempdir().unwrap();
    let model = ScriptedModel::new(vec![
        r#"{"type":"tool_calls","calls":[{"tool":"exec","args":{"command":"git status | cat"}}]}"#
            .to_string(),
        r#"{"type":"tool_calls","calls":[{"tool":"exec","args":{"command":"git status | cat"}}]}"#
            .to_string(),
    ]);
    let driver = LoopDriver::new(LoopConfig::default());

    let err = driver
        .run(&model, dir.path(), "inspect repo", Vec::<ModelTurn>::new())
        .await
        .unwrap_err();

    assert!(matches!(err, LoopError::InvalidToolCall(_)));
}

#[tokio::test]
async fn loop_reuses_duplicate_read_only_tool_calls() {
    let dir = tempdir().unwrap();
    std::fs::write(dir.path().join("a.txt"), "x\n").unwrap();
    let model = ScriptedModel::new(vec![
        r#"{"type":"tool_calls","calls":[{"tool":"read_file","args":{"path":"a.txt"}}]}"#
            .to_string(),
        r#"{"type":"tool_calls","calls":[{"tool":"read_file","args":{"path":"a.txt"}}]}"#
            .to_string(),
        r#"{"type":"final","content":"done after reread"}"#.to_string(),
    ]);
    let driver = LoopDriver::new(LoopConfig::default());

    let out = driver
        .run(&model, dir.path(), "loop forever", Vec::<ModelTurn>::new())
        .await
        .unwrap();

    assert_eq!(out.final_text, "done after reread");
    let prompts = model.prompts();
    assert!(prompts[2].contains("TOOL_RESULT read_file"));
}

#[tokio::test]
async fn loop_turns_excessive_duplicate_read_only_calls_into_tool_error() {
    let dir = tempdir().unwrap();
    std::fs::write(dir.path().join("a.txt"), "x\n").unwrap();
    let model = ScriptedModel::new(vec![
        r#"{"type":"tool_calls","calls":[{"tool":"read_file","args":{"path":"a.txt"}}]}"#
            .to_string(),
        r#"{"type":"tool_calls","calls":[{"tool":"read_file","args":{"path":"a.txt"}}]}"#
            .to_string(),
        r#"{"type":"tool_calls","calls":[{"tool":"read_file","args":{"path":"a.txt"}}]}"#
            .to_string(),
        r#"{"type":"tool_calls","calls":[{"tool":"read_file","args":{"path":"a.txt"}}]}"#
            .to_string(),
        r#"{"type":"final","content":"done after duplicate limit"}"#.to_string(),
    ]);
    let driver = LoopDriver::new(LoopConfig::default());

    let out = driver
        .run(&model, dir.path(), "inspect file", Vec::<ModelTurn>::new())
        .await
        .unwrap();

    assert_eq!(out.final_text, "done after duplicate limit");
    let prompts = model.prompts();
    assert!(prompts[4].contains("duplicate_reuse_limit"));
}

#[tokio::test]
async fn loop_turns_duplicate_empty_glob_into_tool_error_instead_of_reuse() {
    let dir = tempdir().unwrap();
    let model = ScriptedModel::new(vec![
        r#"{"type":"tool_calls","calls":[{"tool":"glob","args":{"pattern":"missing/*"}}]}"#
            .to_string(),
        r#"{"type":"tool_calls","calls":[{"tool":"glob","args":{"pattern":"missing/*"}}]}"#
            .to_string(),
        r#"{"type":"final","content":"changed strategy"}"#.to_string(),
    ]);
    let driver = LoopDriver::new(LoopConfig::default());

    let out = driver
        .run(
            &model,
            dir.path(),
            "inspect missing dir",
            Vec::<ModelTurn>::new(),
        )
        .await
        .unwrap();

    assert_eq!(out.final_text, "changed strategy");
    let prompts = model.prompts();
    assert!(prompts[2].contains("duplicate_empty_result"));
}

#[tokio::test]
async fn loop_blocks_repeated_pre_write_directory_inspection() {
    let dir = tempdir().unwrap();
    let model = ScriptedModel::new(vec![
        r#"{"type":"tool_calls","calls":[{"tool":"mkdir","args":{"path":"out"}}]}"#.to_string(),
        r#"{"type":"tool_calls","calls":[{"tool":"list_dir","args":{"path":"out"}}]}"#
            .to_string(),
        r#"{"type":"tool_calls","calls":[{"tool":"stat_path","args":{"path":"out"}}]}"#
            .to_string(),
        r#"{"type":"tool_calls","calls":[{"tool":"write_file","args":{"path":"out/index.html","content":"<html>ok</html>"}}]}"#
            .to_string(),
        r#"{"type":"final","content":"created output"}"#.to_string(),
    ]);
    let driver = LoopDriver::new(LoopConfig::default());

    let out = driver
        .run(
            &model,
            dir.path(),
            "Create browser output in out",
            Vec::<ModelTurn>::new(),
        )
        .await
        .unwrap();

    assert_eq!(out.final_text, "created output");
    let prompts = model.prompts();
    assert!(prompts[3].contains("stalled_pre_write_inspection"));
}

#[tokio::test]
async fn loop_rejects_path_mismatch_against_requested_output_root() {
    let dir = tempdir().unwrap();
    let model = ScriptedModel::new(vec![
        r#"{"type":"tool_calls","calls":[{"tool":"stat_path","args":{"path":"./sandbox11"}}]}"#
            .to_string(),
        r#"{"type":"tool_calls","calls":[{"tool":"mkdir","args":{"path":"./sandbox/test31_011"}}]}"#
            .to_string(),
        r#"{"type":"tool_calls","calls":[{"tool":"write_file","args":{"path":"./sandbox/test31_011/index.html","content":"<html>ok</html>"}}]}"#
            .to_string(),
        r#"{"type":"final","content":"created output"}"#.to_string(),
    ]);
    let driver = LoopDriver::new(LoopConfig::default());

    let out = driver
        .run(
            &model,
            dir.path(),
            "ブラウザから直接実行可能なページを作成し、./sandbox/test31_011に出力してください",
            Vec::<ModelTurn>::new(),
        )
        .await
        .unwrap();

    assert_eq!(out.final_text, "created output");
    let prompts = model.prompts();
    assert!(prompts[1].contains("path_mismatch"));
    assert!(std::fs::read_to_string(dir.path().join("sandbox/test31_011/index.html")).is_ok());
}

#[tokio::test]
async fn observer_receives_tool_error_events() {
    let dir = tempdir().unwrap();
    let model = ScriptedModel::new(vec![
        r#"{"type":"tool_calls","calls":[{"tool":"stat_path","args":{"path":"./sandbox11"}}]}"#
            .to_string(),
        r#"{"type":"final","content":"done"}"#.to_string(),
    ]);
    let driver = LoopDriver::new(LoopConfig::default());
    let mut events = Vec::new();

    let _ = driver
        .run_with_observer(
            &model,
            dir.path(),
            "ブラウザから直接実行可能なページを作成し、./sandbox/test31_011に出力してください",
            Vec::<ModelTurn>::new(),
            |event| events.push(event),
        )
        .await;

    assert!(events.iter().any(|event| matches!(
        event,
        LoopEvent::ToolErrorRecorded {
            tool,
            error_kind,
            ..
        } if tool == "stat_path" && error_kind == "path_mismatch"
    )));
}

#[tokio::test]
async fn create_task_prompt_includes_expected_root_and_phase() {
    let dir = tempdir().unwrap();
    let model = ScriptedModel::new(vec![
        r#"{"type":"tool_calls","calls":[{"tool":"mkdir","args":{"path":"./sandbox/test31_011"}}]}"#
            .to_string(),
        r#"{"type":"final","content":"done"}"#.to_string(),
    ]);
    let driver = LoopDriver::new(LoopConfig::default());

    let _ = driver
        .run(
            &model,
            dir.path(),
            "ブラウザから直接実行可能なページを作成し、./sandbox/test31_011に出力してください",
            Vec::<ModelTurn>::new(),
        )
        .await;

    let prompts = model.prompts();
    assert!(prompts[0].contains("EXPECTED_OUTPUT_ROOT"));
    assert!(prompts[0].contains("./sandbox/test31_011"));
    assert!(prompts[0].contains("CREATE_PHASE"));
    assert!(prompts[0].contains("prepare"));
    assert!(prompts[1].contains("write"));
}

#[tokio::test]
async fn loop_fail_closed_on_invalid_tool_schema() {
    let dir = tempdir().unwrap();
    let model = ScriptedModel::new(vec![
        r#"{"type":"tool_calls","calls":[{"tool":"read_file","args":{"unknown":"x"}}]}"#
            .to_string(),
        r#"{"type":"tool_calls","calls":[{"tool":"read_file","args":{"unknown":"x"}}]}"#
            .to_string(),
    ]);
    let driver = LoopDriver::new(LoopConfig::default());

    let err = driver
        .run(&model, dir.path(), "bad schema", Vec::<ModelTurn>::new())
        .await
        .unwrap_err();

    assert!(matches!(err, LoopError::InvalidToolCall(_)));
}

#[tokio::test]
async fn loop_reprompts_after_schema_error_and_recovers() {
    let dir = tempdir().unwrap();
    std::fs::write(dir.path().join("README.md"), "hello\n").unwrap();
    let model = ScriptedModel::new(vec![
        r#"{"type":"tool_calls","calls":[{"tool":"glob","args":{"path":"./sandbox/test31_001"}}]}"#
            .to_string(),
        r#"{"type":"tool_calls","calls":[{"tool":"read_file","args":{"path":"README.md"}}]}"#
            .to_string(),
        r#"{"type":"final","content":"recovered after tool error"}"#.to_string(),
    ]);
    let driver = LoopDriver::new(LoopConfig::default());

    let out = driver
        .run(
            &model,
            dir.path(),
            "recover from bad glob call",
            Vec::<ModelTurn>::new(),
        )
        .await
        .unwrap();

    assert_eq!(out.final_text, "recovered after tool error");
    let prompts = model.prompts();
    assert_eq!(prompts.len(), 3);
    assert!(prompts[1].contains("TOOL_ERROR glob"));
    assert!(prompts[1].contains("expected `pattern`"));
}

#[tokio::test]
async fn loop_stops_after_repeated_schema_errors() {
    let dir = tempdir().unwrap();
    let model = ScriptedModel::new(vec![
        r#"{"type":"tool_calls","calls":[{"tool":"glob","args":{"path":"a"}}]}"#.to_string(),
        r#"{"type":"tool_calls","calls":[{"tool":"glob","args":{"path":"b"}}]}"#.to_string(),
    ]);
    let driver = LoopDriver::new(LoopConfig::default());

    let err = driver
        .run(&model, dir.path(), "keep failing", Vec::<ModelTurn>::new())
        .await
        .unwrap_err();

    assert!(matches!(err, LoopError::InvalidToolCall(_)));
}

#[tokio::test]
async fn loop_retries_after_invalid_json_and_recovers() {
    let dir = tempdir().unwrap();
    std::fs::write(dir.path().join("README.md"), "hello\n").unwrap();
    let model = ScriptedModel::new(vec![
        "{\"type\":\"tool_calls\",\"calls\":[{\"tool\":\"read_file\",\"args\":{\"path\":\"README.md\"}}".to_string(),
        r#"{"type":"tool_calls","calls":[{"tool":"read_file","args":{"path":"README.md"}}]}"#
            .to_string(),
        r#"{"type":"final","content":"recovered after invalid json"}"#.to_string(),
    ]);
    let driver = LoopDriver::new(LoopConfig::default());

    let out = driver
        .run(
            &model,
            dir.path(),
            "recover from invalid json",
            Vec::<ModelTurn>::new(),
        )
        .await
        .unwrap();

    assert_eq!(out.final_text, "recovered after invalid json");
    let prompts = model.prompts();
    assert_eq!(prompts.len(), 3);
    assert!(prompts[1].contains("MODEL_ERROR"));
    assert!(prompts[1].contains("invalid_json"));
}

#[tokio::test]
async fn loop_retries_after_tool_execution_error_and_recovers() {
    let dir = tempdir().unwrap();
    std::fs::create_dir_all(dir.path().join("nested")).unwrap();
    std::fs::write(dir.path().join("nested/file.txt"), "hello\n").unwrap();
    let model = ScriptedModel::new(vec![
        r#"{"type":"tool_calls","calls":[{"tool":"read_file","args":{"path":"nested"}}]}"#
            .to_string(),
        r#"{"type":"tool_calls","calls":[{"tool":"glob","args":{"pattern":"nested/*"}}]}"#
            .to_string(),
        r#"{"type":"final","content":"recovered after execution error"}"#.to_string(),
    ]);
    let driver = LoopDriver::new(LoopConfig::default());

    let out = driver
        .run(
            &model,
            dir.path(),
            "inspect nested directory",
            Vec::<ModelTurn>::new(),
        )
        .await
        .unwrap();

    assert_eq!(out.final_text, "recovered after execution error");
    let prompts = model.prompts();
    assert_eq!(prompts.len(), 3);
    assert!(prompts[1].contains("TOOL_ERROR read_file"));
    assert!(prompts[1].contains("execution_error"));
}

#[tokio::test]
async fn loop_stops_after_repeated_invalid_json() {
    let dir = tempdir().unwrap();
    let model = ScriptedModel::new(vec![
        "{\"type\":\"tool_calls\",\"calls\":[".to_string(),
        "{\"type\":\"tool_calls\",\"calls\":[".to_string(),
    ]);
    let driver = LoopDriver::new(LoopConfig::default());

    let err = driver
        .run(
            &model,
            dir.path(),
            "keep breaking json",
            Vec::<ModelTurn>::new(),
        )
        .await
        .unwrap_err();

    assert!(matches!(err, LoopError::InvalidToolCall(_)));
}

#[tokio::test]
async fn loop_carries_prior_context_into_prompt() {
    let dir = tempdir().unwrap();
    let model = ScriptedModel::new(vec![r#"{"type":"final","content":"ok"}"#.to_string()]);
    let driver = LoopDriver::new(LoopConfig::default());
    let prior = vec![ModelTurn::ToolResult {
        tool: "search".to_string(),
        output: "found branch notes".to_string(),
    }];

    let _ = driver
        .run(&model, dir.path(), "continue", prior)
        .await
        .unwrap();

    let prompts = model.prompts();
    assert!(prompts[0].contains("found branch notes"));
}

#[tokio::test]
async fn loop_allows_retry_after_different_validation_error() {
    let dir = tempdir().unwrap();
    std::fs::write(dir.path().join("README.md"), "hello\n").unwrap();
    let model = ScriptedModel::new(vec![
        r#"{"type":"tool_calls","calls":[{"tool":"read_file","args":{"create_if_not_exists":true}}]}"#
            .to_string(),
        r#"{"type":"tool_calls","calls":[{"tool":"exec","args":{"command":"cat > README.md"}}]}"#
            .to_string(),
        r#"{"type":"tool_calls","calls":[{"tool":"read_file","args":{"path":"README.md"}}]}"#
            .to_string(),
        r#"{"type":"final","content":"recovered after multiple validation errors"}"#.to_string(),
    ]);
    let driver = LoopDriver::new(LoopConfig::default());

    let out = driver
        .run(
            &model,
            dir.path(),
            "recover from different validation errors",
            Vec::<ModelTurn>::new(),
        )
        .await
        .unwrap();

    assert_eq!(out.final_text, "recovered after multiple validation errors");
    let prompts = model.prompts();
    assert_eq!(prompts.len(), 4);
    assert!(prompts[1].contains("TOOL_ERROR read_file"));
    assert!(prompts[2].contains("TOOL_ERROR exec"));
}

#[tokio::test]
async fn loop_stops_on_duplicate_write_tool_calls() {
    let dir = tempdir().unwrap();
    let model = ScriptedModel::new(vec![
        r#"{"type":"tool_calls","calls":[{"tool":"write_file","args":{"path":"a.txt","content":"x"}}]}"#
            .to_string(),
        r#"{"type":"tool_calls","calls":[{"tool":"write_file","args":{"path":"a.txt","content":"x"}}]}"#
            .to_string(),
    ]);
    let driver = LoopDriver::new(LoopConfig::default());

    let err = driver
        .run(&model, dir.path(), "write once", Vec::<ModelTurn>::new())
        .await
        .unwrap_err();

    assert!(matches!(err, LoopError::DuplicateToolCall(_)));
}

#[tokio::test]
async fn observer_receives_raw_preview_and_validated_tool_events() {
    let dir = tempdir().unwrap();
    std::fs::write(dir.path().join("README.md"), "hello\n").unwrap();
    std::fs::create_dir_all(dir.path().join("nested")).unwrap();
    let model = ScriptedModel::new(vec![
        r#"{"type":"tool_calls","calls":[{"tool":"read_file","args":{"path":"nested"}}]}"#
            .to_string(),
        r#"{"type":"tool_calls","calls":[{"tool":"read_file","args":{"path":"README.md"}}]}"#
            .to_string(),
        r#"{"type":"tool_calls","calls":[{"tool":"read_file","args":{"path":"README.md"}}]}"#
            .to_string(),
        r#"{"type":"final","content":"done"}"#.to_string(),
    ]);
    let driver = LoopDriver::new(LoopConfig::default());
    let events = Arc::new(Mutex::new(Vec::<LoopEvent>::new()));
    let sink = Arc::clone(&events);

    let out = driver
        .run_with_observer(
            &model,
            dir.path(),
            "inspect readme",
            Vec::<ModelTurn>::new(),
            move |event| sink.lock().unwrap().push(event),
        )
        .await
        .unwrap();

    assert_eq!(out.final_text, "done");
    let events = events.lock().unwrap();
    assert!(
        events
            .iter()
            .any(|event| matches!(event, LoopEvent::ModelResponsePreview { .. }))
    );
    assert!(
        events
            .iter()
            .any(|event| matches!(event, LoopEvent::ToolCallValidated { .. }))
    );
    assert!(
        events
            .iter()
            .any(|event| matches!(event, LoopEvent::ToolResultPreview { .. }))
    );
    assert!(
        events
            .iter()
            .any(|event| matches!(event, LoopEvent::ToolResultReused { .. }))
    );
    assert!(
        events
            .iter()
            .any(|event| matches!(event, LoopEvent::ToolExecutionRetry { .. }))
    );
    assert!(events.iter().any(|event| matches!(
        event,
        LoopEvent::ModelResponseReceived { elapsed_ms: _, .. }
    )));
    assert!(events.iter().any(|event| matches!(
        event,
        LoopEvent::ToolExecutionFinished { elapsed_ms: _, .. }
    )));
    assert!(
        events
            .iter()
            .any(|event| matches!(event, LoopEvent::StepFinished { elapsed_ms: _, .. }))
    );
}

#[test]
fn default_loop_config_allows_longer_generation_tasks() {
    let config = LoopConfig::default();

    assert_eq!(config.max_steps, 12);
    assert_eq!(config.max_cached_reuses_per_call, 2);
}
