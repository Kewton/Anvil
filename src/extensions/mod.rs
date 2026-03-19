pub mod skills;

use serde::Deserialize;
use skills::SkillScope;
use std::fmt::{Display, Formatter};
use std::path::{Path, PathBuf};

/// Sub-actions for the /trust slash command.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TrustAction {
    Show,
    Tool(String),
    All,
    Off,
}

/// Action to perform when a slash command is invoked.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SlashCommandAction {
    Help,
    Status,
    Plan,
    PlanAdd(String),
    PlanFocus(usize),
    PlanClear,
    Checkpoint(String),
    RepoFind(String),
    Timeline,
    Compact,
    ModelList,
    ModelSwitch(String),
    ModelInfo,
    Provider,
    Approve,
    Deny,
    Reset,
    Exit,
    Prompt(String),
    Skill {
        name: String,
        args: String,
        content: String,
        skill_dir: PathBuf,
    },
    SessionList,
    SessionSwitch(String),
    SessionDelete(String),
    Trust(TrustAction),
    Undo(usize),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SlashCommandSpec {
    pub name: String,
    pub description: String,
    pub action: SlashCommandAction,
    pub scope: Option<SkillScope>,
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

impl Default for ExtensionRegistry {
    fn default() -> Self {
        Self::new()
    }
}

impl ExtensionRegistry {
    pub fn new() -> Self {
        Self {
            commands: builtin_slash_commands(),
        }
    }

    pub fn load(cwd: &Path, home_dir: Option<&Path>) -> Result<Self, ExtensionLoadError> {
        let mut registry = Self::new();

        // Load custom slash commands from .anvil/slash-commands.json
        let custom_path = cwd.join(".anvil").join("slash-commands.json");
        if custom_path.exists() {
            let contents =
                std::fs::read_to_string(&custom_path).map_err(ExtensionLoadError::Unreadable)?;
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
                    scope: None,
                });
            }
        }

        // Load skills from user and project scopes
        let skill_commands = skills::discover_and_load(cwd, home_dir, &registry.commands);
        registry.commands.extend(skill_commands);

        Ok(registry)
    }

    pub fn slash_commands(&self) -> &[SlashCommandSpec] {
        &self.commands
    }

    pub fn find_slash_command(&self, command: &str) -> Option<SlashCommandSpec> {
        if let Some(parsed) = parse_undo_command(command) {
            return Some(parsed);
        }
        if let Some(parsed) = parse_plan_command(command) {
            return Some(parsed);
        }
        if let Some(parsed) = parse_repo_command(command) {
            return Some(parsed);
        }
        if let Some(parsed) = parse_session_command(command) {
            return Some(parsed);
        }
        if let Some(parsed) = parse_model_command(command) {
            return Some(parsed);
        }
        if let Some(parsed) = parse_trust_command(command) {
            return Some(parsed);
        }
        if let found @ Some(_) = self
            .commands
            .iter()
            .find(|spec| spec.name == command || (spec.name == "/exit" && command == "/quit"))
            .cloned()
        {
            return found;
        }
        // Try skill command with argument separation
        if let Some(spec) = skills::parse_skill_command(command, &self.commands) {
            return Some(spec);
        }
        None
    }

    /// Suggest the closest matching command name for typo correction.
    pub fn suggest_command(&self, input: &str) -> Option<&str> {
        let cmd = input.split_whitespace().next().unwrap_or(input);
        self.commands
            .iter()
            .map(|spec| (spec.name.as_str(), edit_distance(cmd, &spec.name)))
            .filter(|(_, dist)| *dist <= 2)
            .min_by_key(|(_, dist)| *dist)
            .map(|(name, _)| name)
    }
}

