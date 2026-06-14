use std::io::{self, BufRead, Write};

use crate::config::Config;
use crate::model_registry::RuntimeModels;
use crate::ollama::client::OllamaClient;
use crate::session::store::{SessionSnapshot, SessionStore};

use super::minimal_loop::{
    MinimalChatClient, MinimalLoopConfig, completion_without_write_feedback_disabled_from_env,
    requested_artifact_feedback_disabled_from_env, run_session,
};

#[derive(Debug, Clone, PartialEq, Eq)]
enum ReplInput {
    Empty,
    Exit,
    Prompt(String),
}

pub fn run(
    config: Config,
    models: RuntimeModels,
    mut client: OllamaClient,
    session_store: SessionStore,
    mut session: SessionSnapshot,
) -> Result<(), String> {
    let stdin = io::stdin();
    let mut stdin = stdin.lock();
    let mut line = String::new();

    loop {
        print!("anvil> ");
        io::stdout()
            .flush()
            .map_err(|err| format!("failed to flush REPL prompt: {err}"))?;

        line.clear();
        let bytes = stdin
            .read_line(&mut line)
            .map_err(|err| format!("failed to read REPL input: {err}"))?;
        if bytes == 0 {
            break;
        }

        match parse_repl_input(&line) {
            ReplInput::Empty => continue,
            ReplInput::Exit => break,
            ReplInput::Prompt(prompt) => {
                let reply = run_turn(
                    &config,
                    &models.main,
                    &mut client,
                    &session_store,
                    &mut session,
                    &prompt,
                )?;
                if !reply.is_empty() {
                    println!("{reply}");
                }
            }
        }
    }

    Ok(())
}

pub(crate) fn run_turn<C: MinimalChatClient>(
    config: &Config,
    model: &str,
    client: &mut C,
    session_store: &SessionStore,
    session: &mut SessionSnapshot,
    prompt: &str,
) -> Result<String, String> {
    let work_root = session
        .active_root
        .clone()
        .unwrap_or_else(|| config.cwd.clone());
    let loop_config = MinimalLoopConfig {
        work_root: work_root.clone(),
        mode: session.mode_state.mode,
        context_budget: config.context_budget,
        max_iterations: config.max_iterations,
        auto_approve: config.yes_mode,
        offline: config.offline,
        cancel_flag: None,
        completion_without_write_feedback: !completion_without_write_feedback_disabled_from_env(),
        requested_artifact_feedback: !requested_artifact_feedback_disabled_from_env(),
    };
    let reply = run_session(client, model, session, prompt, &loop_config)?;
    session.active_root = (work_root != config.cwd).then_some(work_root);
    session_store.save(session)?;
    Ok(reply)
}

fn parse_repl_input(input: &str) -> ReplInput {
    let line = input.trim_end_matches(['\r', '\n']);
    match line.trim() {
        "" => ReplInput::Empty,
        "/exit" | "/quit" => ReplInput::Exit,
        _ => ReplInput::Prompt(line.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use std::collections::VecDeque;
    use std::path::PathBuf;

    use crate::modes::plan_act::ExecutionMode;
    use crate::ollama::client::AssistantReply;
    use crate::session::store::ConversationMessage;
    use crate::tools::registry::ToolSpec;

    use super::*;

    #[derive(Default)]
    struct MockClient {
        replies: VecDeque<AssistantReply>,
        prompts_seen: Vec<String>,
    }

    impl MockClient {
        fn push_reply(&mut self, content: &str) {
            self.replies.push_back(AssistantReply {
                content: content.to_string(),
                tool_calls: Vec::new(),
                prompt_tokens: None,
                completion_tokens: None,
            });
        }
    }

    impl MinimalChatClient for MockClient {
        fn chat(
            &mut self,
            _model: &str,
            messages: &[ConversationMessage],
            _tools: &[ToolSpec],
            _native_tools_enabled: bool,
        ) -> Result<AssistantReply, String> {
            self.prompts_seen.extend(
                messages
                    .iter()
                    .filter(|message| message.role == "user")
                    .map(|message| message.content.clone()),
            );
            self.replies
                .pop_front()
                .ok_or_else(|| "no reply".to_string())
        }
    }

    fn config(cwd: PathBuf) -> Config {
        let mut config = Config::default();
        config.cwd = cwd;
        config.context_budget = 24_000;
        config.max_iterations = 2;
        config.yes_mode = true;
        config
    }

    #[test]
    fn parse_repl_input_skips_empty_and_exits() {
        assert_eq!(parse_repl_input("\n"), ReplInput::Empty);
        assert_eq!(parse_repl_input("   \n"), ReplInput::Empty);
        assert_eq!(parse_repl_input("/exit\n"), ReplInput::Exit);
        assert_eq!(parse_repl_input(" /quit \r\n"), ReplInput::Exit);
    }

    #[test]
    fn parse_repl_input_preserves_prompt_text() {
        assert_eq!(
            parse_repl_input("  write hello  \n"),
            ReplInput::Prompt("  write hello  ".to_string())
        );
    }

    #[test]
    fn run_turn_invokes_minimal_session_and_saves_snapshot() {
        let workspace = tempfile::tempdir().unwrap();
        let state = tempfile::tempdir().unwrap();
        let store = SessionStore::new(state.path(), "session-1", "workspace-1");
        let mut session = SessionSnapshot::default();
        session.id = "session-1".to_string();
        session.workspace_key = "workspace-1".to_string();
        session.mode_state.mode = ExecutionMode::Plan;
        let mut client = MockClient::default();
        client.push_reply("first reply");

        let reply = run_turn(
            &config(workspace.path().to_path_buf()),
            "qwen3:8b",
            &mut client,
            &store,
            &mut session,
            "first prompt",
        )
        .unwrap();

        assert_eq!(reply, "first reply");
        assert_eq!(client.prompts_seen, vec!["first prompt"]);
        let saved = std::fs::read_to_string(store.path()).unwrap();
        assert!(saved.contains("first prompt"));
        assert!(saved.contains("first reply"));
    }
}
