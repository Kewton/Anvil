use std::path::Path;

use crate::state::memory::MemoryStore;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BuiltinCommand {
    MemoryAdd { text: String },
    MemoryShow,
    MemoryEdit { text: String },
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
    None
}
