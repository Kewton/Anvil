use crate::modes::plan_act::ExecutionMode;

pub fn user_prompt_requires_action(prompt: &str, mode: ExecutionMode) -> bool {
    if mode == ExecutionMode::Plan {
        return false;
    }

    let normalized = prompt.to_ascii_lowercase();
    let english_keywords = [
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
        "test",
        "run",
    ];
    if english_keywords
        .iter()
        .any(|keyword| normalized.contains(keyword))
    {
        return true;
    }

    let japanese_keywords = [
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
        "動かして",
        "テスト",
        "確認",
    ];
    japanese_keywords
        .iter()
        .any(|keyword| prompt.contains(keyword))
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
