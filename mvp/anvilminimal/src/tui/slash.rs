use std::path::Path;

use anyhow::bail;

use crate::config::Config;
use crate::providers::ChatClient;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParsedSlash {
    pub command: String,
    pub profile: String,
    pub style: String,
    pub goal: String,
}

pub fn parse_slash(line: &str, config: &Config) -> anyhow::Result<ParsedSlash> {
    let args = parse_words(line);
    let Some(command) = args.first() else {
        bail!("empty command");
    };
    let (profile, style, rest) = parse_profile_style(&args[1..], config);
    let goal = expand_goal_references(&rest.join(" "), config)?;
    Ok(ParsedSlash {
        command: command.clone(),
        profile,
        style,
        goal,
    })
}

pub fn handle_command(
    line: &str,
    config: &Config,
    planner: &mut dyn ChatClient,
    execution: &mut dyn ChatClient,
    ui: &dyn crate::tui::InteractionUi,
) -> anyhow::Result<String> {
    let parsed = parse_slash(line, config)?;
    let mut config = config.clone();
    config.profile = parsed.profile;
    config.style = parsed.style;
    match parsed.command.as_str() {
        "/plan-steps" => {
            let plan =
                crate::planner::generate_step_plan_with_ui(planner, &parsed.goal, &config, ui)?;
            let path = crate::planner::save_step_plan(&config.workspace_root, &plan)?;
            Ok(path.display().to_string())
        }
        "/plan-run" => crate::planner::generate_and_run_step_plan_with_ui(
            planner,
            execution,
            &parsed.goal,
            &config,
            ui,
        ),
        "/run-plan" => {
            crate::planner::run_plan_file_with_ui(execution, Path::new(&parsed.goal), &config, ui)
        }
        "/ultra-plan" => {
            let plan =
                crate::planner::generate_ultra_plan_with_ui(planner, &parsed.goal, &config, ui)?;
            let path = crate::planner::save_ultra_plan(&config.workspace_root, &plan)?;
            Ok(path.display().to_string())
        }
        "/ultra-plan-run" => crate::planner::generate_and_run_ultra_plan_with_ui(
            planner,
            execution,
            &parsed.goal,
            &config,
            ui,
        ),
        "/run-ultra-plan" => crate::planner::run_ultra_plan_file_with_ui(
            planner,
            execution,
            Path::new(&parsed.goal),
            &config,
            ui,
        ),
        other => bail!("unknown slash command: {other}"),
    }
}

pub fn parse_profile_style(args: &[String], config: &Config) -> (String, String, Vec<String>) {
    let mut profile = config.profile.clone();
    let mut style = config.style.clone();
    let mut rest = Vec::new();
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--profile" if i + 1 < args.len() => {
                profile = args[i + 1].clone();
                i += 2;
            }
            "--style" if i + 1 < args.len() => {
                style = args[i + 1].clone();
                i += 2;
            }
            _ => {
                rest.push(args[i].clone());
                i += 1;
            }
        }
    }
    (profile, style, rest)
}

pub fn expand_goal_references(goal: &str, config: &Config) -> anyhow::Result<String> {
    let mut out = goal.to_string();
    while let Some(start) = out.find("$(cat ") {
        let Some(end_rel) = out[start..].find(')') else {
            break;
        };
        let end = start + end_rel;
        let raw = out[start + "$(cat ".len()..end].trim();
        let path = crate::tools::path_guard::resolve_existing(&config.workspace_root, raw)?;
        let content = std::fs::read_to_string(path)?;
        out.replace_range(start..=end, &content);
    }
    Ok(out)
}

pub fn parse_words(input: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut current = String::new();
    let mut quoted = false;
    for ch in input.chars() {
        match ch {
            '"' => quoted = !quoted,
            ' ' | '\t' if !quoted => {
                if !current.is_empty() {
                    out.push(std::mem::take(&mut current));
                }
            }
            _ => current.push(ch),
        }
    }
    if !current.is_empty() {
        out.push(current);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn config() -> Config {
        Config {
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
            style: "default".to_string(),
            action: crate::config::Action::Repl,
        }
    }

    #[test]
    fn slash_command_parse_quoted_prompt() {
        let words = parse_words(r#"/ultra-plan-run --profile nextjs "3011 port app""#);
        assert_eq!(words[0], "/ultra-plan-run");
        assert_eq!(words[2], "nextjs");
        assert_eq!(words[3], "3011 port app");
    }

    #[test]
    fn slash_command_parse_profile_style_and_goal() {
        let words = parse_words(r#"/plan-run --profile nextjs --style compact "3011 port app""#);
        let (profile, style, rest) = parse_profile_style(&words[1..], &config());
        assert_eq!(profile, "nextjs");
        assert_eq!(style, "compact");
        assert_eq!(rest, vec!["3011 port app"]);
    }

    #[test]
    fn parse_slash_uses_config_defaults() {
        let parsed = parse_slash("/plan-run goal", &config()).unwrap();
        assert_eq!(parsed.profile, "generic");
        assert_eq!(parsed.style, "default");
        assert_eq!(parsed.goal, "goal");
    }

    #[test]
    fn parses_user_ultra_plan_run_nextjs_command() {
        let parsed = parse_slash(
            "/ultra-plan-run --profile nextjs あなたが考える最高に面白くかっこいいスペースインベーダーゲームを3011ポートで起動可能なnext.jsアプリとして開発してください。",
            &config(),
        )
        .unwrap();
        assert_eq!(parsed.command, "/ultra-plan-run");
        assert_eq!(parsed.profile, "nextjs");
        assert_eq!(
            parsed.goal,
            "あなたが考える最高に面白くかっこいいスペースインベーダーゲームを3011ポートで起動可能なnext.jsアプリとして開発してください。"
        );
    }
}
