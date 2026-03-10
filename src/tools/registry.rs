use std::path::PathBuf;

use crate::tools::diff::DiffTool;
use crate::tools::env::{EnvSnapshot, EnvTool};
use crate::tools::exec::{ExecRequest, ExecResult, ExecTool};
use crate::tools::fs_edit::{FileEditResult, FileEditTool};
use crate::tools::fs_read::{FileReadResult, FileReadTool};
use crate::tools::search::{SearchMatch, SearchTool};

#[derive(Debug, Default)]
pub struct ToolRegistry {
    pub file_read: FileReadTool,
    pub file_edit: FileEditTool,
    pub search: SearchTool,
    pub exec: ExecTool,
    pub env: EnvTool,
    pub diff: DiffTool,
}

impl ToolRegistry {
    pub fn execute(&self, request: ToolRequest) -> anyhow::Result<ToolResponse> {
        match request {
            ToolRequest::ReadFile { path } => {
                Ok(ToolResponse::FileContents(self.file_read.read(&path)?))
            }
            ToolRequest::WriteFile { path, contents } => Ok(ToolResponse::WriteResult(
                self.file_edit.write(&path, &contents)?,
            )),
            ToolRequest::Search { root, needle } => Ok(ToolResponse::SearchMatches(
                self.search.search(&root, &needle)?,
            )),
            ToolRequest::Exec { request } => Ok(ToolResponse::ExecResult(self.exec.run(&request)?)),
            ToolRequest::InspectEnv => Ok(ToolResponse::EnvSnapshot(self.env.inspect()?)),
            ToolRequest::Diff { root } => Ok(ToolResponse::Diff(self.diff.diff(&root)?)),
        }
    }
}

#[derive(Debug, Clone)]
pub enum ToolRequest {
    ReadFile { path: PathBuf },
    WriteFile { path: PathBuf, contents: String },
    Search { root: PathBuf, needle: String },
    Exec { request: ExecRequest },
    InspectEnv,
    Diff { root: PathBuf },
}

#[derive(Debug, Clone)]
pub enum ToolResponse {
    FileContents(FileReadResult),
    WriteResult(FileEditResult),
    SearchMatches(Vec<SearchMatch>),
    ExecResult(ExecResult),
    EnvSnapshot(EnvSnapshot),
    Diff(String),
}
