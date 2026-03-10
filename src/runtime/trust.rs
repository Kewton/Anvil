use std::fmt;

#[derive(Debug, Clone, Copy, Eq, Ord, PartialEq, PartialOrd)]
pub enum SourceType {
    RuntimePolicy,
    User,
    AnvilMd,
    Memory,
    Handoff,
    RepoFile,
    ToolOutput,
}

impl SourceType {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::RuntimePolicy => "runtime-policy",
            Self::User => "user",
            Self::AnvilMd => "anvil-md",
            Self::Memory => "memory",
            Self::Handoff => "handoff",
            Self::RepoFile => "repo-file",
            Self::ToolOutput => "tool-output",
        }
    }

    pub fn precedence(self) -> u8 {
        match self {
            Self::RuntimePolicy => 0,
            Self::User => 1,
            Self::AnvilMd => 2,
            Self::Memory => 3,
            Self::Handoff => 3,
            Self::RepoFile => 4,
            Self::ToolOutput => 5,
        }
    }
}

impl fmt::Display for SourceType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}
