use crate::modes::plan_act::ExecutionMode;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ActionExpectation {
    None,
    ToolAction,
    RepoChange,
}

pub fn classify_action_expectation(prompt: &str, mode: ExecutionMode) -> ActionExpectation {
    if mode == ExecutionMode::Plan {
        return ActionExpectation::None;
    }

    let normalized = prompt.to_ascii_lowercase();
    let repo_change_english_keywords = [
        "write",
        "edit",
        "modify",
        "change",
        "update",
        "create",
        "fix",
        "implement",
        "build",
        "add",
        "remove",
        "rename",
        "refactor",
        "develop",
        "scaffold",
    ];
    if repo_change_english_keywords
        .iter()
        .any(|keyword| normalized.contains(keyword))
    {
        return ActionExpectation::RepoChange;
    }

    let repo_change_japanese_keywords = [
        "作って",
        "作成",
        "書いて",
        "編集",
        "修正",
        "変更",
        "直して",
        "実装",
        "追加",
        "削除",
        "開発",
        "作り直",
        "組み直",
        "改修",
    ];
    if repo_change_japanese_keywords
        .iter()
        .any(|keyword| prompt.contains(keyword))
    {
        return ActionExpectation::RepoChange;
    }

    let tool_action_english_keywords = [
        "test", "run", "check", "verify", "install", "start", "launch",
    ];
    if tool_action_english_keywords
        .iter()
        .any(|keyword| normalized.contains(keyword))
    {
        return ActionExpectation::ToolAction;
    }

    let tool_action_japanese_keywords =
        ["動かして", "起動", "テスト", "確認", "実行", "インストール"];
    if tool_action_japanese_keywords
        .iter()
        .any(|keyword| prompt.contains(keyword))
    {
        return ActionExpectation::ToolAction;
    }

    ActionExpectation::None
}

pub fn user_prompt_requires_action(prompt: &str, mode: ExecutionMode) -> bool {
    classify_action_expectation(prompt, mode) != ActionExpectation::None
}

pub fn empty_response_recovery_note(attempt: usize, requires_action: bool) -> String {
    if requires_action {
        format!(
            "The previous response was empty. The user asked for an action. On the next turn, either call an appropriate tool immediately or provide a concrete final answer only if the requested repository change is already complete. Empty replies are not allowed. attempt={attempt}"
        )
    } else {
        format!(
            "The previous response was empty. On the next turn, answer directly or call a tool if external state is required. Empty replies are not allowed. attempt={attempt}"
        )
    }
}

pub fn no_tool_recovery_note(attempt: usize) -> String {
    format!(
        "The user asked for a concrete action in the repository. Do not only describe intent. On the next turn, call a tool immediately unless the work is already complete. no_tool_attempt={attempt}"
    )
}

pub fn repo_change_recovery_note(attempt: usize) -> String {
    format!(
        "The user asked for an actual repository change. Setup, scaffolding, or explanation alone is not enough. Do not describe the next step. On the next turn, inspect the target implementation files and then call Write or Edit to change them. Only give a final answer after the requested code change is already present. repo_change_attempt={attempt}"
    )
}

pub fn tool_call_counts_as_repo_edit(name: &str) -> bool {
    matches!(name, "Write" | "Edit")
}
