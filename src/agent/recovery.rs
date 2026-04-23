use crate::modes::plan_act::ExecutionMode;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ActionExpectation {
    None,
    ToolAction,
    RepoChange,
    PlanProgress,
}

pub fn classify_action_expectation(prompt: &str, mode: ExecutionMode) -> ActionExpectation {
    if mode == ExecutionMode::Plan {
        return ActionExpectation::PlanProgress;
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

pub fn plan_no_tool_recovery_note(attempt: usize) -> String {
    format!(
        "You are still in Plan mode and the plan has not advanced. Do not describe intent only. On the next turn, call a tool immediately and make one small Write or Edit to the active plan file. plan_no_tool_attempt={attempt}"
    )
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

pub fn tool_call_format_recovery_note(error: &str, attempt: usize) -> String {
    format!(
        "Previous tool call failed to parse: {error}. On the next turn, emit exactly one valid <anvil_tool_call>{{\"name\":\"Tool\",\"arguments\":{{...}}}}</anvil_tool_call> block with complete JSON. Keep the tool call small. Start with one small, self-contained change only, and prefer short Write or Edit actions over large full-file outputs. tool_call_format_attempt={attempt}"
    )
}

pub fn plan_progress_recovery_note(
    next_sections: &[&str],
    missing_sections: &[&str],
    attempt: usize,
) -> String {
    let next = if next_sections.is_empty() {
        "the remaining missing sections".to_string()
    } else {
        next_sections.join(", ")
    };
    let missing = if missing_sections.is_empty() {
        "-".to_string()
    } else {
        missing_sections.join(", ")
    };
    format!(
        "The plan is still incomplete. Do not keep exploring. On the next turn, make exactly one small Write or Edit to the plan file and fill only these next sections: {next}. Missing sections now: {missing}. Avoid broad Read or Glob unless a specific missing section requires it. plan_progress_attempt={attempt}"
    )
}

pub fn install_loop_recovery_note() -> String {
    "Recent turns repeated setup, rescaffolding, or dependency installation commands without finishing the implementation. Stop reinstalling packages or recreating the project. Inspect the project files that matter, then use Write or Edit to make concrete code changes before any further setup.".to_string()
}

pub fn repeated_bash_error(command: &str) -> String {
    format!(
        "Error: repeated or risky Bash command blocked to prevent a tool loop: {command}. Do not repeat setup, rescaffolding, workspace resets, or install steps. Inspect files and continue with Read, Write, or Edit instead."
    )
}

pub fn broad_restart_discovery_error(tool_name: &str) -> String {
    format!(
        "Error: broad {tool_name} discovery is blocked during actor restart. Continue from the files already changed and use Read, Write, or Edit on the current implementation area instead."
    )
}

pub fn is_dependency_install_command(command: &str) -> bool {
    let normalized = command.to_ascii_lowercase();
    normalized.contains("npm install")
        || normalized.contains("npm i ")
        || normalized.contains("pnpm add")
        || normalized.contains("pnpm install")
        || normalized.contains("yarn add")
        || normalized.contains("yarn install")
        || normalized.contains("pip install")
        || normalized.contains("pip3 install")
        || normalized.contains("cargo add")
        || normalized.contains("cargo install")
        || normalized.contains("bundle add")
        || normalized.contains("composer require")
}

pub fn is_scaffold_command(command: &str) -> bool {
    let normalized = command.to_ascii_lowercase();
    normalized.contains("create-next-app")
        || normalized.contains("npm create ")
        || normalized.contains("pnpm create ")
        || normalized.contains("yarn create ")
        || normalized.contains("cargo new ")
        || normalized.contains("cargo init ")
}

pub fn is_workspace_reset_command(command: &str) -> bool {
    let normalized = command.to_ascii_lowercase();
    normalized.contains("rm -rf .anvil")
        || normalized.contains("rm -rf .next")
        || normalized.contains("rm -rf node_modules")
        || normalized.contains("rm -rf package-lock.json")
}

pub fn should_block_bash_command(
    command: &str,
    recent_bash_commands: &[String],
    install_commands_seen: usize,
) -> bool {
    let normalized = command.trim().to_ascii_lowercase();
    if is_workspace_reset_command(&normalized) {
        return true;
    }
    if recent_bash_commands
        .iter()
        .any(|previous| previous.trim().eq_ignore_ascii_case(command.trim()))
    {
        return true;
    }
    if is_scaffold_command(&normalized)
        && recent_bash_commands
            .iter()
            .any(|previous| is_scaffold_command(previous))
    {
        return true;
    }
    is_dependency_install_command(&normalized) && install_commands_seen >= 2
}

pub fn tool_call_counts_as_repo_edit(name: &str) -> bool {
    matches!(name, "Write" | "Edit")
}

pub fn should_block_restart_discovery(tool_name: &str, progress_exists: bool) -> bool {
    progress_exists && matches!(tool_name, "Glob" | "Bash")
}
