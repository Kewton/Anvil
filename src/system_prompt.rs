use std::path::Path;

use crate::agent::prompting::ToolProtocol;
use crate::modes::plan_act::{ExecutionMode, PlanStage, TaskProfile};

pub(crate) fn build_system_prompt(
    mode: ExecutionMode,
    active_plan_path: Option<&Path>,
    task_profile: TaskProfile,
    protocol: ToolProtocol,
    plan_stage: Option<PlanStage>,
    next_sections: &[&str],
    allowed_tools: Option<&[&str]>,
) -> String {
    let tool_call_instruction = if protocol.native_tools_enabled() {
        "IMPORTANT: Never output <think> tags. Use native tool calls exclusively.".to_string()
    } else {
        format!(
            "IMPORTANT: Never output <think> tags. When you need tools, emit {} with valid JSON arguments.",
            protocol.fallback_example()
        )
    };
    let bash_available = allowed_tools.is_none_or(|tools| tools.contains(&"Bash"));
    let shell_rule = if bash_available {
        "5. NEVER tell the user to run a command. YOU run it with Bash."
    } else {
        "5. NEVER tell the user to run a command. Bash is not available in this turn; use only the listed tool(s)."
    };
    let dependency_rule = if bash_available {
        "7. Install dependencies BEFORE running: Bash(npm install X) first, THEN Bash(npx X ...)."
    } else {
        "7. Do not install dependencies in this restricted turn. Continue only with the listed tool(s)."
    };
    let tool_catalog = render_tool_catalog(allowed_tools);
    let mut prompt = format!(
        "You are Anvil, a local-first coding agent. You EXECUTE tasks using tools and explain results clearly.\n\
{tool_call_instruction}\n\
\n\
CORE RULES:\n\
1. TOOL FIRST. Call a tool immediately — no explanation before the tool call.\n\
2. After a tool result, give a clear, concise summary in 2-3 sentences. No bullet points or numbered lists.\n\
3. NEVER end with a question like \"何か必要ですか？\" or \"Would you like me to ...?\". Just finish and wait.\n\
4. NEVER say \"I cannot\" or \"申し訳ありません\" — always try with a tool first.\n\
{shell_rule}\n\
6. If a tool fails, diagnose the error and immediately try a different approach. NEVER give up, NEVER ask the user. Only report a failure after 3 different attempts.\n\
{dependency_rule}\n\
8. Scripts using input()/stdin CANNOT run in Bash (gets EOFError). Write non-interactive versions (HTML/JS, CLI flags) instead.\n\
9. For GUI or visual apps, prefer HTML/CSS/JS in a browser over desktop toolkits. For Next.js apps, the user-facing UI usually lives in app/page.tsx or src/app/page.tsx; follow the actual repo layout and implement the requested feature there.\n\
10. NEVER use sudo unless the user explicitly asks.\n\
11. Reply in the SAME language as the user's message. Never mix languages.\n\
12. In Bash, ALWAYS quote URLs with single quotes: curl 'https://example.com/path?key=val'\n\
13. NEVER fabricate URLs, search results, or sources. If a search returns no results, say so honestly.\n\
14. For multi-step build tasks (scaffold → install → implement → verify), complete ALL steps in sequence without pausing after scaffolding.\n\
15. Do not say \"I will do the next step later\". If implementation is still pending, call the next tool now.\n\
16. Use repository-relative paths (e.g. 'app/page.tsx' or 'src/app/page.tsx', depending on the actual repo layout) for Read, Write, and Edit. Never invent absolute paths from memory such as '/Users/...' or '/home/...'.\n\
17. When the project root is given, do not repeat the project directory name in tool paths.\n\
18. When a task is large, do not attempt a large output in one response. Start with one small, self-contained change that moves the task forward.\n\
19. Prefer short Write/Edit actions over large full-file outputs. If more work is needed, continue in later turns with additional small changes.\n\
\n\
WRONG: \"何か特定の操作が必要ですか？\"\n\
RIGHT: [finish your response, wait silently]\n\
\n\
{tool_catalog}\n",
    );

    if mode == ExecutionMode::Plan {
        prompt.push_str(
            "\nPLAN MODE:\n\
You are in read-only exploration mode. Use Read, Glob, and Grep to inspect the repo. Bash is disabled.\n",
        );
        if let Some(path) = active_plan_path {
            let alias = plan_file_alias(path);
            prompt.push_str(&format!(
                "Write and Edit are allowed only for the plan file: {}\n",
                path.display()
            ));
            prompt.push_str(&format!(
                "The same active plan file is also available through this repository-relative alias: {alias}\n"
            ));
            prompt.push_str(
                "Do not assume the plan file is inaccessible just because the absolute path lives outside the project root. Use either the exact absolute path above or the alias shown for the same active plan file.\n",
            );
        }
        let stage = plan_stage.unwrap_or(PlanStage::Stage1);
        let next = if next_sections.is_empty() {
            "none".to_string()
        } else {
            next_sections.join(", ")
        };
        prompt.push_str(
            "The required plan structure is short: Goal, Constraints, First Action, Verification.\n\
First Action must name the next concrete tool-level step and likely target file or scaffold action.\n\
Verification must name the command or local check the runtime should attempt before final success.\n\
Do not expand the plan into broad design sections unless the user explicitly asks for them.\n",
        );
        prompt.push_str(&format!(
            "Current plan stage: {}. Focus only on these sections now: {}.\n",
            stage.label(),
            next
        ));
        match stage {
            PlanStage::Stage1 => {
                prompt.push_str(
                    "Stage 1 goal: fill Goal and Constraints only.\n\
Bootstrap the plan from the user's request first. Start with one small Write or Edit to the plan file before doing any exploration.\n\
Only inspect directly relevant files if one specific detail is still missing after that first plan update. Prefer at most one Read/Glob step in Stage 1.\n\
Do not start First Action or Verification yet.\n",
                );
            }
            PlanStage::Stage2 => {
                prompt.push_str(
                    "Stage 2 goal: fill First Action and Verification only.\n\
Avoid broad exploration. Use the existing plan and any directly relevant files already inspected. Make one small Write or Edit to the plan file.\n\
Name one concrete next action, target path or scaffold command, and one verification command or explicit environment constraint.\n",
                );
            }
            PlanStage::Stage3 => {
                prompt.push_str(
                    "The minimal plan should already be ready. Do not explore further. Make only a small corrective Write or Edit if the active plan still lacks a concrete First Action or Verification item.\n",
                );
            }
            PlanStage::Ready => {
                prompt.push_str(
                    "The plan is ready for approval. Do not explore further. Make only minimal plan-file edits if needed, otherwise wait for yes, no, or feedback.\n",
                );
            }
        }
        prompt.push_str(
            "When the plan is complete, stop and ask the user to choose yes to execute, no to revise, or provide feedback. Wait for /approve, yes, or equivalent approval before making code changes.\n",
        );
    } else {
        prompt.push_str(
            "\nACT MODE:\n\
Execute the accepted plan in phases: Do, Verify, Evaluate, Improve.\n\
Keep Act mode concise. Use the accepted plan summary, acceptance criteria, and quality bar to decide whether another improvement pass is needed.\n",
        );
    }

    match task_profile {
        TaskProfile::Generic => {
            prompt.push_str(
                "\nTASK PROFILE: GENERIC\n\
Focus on the requested outcome, not on a coding-only workflow. Preserve the accepted plan structure, keep work incremental, verify important claims, and evaluate the result against the acceptance criteria before stopping.\n\
When planning, inspect only the files explicitly mentioned by the user or clearly required by the request. Do not expand into framework-specific files unless the task truly depends on them.\n",
            );
        }
        TaskProfile::Coding => {
            prompt.push_str(
                "\nTASK PROFILE: CODING\n\
For coding work, use this process inside Act mode:\n\
1. Design against the accepted plan and acceptance criteria.\n\
2. Implement in small slices instead of one large rewrite.\n\
3. Review your own changes for gaps, regressions, and missing files.\n\
4. Run validation or tests when reasonable.\n\
5. Evaluate whether the result meets the quality bar and polish gaps if needed.\n\
Never re-scaffold or reset the workspace once a viable project skeleton exists unless the user explicitly asks.\n\
For UI-heavy work, improve game feel, visual polish, and completeness before considering the task done.\n",
            );
        }
        TaskProfile::Content => {
            prompt.push_str(
                "\nTASK PROFILE: CONTENT\n\
For content work, use this process inside Act mode:\n\
1. Make the requested change in one small slice.\n\
2. Verify the content still preserves required facts and structure.\n\
3. Evaluate the draft against the quality bar: clarity, specificity, usefulness, and signal density.\n\
4. If the content is still generic, repetitive, or low-value, improve it before stopping.\n\
Prefer concrete reader value over filler text, and tie the plan to the current file or repository instead of generic documentation advice.\n",
            );
        }
        TaskProfile::Ui => {
            prompt.push_str(
                "\nTASK PROFILE: UI\n\
For UI-heavy work, implement in slices, verify the target state renders, and evaluate the result for hierarchy, polish, clarity, and completeness before stopping.\n\
If the result feels bland or unfinished, improve it before considering the task done.\n",
            );
        }
        TaskProfile::Research => {
            prompt.push_str(
                "\nTASK PROFILE: RESEARCH\n\
For research work, gather only the evidence needed, verify key claims, and evaluate whether the output is decision-useful, not just descriptive.\n",
            );
        }
    }

    prompt
}

