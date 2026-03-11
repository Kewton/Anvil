use std::sync::{Arc, Mutex};

use anvil::agent::looping::{LoopConfig, LoopDriver, LoopError, ModelExchange, ModelTurn};
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
    ]);
    let driver = LoopDriver::new(LoopConfig::default());

    let err = driver
        .run(&model, dir.path(), "inspect repo", Vec::<ModelTurn>::new())
        .await
        .unwrap_err();

    assert!(matches!(err, LoopError::InvalidToolCall(_)));
}

#[tokio::test]
async fn loop_stops_on_duplicate_tool_calls() {
    let dir = tempdir().unwrap();
    std::fs::write(dir.path().join("a.txt"), "x\n").unwrap();
    let model = ScriptedModel::new(vec![
        r#"{"type":"tool_calls","calls":[{"tool":"read_file","args":{"path":"a.txt"}}]}"#
            .to_string(),
        r#"{"type":"tool_calls","calls":[{"tool":"read_file","args":{"path":"a.txt"}}]}"#
            .to_string(),
    ]);
    let driver = LoopDriver::new(LoopConfig::default());

    let err = driver
        .run(&model, dir.path(), "loop forever", Vec::<ModelTurn>::new())
        .await
        .unwrap_err();

    assert!(matches!(err, LoopError::DuplicateToolCall(_)));
}

#[tokio::test]
async fn loop_fail_closed_on_invalid_tool_schema() {
    let dir = tempdir().unwrap();
    let model = ScriptedModel::new(vec![
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
