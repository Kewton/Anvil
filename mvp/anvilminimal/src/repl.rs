use std::io::{self, IsTerminal, Write};

use anyhow::bail;

use crate::config::Config;

pub fn run_repl(config: Config) -> anyhow::Result<()> {
    if !io::stdin().is_terminal() {
        bail!("stdin is not a TTY; pass --prompt or an action flag");
    }
    let mut execution = crate::providers::client_from_config(&config, false)?;
    let mut planner = crate::providers::client_from_config(&config, true)?;
    loop {
        print!("anvil> ");
        io::stdout().flush()?;
        let mut line = String::new();
        if io::stdin().read_line(&mut line)? == 0 {
            break;
        }
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        if matches!(line, "/exit" | "/quit") {
            break;
        }
        match handle_command(line, &config, &mut *planner, &mut *execution) {
            Ok(output) => println!("{output}"),
            Err(err) => eprintln!("error: {err:#}"),
        }
    }
    Ok(())
}

fn handle_command(
    line: &str,
    config: &Config,
    planner: &mut dyn crate::providers::ChatClient,
    execution: &mut dyn crate::providers::ChatClient,
) -> anyhow::Result<String> {
    let args = parse_words(line);
    let Some(command) = args.first().map(String::as_str) else {
        bail!("empty command");
    };
    let (profile, style, rest) = parse_profile_style(&args[1..], config);
    let goal = expand_goal_references(&rest.join(" "), config)?;
    let mut config = config.clone();
    config.profile = profile;
    config.style = style;
    match command {
        "/plan-steps" => {
            let plan = crate::planner::generate_step_plan(planner, &goal, &config)?;
            let path = crate::planner::save_step_plan(&config.workspace_root, &plan)?;
            Ok(path.display().to_string())
        }
        "/plan-run" => {
            crate::planner::generate_and_run_step_plan(planner, execution, &goal, &config)
        }
        "/run-plan" => {
            crate::planner::run_plan_file(execution, std::path::Path::new(&goal), &config)
        }
        "/ultra-plan" => {
            let plan = crate::planner::generate_ultra_plan(planner, &goal, &config)?;
            let path = crate::planner::save_ultra_plan(&config.workspace_root, &plan)?;
            Ok(path.display().to_string())
        }
        "/ultra-plan-run" => {
            crate::planner::generate_and_run_ultra_plan(planner, execution, &goal, &config)
        }
        "/run-ultra-plan" => crate::planner::run_ultra_plan_file(
            planner,
            execution,
            std::path::Path::new(&goal),
            &config,
        ),
        other => bail!("unknown slash command: {other}"),
    }
}

fn parse_profile_style(args: &[String], config: &Config) -> (String, String, Vec<String>) {
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

fn parse_words(input: &str) -> Vec<String> {
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
            profile: "generic".to_string(),
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
