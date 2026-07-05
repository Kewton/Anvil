use std::path::Path;

use crate::tools::registry::ToolSpec;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PromptToolMode {
    Native,
    XmlFallback,
}

pub fn build_system_prompt(
    work_root: &Path,
    tools: &[ToolSpec],
    tool_mode: PromptToolMode,
) -> String {
    let tool_catalog = render_tool_catalog(tools);
    let tool_mode_instruction = match tool_mode {
        PromptToolMode::Native => {
            "Use the runtime-provided tool call channel for tools. Do not place tool calls in ordinary assistant text."
        }
        PromptToolMode::XmlFallback => {
            "Native tool calls are unavailable for this session. To use a tool, emit exactly one XML tool call like:\n\
<anvil_tool_call>{\"name\":\"Read\",\"arguments\":{\"path\":\"README.md\"}}</anvil_tool_call>"
        }
    };
    format!(
        "You are Anvil, a local-first coding agent running against a configured LLM model.\n\
\n\
Rules:\n\
1. Prefer tools when repository facts or file contents are needed.\n\
2. Reply in the user's language.\n\
3. Never use sudo or destructive shell commands.\n\
4. If you say you will create, edit, read, or verify something, call the tool in that same response.\n\
5. Final answers must describe completed work, not planned next steps. Do not end with phrases like \"I will create\", \"Let me verify\", or \"Now I'll edit\".\n\
6. Use repository-relative paths under the project root. Paths must be workspace-relative; absolute paths are auto-normalized when safe but relative is the required form.\n\
7. If a tool fails, try a different local approach or explain the blocker.\n\
8. Do not invent files, outputs, tests, or command results you have not observed.\n\
9. In Plan mode, only inspect files and produce a plan; do not edit.\n\
\n\
Project root: {}\n\
\n\
Tools:\n\
{}\n\
\n\
{}",
        work_root.display(),
        tool_catalog,
        tool_mode_instruction,
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
    fn fixed_native_prompt_snapshot_has_at_most_ten_rules() {
        let registry = ToolRegistry::default();
        let prompt = build_system_prompt(
            Path::new("/workspace/project"),
            registry.specs(),
            PromptToolMode::Native,
        );

        let expected = "You are Anvil, a local-first coding agent running against a configured LLM model.\n\
\n\
Rules:\n\
1. Prefer tools when repository facts or file contents are needed.\n\
2. Reply in the user's language.\n\
3. Never use sudo or destructive shell commands.\n\
4. If you say you will create, edit, read, or verify something, call the tool in that same response.\n\
5. Final answers must describe completed work, not planned next steps. Do not end with phrases like \"I will create\", \"Let me verify\", or \"Now I'll edit\".\n\
6. Use repository-relative paths under the project root. Paths must be workspace-relative; absolute paths are auto-normalized when safe but relative is the required form.\n\
7. If a tool fails, try a different local approach or explain the blocker.\n\
8. Do not invent files, outputs, tests, or command results you have not observed.\n\
9. In Plan mode, only inspect files and produce a plan; do not edit.\n\
\n\
Project root: /workspace/project\n\
\n\
Tools:\n\
- Bash: Run read-only inspection, build/test, or local script validation commands in the project directory. Do not use Bash to create files or directories; use Write for file creation because Write creates parent directories automatically. Offline mode blocks networked, mutating, or general shell commands.\n\
- Read: Read a text file or list a directory. Use repository-relative paths. Paths must be workspace-relative; absolute paths are auto-normalized when safe but relative is the required form.\n\
- Write: Create or overwrite a file. Parent directories are created automatically. Use repository-relative paths. Paths must be workspace-relative; absolute paths are auto-normalized when safe but relative is the required form.\n\
- Edit: Replace exact text in an existing file. Use repository-relative paths. Paths must be workspace-relative; absolute paths are auto-normalized when safe but relative is the required form.\n\
- Glob: Find files by glob pattern.\n\
- Grep: Search repository text.\n\
\n\
Use the runtime-provided tool call channel for tools. Do not place tool calls in ordinary assistant text.";

        assert_eq!(prompt, expected);
        assert!(!prompt.contains("<anvil_tool_call>"));
        assert_eq!(
            prompt
                .lines()
                .filter(|line| { line.chars().next().is_some_and(|ch| ch.is_ascii_digit()) })
                .count(),
            9
        );
    }

    #[test]
    fn fixed_xml_fallback_prompt_snapshot_has_xml_example() {
        let registry = ToolRegistry::default();
        let prompt = build_system_prompt(
            Path::new("/workspace/project"),
            registry.specs(),
            PromptToolMode::XmlFallback,
        );

        let expected = "You are Anvil, a local-first coding agent running against a configured LLM model.\n\
\n\
Rules:\n\
1. Prefer tools when repository facts or file contents are needed.\n\
2. Reply in the user's language.\n\
3. Never use sudo or destructive shell commands.\n\
4. If you say you will create, edit, read, or verify something, call the tool in that same response.\n\
5. Final answers must describe completed work, not planned next steps. Do not end with phrases like \"I will create\", \"Let me verify\", or \"Now I'll edit\".\n\
6. Use repository-relative paths under the project root. Paths must be workspace-relative; absolute paths are auto-normalized when safe but relative is the required form.\n\
7. If a tool fails, try a different local approach or explain the blocker.\n\
8. Do not invent files, outputs, tests, or command results you have not observed.\n\
9. In Plan mode, only inspect files and produce a plan; do not edit.\n\
\n\
Project root: /workspace/project\n\
\n\
Tools:\n\
- Bash: Run read-only inspection, build/test, or local script validation commands in the project directory. Do not use Bash to create files or directories; use Write for file creation because Write creates parent directories automatically. Offline mode blocks networked, mutating, or general shell commands.\n\
- Read: Read a text file or list a directory. Use repository-relative paths. Paths must be workspace-relative; absolute paths are auto-normalized when safe but relative is the required form.\n\
- Write: Create or overwrite a file. Parent directories are created automatically. Use repository-relative paths. Paths must be workspace-relative; absolute paths are auto-normalized when safe but relative is the required form.\n\
- Edit: Replace exact text in an existing file. Use repository-relative paths. Paths must be workspace-relative; absolute paths are auto-normalized when safe but relative is the required form.\n\
- Glob: Find files by glob pattern.\n\
- Grep: Search repository text.\n\
\n\
Native tool calls are unavailable for this session. To use a tool, emit exactly one XML tool call like:\n\
<anvil_tool_call>{\"name\":\"Read\",\"arguments\":{\"path\":\"README.md\"}}</anvil_tool_call>";

        assert_eq!(prompt, expected);
        assert!(prompt.contains("<anvil_tool_call>"));
        assert_eq!(
            prompt
                .lines()
                .filter(|line| { line.chars().next().is_some_and(|ch| ch.is_ascii_digit()) })
                .count(),
            9
        );
    }
}
