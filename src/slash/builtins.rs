use std::path::Path;

use crate::state::memory::MemoryStore;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BuiltinCommand {
    MemoryAdd { text: String },
    MemoryShow,
    MemoryEdit { text: String },
    PlanCreate { slug: String, text: String },
    PlanShow,
    Act { path: Option<String> },
    SubagentRun { task: String },
}

impl BuiltinCommand {
    pub fn execute(&self, memory_path: &Path) -> anyhow::Result<String> {
        let store = MemoryStore::new(memory_path);
        match self {
            Self::MemoryAdd { text } => {
                store.add_entry(text)?;
                Ok("memory updated".to_string())
            }
            Self::MemoryShow => store.load(),
            Self::MemoryEdit { text } => {
                store.replace_all(text)?;
                Ok("memory replaced".to_string())
            }
            Self::PlanCreate { .. }
            | Self::PlanShow
            | Self::Act { .. }
            | Self::SubagentRun { .. } => anyhow::bail!("command requires agent context"),
        }
    }
}

pub fn parse_builtin_command(input: &str) -> Option<BuiltinCommand> {
    if let Some(text) = input.strip_prefix("/memory add ") {
        return Some(BuiltinCommand::MemoryAdd {
            text: text.trim().to_string(),
        });
    }
    if input.trim() == "/memory show" {
        return Some(BuiltinCommand::MemoryShow);
    }
    if let Some(text) = input.strip_prefix("/memory edit ") {
        return Some(BuiltinCommand::MemoryEdit {
            text: text.trim().to_string(),
        });
    }
    if let Some(rest) = input.strip_prefix("/plan create ") {
        let mut parts = rest.trim().splitn(2, ' ');
        return Some(BuiltinCommand::PlanCreate {
            slug: parts.next()?.trim().to_string(),
            text: parts.next()?.trim().to_string(),
        });
    }
    if input.trim() == "/plan show" {
        return Some(BuiltinCommand::PlanShow);
    }
    if let Some(path) = input.strip_prefix("/act ") {
        return Some(BuiltinCommand::Act {
            path: Some(path.trim().to_string()),
        });
    }
    if input.trim() == "/act" {
        return Some(BuiltinCommand::Act { path: None });
    }
    if let Some(task) = input.strip_prefix("/subagent ") {
        return Some(BuiltinCommand::SubagentRun {
            task: task.trim().to_string(),
        });
    }
    None
}
