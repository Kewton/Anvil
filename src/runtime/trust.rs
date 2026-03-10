#[derive(Debug, Clone, Copy)]
pub enum SourceType {
    RuntimePolicy,
    User,
    AnvilMd,
    Memory,
    Handoff,
    RepoFile,
    ToolOutput,
}