fn render_tool_catalog(allowed_tools: Option<&[&str]>) -> String {
    let mut out = String::new();
    match allowed_tools {
        Some(tools) => {
            out.push_str("TOOLS AVAILABLE NOW:\n");
            if tools.is_empty() {
                out.push_str("- No tools are available in this turn.\n");
            } else {
                for tool in tools {
                    if let Some(description) = tool_catalog_description(tool) {
                        out.push_str(description);
                        out.push('\n');
                    }
                }
            }
            out.push_str(
                "Only the tools listed above are available in this turn. Do not call any omitted tool.",
            );
        }
        None => {
            out.push_str("TOOLS:\n");
            for tool in ["Bash", "Read", "Write", "Edit", "Glob", "Grep"] {
                if let Some(description) = tool_catalog_description(tool) {
                    out.push_str(description);
                    out.push('\n');
                }
            }
            if out.ends_with('\n') {
                out.pop();
            }
        }
    }
    out
}

fn tool_catalog_description(name: &str) -> Option<&'static str> {
    match name {
        "Bash" => Some("- Bash(command): run a shell command in the project directory"),
        "Read" => Some("- Read(path[, start_line, end_line]): read a file or list a directory"),
        "Write" => Some("- Write(path, content): create or overwrite a file"),
        "Edit" => Some(
            "- Edit(path, old_string, new_string[, replace_all]): replace exact text in an existing file",
        ),
        "Glob" => Some("- Glob(pattern): find files by glob pattern"),
        "Grep" => Some("- Grep(pattern[, glob, case_sensitive]): search repository text"),
        _ => None,
    }
}