pub fn builtin_slash_commands() -> Vec<SlashCommandSpec> {
    vec![
        SlashCommandSpec {
            name: "/help".to_string(),
            description: "show available commands".to_string(),
            action: SlashCommandAction::Help,
            scope: None,
        },
        SlashCommandSpec {
            name: "/status".to_string(),
            description: "show the current console state".to_string(),
            action: SlashCommandAction::Status,
            scope: None,
        },
        SlashCommandSpec {
            name: "/plan".to_string(),
            description: "show the current plan and active step".to_string(),
            action: SlashCommandAction::Plan,
            scope: None,
        },
        SlashCommandSpec {
            name: "/plan-add".to_string(),
            description: "append a new item to the current plan".to_string(),
            action: SlashCommandAction::PlanAdd(String::new()),
            scope: None,
        },
        SlashCommandSpec {
            name: "/plan-focus".to_string(),
            description: "set the active plan step by 1-based index".to_string(),
            action: SlashCommandAction::PlanFocus(0),
            scope: None,
        },
        SlashCommandSpec {
            name: "/plan-clear".to_string(),
            description: "clear the current plan".to_string(),
            action: SlashCommandAction::PlanClear,
            scope: None,
        },
        SlashCommandSpec {
            name: "/checkpoint".to_string(),
            description: "save a planning checkpoint note".to_string(),
            action: SlashCommandAction::Checkpoint(String::new()),
            scope: None,
        },
        SlashCommandSpec {
            name: "/repo-find".to_string(),
            description: "search the repo by path and content".to_string(),
            action: SlashCommandAction::RepoFind(String::new()),
            scope: None,
        },
        SlashCommandSpec {
            name: "/timeline".to_string(),
            description: "show the recent session timeline".to_string(),
            action: SlashCommandAction::Timeline,
            scope: None,
        },
        SlashCommandSpec {
            name: "/compact".to_string(),
            description: "compact older session history into a summary".to_string(),
            action: SlashCommandAction::Compact,
            scope: None,
        },
        SlashCommandSpec {
            name: "/model".to_string(),
            description: "model management (list/switch/info)".to_string(),
            action: SlashCommandAction::ModelInfo,
            scope: None,
        },
        SlashCommandSpec {
            name: "/provider".to_string(),
            description: "show provider backend and capability diagnostics".to_string(),
            action: SlashCommandAction::Provider,
            scope: None,
        },
        SlashCommandSpec {
            name: "/approve".to_string(),
            description: "continue the pending approved tool call".to_string(),
            action: SlashCommandAction::Approve,
            scope: None,
        },
        SlashCommandSpec {
            name: "/deny".to_string(),
            description: "reject the pending tool call".to_string(),
            action: SlashCommandAction::Deny,
            scope: None,
        },
        SlashCommandSpec {
            name: "/reset".to_string(),
            description: "return to Ready".to_string(),
            action: SlashCommandAction::Reset,
            scope: None,
        },
        SlashCommandSpec {
            name: "/trust".to_string(),
            description: "show or manage trust settings (/trust [all|off|<tool>])".to_string(),
            action: SlashCommandAction::Trust(TrustAction::Show),
            scope: None,
        },
        SlashCommandSpec {
            name: "/undo".to_string(),
            description: "undo the last file change(s)".to_string(),
            action: SlashCommandAction::Undo(1),
            scope: None,
        },
        SlashCommandSpec {
            name: "/exit".to_string(),
            description: "exit the session".to_string(),
            action: SlashCommandAction::Exit,
            scope: None,
        },
        SlashCommandSpec {
            name: "/session".to_string(),
            description: "manage sessions (list/switch/delete)".to_string(),
            action: SlashCommandAction::SessionList,
            scope: None,
        },
    ]
}

pub(crate) fn normalize_command_name(name: &str) -> Option<String> {
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
            scope: None,
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
            scope: None,
        });
    }

    if command == "/plan-clear" {
        return Some(SlashCommandSpec {
            name: "/plan-clear".to_string(),
            description: "clear the current plan".to_string(),
            action: SlashCommandAction::PlanClear,
            scope: None,
        });
    }

    if let Some(rest) = command.strip_prefix("/checkpoint ") {
        let note = rest.trim();
        if note.is_empty() {
            return None;
        }
        return Some(SlashCommandSpec {
            name: "/checkpoint".to_string(),
            description: "save a planning checkpoint note".to_string(),
            action: SlashCommandAction::Checkpoint(note.to_string()),
            scope: None,
        });
    }

    None
}

fn edit_distance(a: &str, b: &str) -> usize {
    let a: Vec<char> = a.chars().collect();
    let b: Vec<char> = b.chars().collect();
    let n = b.len();
    let mut prev = (0..=n).collect::<Vec<_>>();
    let mut curr = vec![0usize; n + 1];
    for (i, ca) in a.iter().enumerate() {
        curr[0] = i + 1;
        for (j, cb) in b.iter().enumerate() {
            let cost = if ca == cb { 0 } else { 1 };
            curr[j + 1] = (prev[j + 1] + 1).min(curr[j] + 1).min(prev[j] + cost);
        }
        std::mem::swap(&mut prev, &mut curr);
    }
    prev[n]
}

fn parse_session_command(command: &str) -> Option<SlashCommandSpec> {
    let rest = command.strip_prefix("/session")?.trim();

    let action = if rest.is_empty() || rest == "list" {
        SlashCommandAction::SessionList
    } else if let Some(name) = rest.strip_prefix("switch ") {
        let name = name.trim();
        if name.is_empty() {
            return None;
        }
        SlashCommandAction::SessionSwitch(name.to_string())
    } else if let Some(name) = rest.strip_prefix("delete ") {
        let name = name.trim();
        if name.is_empty() {
            return None;
        }
        SlashCommandAction::SessionDelete(name.to_string())
    } else {
        return None;
    };

    Some(SlashCommandSpec {
        name: "/session".to_string(),
        description: "manage sessions (list/switch/delete)".to_string(),
        action,
        scope: None,
    })
}

