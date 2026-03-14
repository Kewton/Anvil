/// Slash command registry for interactive CLI extensions.
///
/// Commands are statically defined and looked up by the [`ExtensionRegistry`].

/// Action to perform when a slash command is invoked.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SlashCommandAction {
    Help,
    Status,
    Plan,
    Model,
    Approve,
    Deny,
    Reset,
    Exit,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SlashCommandSpec {
    pub name: &'static str,
    pub description: &'static str,
    pub action: SlashCommandAction,
}

const SLASH_COMMANDS: [SlashCommandSpec; 8] = [
    SlashCommandSpec {
        name: "/help",
        description: "show available commands",
        action: SlashCommandAction::Help,
    },
    SlashCommandSpec {
        name: "/status",
        description: "show the current console state",
        action: SlashCommandAction::Status,
    },
    SlashCommandSpec {
        name: "/plan",
        description: "show the current plan and active step",
        action: SlashCommandAction::Plan,
    },
    SlashCommandSpec {
        name: "/model",
        description: "show the current model context",
        action: SlashCommandAction::Model,
    },
    SlashCommandSpec {
        name: "/approve",
        description: "continue the pending approved tool call",
        action: SlashCommandAction::Approve,
    },
    SlashCommandSpec {
        name: "/deny",
        description: "reject the pending tool call",
        action: SlashCommandAction::Deny,
    },
    SlashCommandSpec {
        name: "/reset",
        description: "return to Ready",
        action: SlashCommandAction::Reset,
    },
    SlashCommandSpec {
        name: "/exit",
        description: "exit the session",
        action: SlashCommandAction::Exit,
    },
];

pub struct ExtensionRegistry;

impl ExtensionRegistry {
    pub fn new() -> Self {
        Self
    }

    pub fn slash_commands(&self) -> &'static [SlashCommandSpec] {
        &SLASH_COMMANDS
    }

    pub fn find_slash_command(&self, command: &str) -> Option<SlashCommandSpec> {
        self.slash_commands()
            .iter()
            .copied()
            .find(|spec| spec.name == command || (spec.name == "/exit" && command == "/quit"))
    }
}
