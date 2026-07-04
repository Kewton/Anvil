use crate::config::Config;

pub fn run_repl(config: Config) -> anyhow::Result<()> {
    crate::tui::repl::run(config)
}

pub use crate::tui::slash::expand_goal_references;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tui::slash::{parse_profile_style, parse_words};

    #[test]
    fn slash_command_parse_quoted_prompt() {
        let words = parse_words(r#"/ultra-plan-run --profile nextjs "3011 port app""#);
        assert_eq!(words[0], "/ultra-plan-run");
        assert_eq!(words[2], "nextjs");
        assert_eq!(words[3], "3011 port app");
    }

    #[test]
    fn slash_command_parse_profile_style_and_goal() {
        let config = Config {
            workspace_root: std::path::PathBuf::from("."),
            state_dir: std::path::PathBuf::from("state"),
            eval_events_path: None,
            completion_contract_path: None,
            yes: true,
            offline: false,
            context_budget: 1000,
            model: "m".to_string(),
            provider: crate::config::Provider::Ollama,
            planner_model: "m".to_string(),
            planner_provider: crate::config::Provider::Ollama,
            ollama_host: "http://localhost:11434".to_string(),
            num_predict: 100,
            max_iterations: 4,
            chat_timeout_secs: 1,
            chat_retries: 1,
            resume: None,
            fresh_session: false,
            no_footer: false,
            profile: "generic".to_string(),
            profile_explicit: false,
            profile_inference: None,
            style: "default".to_string(),
            action: crate::config::Action::Repl,
        };
        let words = parse_words(r#"/plan-run --profile nextjs --style compact "3011 port app""#);
        let (profile, style, rest) = parse_profile_style(&words[1..], &config);
        assert_eq!(profile, "nextjs");
        assert_eq!(style, "compact");
        assert_eq!(rest, vec!["3011 port app"]);
    }
}