/// Maximum number of undo steps allowed in a single `/undo N` command.
const MAX_UNDO_STEPS: usize = 20;

fn parse_undo_command(command: &str) -> Option<SlashCommandSpec> {
    let n = if command == "/undo" {
        1
    } else if let Some(rest) = command.strip_prefix("/undo ") {
        let parsed = rest.trim().parse::<usize>().ok()?;
        if parsed == 0 {
            return None;
        }
        parsed.min(MAX_UNDO_STEPS)
    } else {
        return None;
    };

    Some(SlashCommandSpec {
        name: "/undo".to_string(),
        description: "undo the last file change(s)".to_string(),
        action: SlashCommandAction::Undo(n),
        scope: None,
    })
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
        scope: None,
    })
}

fn is_valid_model_name(name: &str) -> bool {
    if name.is_empty() || name.len() > 128 {
        return false;
    }
    // '/' is not allowed (path traversal '../' prevention)
    name.chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_' || c == ':' || c == '.')
}

fn parse_model_command(command: &str) -> Option<SlashCommandSpec> {
    let trimmed = command.trim();
    if !trimmed.starts_with("/model") {
        return None;
    }
    let rest = trimmed.strip_prefix("/model")?.trim();

    let action = if rest.is_empty() {
        SlashCommandAction::ModelInfo // /model (no args) = info
    } else if rest == "list" {
        SlashCommandAction::ModelList
    } else if rest == "info" {
        SlashCommandAction::ModelInfo
    } else if let Some(name) = rest.strip_prefix("switch ") {
        let name = name.trim();
        if name.is_empty() {
            return None;
        }
        if !is_valid_model_name(name) {
            return None;
        }
        SlashCommandAction::ModelSwitch(name.to_string())
    } else if rest == "switch" {
        return None; // /model switch without arg → None
    } else {
        return None;
    };

    Some(SlashCommandSpec {
        name: trimmed.to_string(),
        description: String::new(),
        action,
        scope: None,
    })
}

fn parse_trust_command(command: &str) -> Option<SlashCommandSpec> {
    if command == "/trust" {
        return Some(SlashCommandSpec {
            name: "/trust".to_string(),
            description: "show current trust settings".to_string(),
            action: SlashCommandAction::Trust(TrustAction::Show),
            scope: None,
        });
    }

    let rest = command.strip_prefix("/trust ")?.trim();
    if rest.is_empty() {
        return Some(SlashCommandSpec {
            name: "/trust".to_string(),
            description: "show current trust settings".to_string(),
            action: SlashCommandAction::Trust(TrustAction::Show),
            scope: None,
        });
    }

    let action = match rest {
        "all" => TrustAction::All,
        "off" => TrustAction::Off,
        tool_name => TrustAction::Tool(tool_name.to_string()),
    };

    Some(SlashCommandSpec {
        name: "/trust".to_string(),
        description: "manage trust settings".to_string(),
        action: SlashCommandAction::Trust(action),
        scope: None,
    })
}

#[cfg(test)]
mod trust_parse_tests {
    use super::*;

    #[test]
    fn parse_trust_show() {
        let result = parse_trust_command("/trust").unwrap();
        assert_eq!(result.action, SlashCommandAction::Trust(TrustAction::Show));
    }

    #[test]
    fn parse_trust_show_with_trailing_space() {
        let result = parse_trust_command("/trust ").unwrap();
        assert_eq!(result.action, SlashCommandAction::Trust(TrustAction::Show));
    }

    #[test]
    fn parse_trust_all() {
        let result = parse_trust_command("/trust all").unwrap();
        assert_eq!(result.action, SlashCommandAction::Trust(TrustAction::All));
    }

    #[test]
    fn parse_trust_off() {
        let result = parse_trust_command("/trust off").unwrap();
        assert_eq!(result.action, SlashCommandAction::Trust(TrustAction::Off));
    }

    #[test]
    fn parse_trust_tool() {
        let result = parse_trust_command("/trust file.edit").unwrap();
        assert_eq!(
            result.action,
            SlashCommandAction::Trust(TrustAction::Tool("file.edit".to_string()))
        );
    }

    #[test]
    fn parse_trust_mcp_tool() {
        let result = parse_trust_command("/trust mcp__github__create_issue").unwrap();
        assert_eq!(
            result.action,
            SlashCommandAction::Trust(TrustAction::Tool("mcp__github__create_issue".to_string()))
        );
    }

    #[test]
    fn parse_non_trust_command_returns_none() {
        assert!(parse_trust_command("/help").is_none());
    }
}
