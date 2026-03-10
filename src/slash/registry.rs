use std::path::Path;

use crate::slash::builtins::{BuiltinCommand, parse_builtin_command};
use crate::slash::custom::{
    CustomCommandDefinition, CustomCommandInvocation, load_custom_commands,
};

#[derive(Debug, Clone)]
pub struct SlashRegistry {
    custom_commands: Vec<CustomCommandDefinition>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ResolvedSlashCommand {
    Builtin(BuiltinCommand),
    Custom(CustomCommandInvocation),
}

impl SlashRegistry {
    pub fn load(root: &Path) -> anyhow::Result<Self> {
        Ok(Self {
            custom_commands: load_custom_commands(root)?,
        })
    }

    pub fn resolve(&self, input: &str) -> anyhow::Result<Option<ResolvedSlashCommand>> {
        if let Some(command) = parse_builtin_command(input) {
            return Ok(Some(ResolvedSlashCommand::Builtin(command)));
        }
        for def in &self.custom_commands {
            if let Some(invocation) = CustomCommandInvocation::parse(def, input)? {
                return Ok(Some(ResolvedSlashCommand::Custom(invocation)));
            }
        }
        Ok(None)
    }
}
