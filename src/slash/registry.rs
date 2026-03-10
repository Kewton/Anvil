use crate::slash::builtins::{BuiltinCommand, parse_builtin_command};

#[derive(Debug, Default, Clone)]
pub struct SlashRegistry;

impl SlashRegistry {
    pub fn resolve(&self, input: &str) -> Option<BuiltinCommand> {
        parse_builtin_command(input)
    }
}
