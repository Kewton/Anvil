use serde::Deserialize;
use std::fmt::{Display, Formatter};
use std::path::Path;

/// Action to perform when a slash command is invoked.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SlashCommandAction {
    Help,
    Status,
    Plan,
    PlanAdd(String),
    PlanFocus(usize),
    PlanClear,
    RepoFind(String),
    Timeline,
    Compact,
    Model,
    Approve,
    Deny,
    Reset,
    Exit,
    Prompt(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SlashCommandSpec {
    pub name: String,
    pub description: String,
    pub action: SlashCommandAction,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExtensionRegistry {
    commands: Vec<SlashCommandSpec>,
}

#[derive(Debug)]
pub enum ExtensionLoadError {
    Unreadable(std::io::Error),
    InvalidJson(serde_json::Error),
    InvalidCommandName(String),
    DuplicateCommand(String),
}

impl Display for ExtensionLoadError {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Unreadable(err) => write!(f, "failed to read slash command extensions: {err}"),
            Self::InvalidJson(err) => write!(f, "invalid slash command extension json: {err}"),
            Self::InvalidCommandName(name) => {
                write!(f, "invalid custom slash command name: {name}")
            }
            Self::DuplicateCommand(name) => write!(f, "duplicate slash command: {name}"),
        }
    }
}

impl std::error::Error for ExtensionLoadError {}

#[derive(Debug, Deserialize)]
struct CustomSlashCommandFile {
    #[serde(default)]
    commands: Vec<CustomSlashCommandSpec>,
}

#[derive(Debug, Deserialize)]
struct CustomSlashCommandSpec {
    name: String,
    description: String,
    prompt: String,
}

impl ExtensionRegistry {
    pub fn new() -> Self {
        Self {
            commands: builtin_slash_commands(),
        }
    }

    pub fn load(cwd: &Path) -> Result<Self, ExtensionLoadError> {
        let mut registry = Self::new();
        let custom_path = cwd.join(".anvil").join("slash-commands.json");
        if !custom_path.exists() {
            return Ok(registry);
        }

        let contents = std::fs::read_to_string(&custom_path).map_err(ExtensionLoadError::Unreadable)?;
        let parsed: CustomSlashCommandFile =
            serde_json::from_str(&contents).map_err(ExtensionLoadError::InvalidJson)?;

        for command in parsed.commands {
            let name = normalize_command_name(&command.name)
                .ok_or_else(|| ExtensionLoadError::InvalidCommandName(command.name.clone()))?;
            if registry.commands.iter().any(|spec| spec.name == name) {
                return Err(ExtensionLoadError::DuplicateCommand(name));
            }
            registry.commands.push(SlashCommandSpec {
                name,
                description: command.description,
                action: SlashCommandAction::Prompt(command.prompt),
            });
        }

        Ok(registry)
    }

    pub fn slash_commands(&self) -> &[SlashCommandSpec] {
        &self.commands
    }

    pub fn find_slash_command(&self, command: &str) -> Option<SlashCommandSpec> {
        if let Some(parsed) = parse_plan_command(command) {
            return Some(parsed);
        }
        if let Some(parsed) = parse_repo_command(command) {
            return Some(parsed);
        }
        self.commands
            .iter()
            .find(|spec| spec.name == command || (spec.name == "/exit" && command == "/quit"))
            .cloned()
    }
}

pub fn builtin_slash_commands() -> Vec<SlashCommandSpec> {
    vec![
        SlashCommandSpec {
            name: "/help".to_string(),
            description: "show available commands".to_string(),
            action: SlashCommandAction::Help,
        },
        SlashCommandSpec {
            name: "/status".to_string(),
            description: "show the current console state".to_string(),
            action: SlashCommandAction::Status,
        },
        SlashCommandSpec {
            name: "/plan".to_string(),
            description: "show the current plan and active step".to_string(),
            action: SlashCommandAction::Plan,
        },
        SlashCommandSpec {
            name: "/plan-add".to_string(),
            description: "append a new item to the current plan".to_string(),
            action: SlashCommandAction::PlanAdd(String::new()),
        },
        SlashCommandSpec {
            name: "/plan-focus".to_string(),
            description: "set the active plan step by 1-based index".to_string(),
            action: SlashCommandAction::PlanFocus(0),
        },
        SlashCommandSpec {
            name: "/plan-clear".to_string(),
            description: "clear the current plan".to_string(),
            action: SlashCommandAction::PlanClear,
        },
        SlashCommandSpec {
            name: "/repo-find".to_string(),
            description: "search the repo by path and content".to_string(),
            action: SlashCommandAction::RepoFind(String::new()),
        },
        SlashCommandSpec {
            name: "/timeline".to_string(),
            description: "show the recent session timeline".to_string(),
            action: SlashCommandAction::Timeline,
        },
        SlashCommandSpec {
            name: "/compact".to_string(),
            description: "compact older session history into a summary".to_string(),
            action: SlashCommandAction::Compact,
        },
        SlashCommandSpec {
            name: "/model".to_string(),
            description: "show the current model context".to_string(),
            action: SlashCommandAction::Model,
        },
        SlashCommandSpec {
            name: "/approve".to_string(),
            description: "continue the pending approved tool call".to_string(),
            action: SlashCommandAction::Approve,
        },
        SlashCommandSpec {
            name: "/deny".to_string(),
            description: "reject the pending tool call".to_string(),
            action: SlashCommandAction::Deny,
        },
        SlashCommandSpec {
            name: "/reset".to_string(),
            description: "return to Ready".to_string(),
            action: SlashCommandAction::Reset,
        },
        SlashCommandSpec {
            name: "/exit".to_string(),
            description: "exit the session".to_string(),
            action: SlashCommandAction::Exit,
        },
    ]
}

fn normalize_command_name(name: &str) -> Option<String> {
    let trimmed = name.trim();
    if !trimmed.starts_with('/') || trimmed.len() <= 1 {
        return None;
    }
    if trimmed[1..]
        .chars()
        .all(|ch| ch.is_ascii_lowercase() || ch.is_ascii_digit() || ch == '-' || ch == '_')
    {
        Some(trimmed.to_string())
    } else {
        None
    }
}

fn parse_plan_command(command: &str) -> Option<SlashCommandSpec> {
    if let Some(rest) = command.strip_prefix("/plan-add ") {
        let item = rest.trim();
        if item.is_empty() {
            return None;
        }
        return Some(SlashCommandSpec {
            name: "/plan-add".to_string(),
            description: "append a new item to the current plan".to_string(),
            action: SlashCommandAction::PlanAdd(item.to_string()),
        });
    }

    if let Some(rest) = command.strip_prefix("/plan-focus ") {
        let one_based = rest.trim().parse::<usize>().ok()?;
        if one_based == 0 {
            return None;
        }
        return Some(SlashCommandSpec {
            name: "/plan-focus".to_string(),
            description: "set the active plan step by 1-based index".to_string(),
            action: SlashCommandAction::PlanFocus(one_based - 1),
        });
    }

    if command == "/plan-clear" {
        return Some(SlashCommandSpec {
            name: "/plan-clear".to_string(),
            description: "clear the current plan".to_string(),
            action: SlashCommandAction::PlanClear,
        });
    }

    None
}

fn parse_repo_command(command: &str) -> Option<SlashCommandSpec> {
    let rest = command.strip_prefix("/repo-find ")?;
    let query = rest.trim();
    if query.is_empty() {
        return None;
    }
    Some(SlashCommandSpec {
        name: "/repo-find".to_string(),
        description: "search the repo by path and content".to_string(),
        action: SlashCommandAction::RepoFind(query.to_string()),
    })
}