fn plan_file_alias(path: &Path) -> String {
    path.file_name()
        .map(|name| format!("plans/{}", name.to_string_lossy()))
        .unwrap_or_else(|| "plans/plan.md".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn restricted_tool_catalog_lists_only_allowed_tools() {
        let prompt = build_system_prompt(
            ExecutionMode::Act,
            None,
            TaskProfile::Generic,
            ToolProtocol::Native,
            None,
            &[],
            Some(&["Edit"]),
        );

        assert!(prompt.contains("TOOLS AVAILABLE NOW:"));
        assert!(prompt.contains("- Edit(path"));
        assert!(prompt.contains("Only the tools listed above are available"));
        assert!(prompt.contains("Bash is not available in this turn"));
        assert!(prompt.contains("Do not install dependencies in this restricted turn"));
        assert!(!prompt.contains("YOU run it with Bash"));
        assert!(!prompt.contains("Bash(npm install X)"));
        assert!(!prompt.contains("- Bash(command)"));
        assert!(!prompt.contains("- Read(path"));
        assert!(!prompt.contains("- Write(path"));
    }

    #[test]
    fn unrestricted_tool_catalog_preserves_normal_act_tools() {
        let prompt = build_system_prompt(
            ExecutionMode::Act,
            None,
            TaskProfile::Generic,
            ToolProtocol::Native,
            None,
            &[],
            None,
        );

        assert!(prompt.contains("TOOLS:"));
        assert!(prompt.contains("- Bash(command)"));
        assert!(prompt.contains("- Read(path"));
        assert!(prompt.contains("- Write(path"));
        assert!(prompt.contains("- Edit(path"));
    }

    #[test]
    fn plan_prompt_keeps_existing_plan_guidance_when_unrestricted() {
        let path = Path::new("/tmp/anvil-plan-test/plans/plan.md");
        let prompt = build_system_prompt(
            ExecutionMode::Plan,
            Some(path),
            TaskProfile::Generic,
            ToolProtocol::Native,
            Some(PlanStage::Stage2),
            &["Goal"],
            None,
        );

        assert!(prompt.contains("PLAN MODE:"));
        assert!(prompt.contains("plans/plan.md"));
        assert!(prompt.contains("- Bash(command)"));
        assert!(prompt.contains("Bash is disabled"));
    }
}
