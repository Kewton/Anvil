use std::path::Path;

use crate::tools::registry::ToolSpec;

pub fn build_system_prompt(work_root: &Path, tools: &[ToolSpec]) -> String {
    let tool_catalog = render_tool_catalog(tools);
    format!(
        "You are Anvil, a local-first coding agent running against a local Ollama model.\n\
\n\
Rules:\n\
1. Prefer tools when repository facts or file contents are needed.\n\
2. Reply in the user's language.\n\
3. Never use sudo or destructive shell commands.\n\
4. Make small coherent changes and verify when practical.\n\
5. Use repository-relative paths under the project root.\n\
6. If a tool fails, try a different local approach or explain the blocker.\n\
7. Do not invent files, outputs, tests, or command results you have not observed.\n\
8. In Plan mode, only inspect files and produce a plan; do not edit.\n\
\n\
Project root: {}\n\
\n\
Tools:\n\
{}\n\
\n\
When native tool calls are unavailable, emit exactly one XML tool call like:\n\
<anvil_tool_call>{{\"name\":\"Read\",\"arguments\":{{\"path\":\"README.md\"}}}}</anvil_tool_call>",
        work_root.display(),
        tool_catalog,
    )
}

fn render_tool_catalog(tools: &[ToolSpec]) -> String {
    tools
        .iter()
        .map(|tool| format!("- {}: {}", tool.function.name, tool.function.description))
        .collect::<Vec<_>>()
        .join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tools::registry::ToolRegistry;

    #[test]
    fn fixed_prompt_snapshot_has_at_most_ten_rules() {
        let registry = ToolRegistry::default();
        let prompt = build_system_prompt(Path::new("/workspace/project"), registry.specs());

        let expected = "You are Anvil, a local-first coding agent running against a local Ollama model.\n\
\n\
Rules:\n\
1. Prefer tools when repository facts or file contents are needed.\n\
2. Reply in the user's language.\n\
3. Never use sudo or destructive shell commands.\n\
4. Make small coherent changes and verify when practical.\n\
5. Use repository-relative paths under the project root.\n\
6. If a tool fails, try a different local approach or explain the blocker.\n\
7. Do not invent files, outputs, tests, or command results you have not observed.\n\
8. In Plan mode, only inspect files and produce a plan; do not edit.\n\
\n\
Project root: /workspace/project\n\
\n\
Tools:\n\
- Bash: Run a shell command in the project directory. Runtime classifies commands as read-only, build-test, or general, and offline mode blocks networked or general shell commands.\n\
- Read: Read a text file or list a directory. Use repository-relative paths.\n\
- Write: Create or overwrite a file. Use repository-relative paths.\n\
- Edit: Replace exact text in an existing file. Use repository-relative paths.\n\
- Glob: Find files by glob pattern.\n\
- Grep: Search repository text.\n\
\n\
When native tool calls are unavailable, emit exactly one XML tool call like:\n\
<anvil_tool_call>{\"name\":\"Read\",\"arguments\":{\"path\":\"README.md\"}}</anvil_tool_call>";

        assert_eq!(prompt, expected);
        assert_eq!(
            prompt
                .lines()
                .filter(|line| { line.chars().next().is_some_and(|ch| ch.is_ascii_digit()) })
                .count(),
            8
        );
    }
}
