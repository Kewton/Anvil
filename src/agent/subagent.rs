use std::collections::BTreeSet;
use std::path::PathBuf;

use anyhow::anyhow;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::policy::permissions::PermissionCategory;
use crate::state::audit::{
    AuditActor, AuditEvent, AuditEventData, AuditLog, AuditMetadata, AuditSource,
};
use crate::tools::{glob_paths, read_file, search_in_files};

#[derive(Debug, Clone)]
pub struct SubagentRequest {
    pub task: String,
    pub granted_permissions: Vec<PermissionCategory>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SubagentReport {
    pub summary: String,
    pub key_findings: Vec<String>,
    pub referenced_files: Vec<PathBuf>,
    pub recommended_next_action: String,
    pub artifacts: Vec<PathBuf>,
}

#[derive(Debug, Clone)]
pub struct SubagentRunner {
    root: PathBuf,
    state_dir: PathBuf,
}

impl SubagentRunner {
    pub fn new(root: impl Into<PathBuf>, state_dir: impl Into<PathBuf>) -> Self {
        Self {
            root: root.into(),
            state_dir: state_dir.into(),
        }
    }

    pub fn run(
        &self,
        session_id: &str,
        audit: &AuditLog,
        req: SubagentRequest,
    ) -> anyhow::Result<SubagentReport> {
        let subagent_id = format!("sub_{}", Uuid::new_v4().simple());
        self.ensure_permissions(session_id, audit, &subagent_id, &req)?;

        audit.append(&AuditEvent {
            meta: AuditMetadata::new(
                session_id,
                AuditActor::MainAgent,
                AuditSource::Interactive,
                &self.root,
            ),
            data: AuditEventData::SubagentStarted {
                subagent_id: subagent_id.clone(),
                task: req.task.clone(),
                granted_permissions: req
                    .granted_permissions
                    .iter()
                    .map(|perm| format!("{perm:?}"))
                    .collect(),
                input_summary: truncate(&req.task, 120),
            },
        })?;

        let report = self.build_report(&req.task)?;
        let artifact = self
            .state_dir
            .join("artifacts")
            .join(format!("subagent-report-{}.json", subagent_id));
        if let Some(parent) = artifact.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(&artifact, serde_json::to_string_pretty(&report)?)?;

        audit.append(&AuditEvent {
            meta: AuditMetadata::new(
                session_id,
                AuditActor::Subagent,
                AuditSource::Interactive,
                &self.root,
            ),
            data: AuditEventData::SubagentFinished {
                subagent_id,
                executed_tools: vec!["glob".to_string(), "search".to_string(), "read".to_string()],
                changed_files: Vec::new(),
                report_summary: report.summary.clone(),
                report_ref: artifact.clone(),
            },
        })?;

        Ok(SubagentReport {
            artifacts: vec![artifact],
            ..report
        })
    }

    fn ensure_permissions(
        &self,
        session_id: &str,
        audit: &AuditLog,
        subagent_id: &str,
        req: &SubagentRequest,
    ) -> anyhow::Result<()> {
        let requested = req
            .granted_permissions
            .iter()
            .map(|perm| format!("{perm:?}"))
            .collect::<Vec<_>>();
        audit.append(&AuditEvent {
            meta: AuditMetadata::new(
                session_id,
                AuditActor::MainAgent,
                AuditSource::Interactive,
                &self.root,
            ),
            data: AuditEventData::SubagentPermissionRequested {
                subagent_id: subagent_id.to_string(),
                requested_permissions: requested.clone(),
            },
        })?;

        if req
            .granted_permissions
            .iter()
            .any(|perm| matches!(perm, PermissionCategory::SubagentWrite))
        {
            audit.append(&AuditEvent {
                meta: AuditMetadata::new(
                    session_id,
                    AuditActor::System,
                    AuditSource::Interactive,
                    &self.root,
                ),
                data: AuditEventData::SubagentPermissionResolved {
                    subagent_id: subagent_id.to_string(),
                    allowed: false,
                    granted_permissions: vec!["SubagentWrite".to_string()],
                },
            })?;
            return Err(anyhow!(
                "subagent write permission is not permitted by default"
            ));
        }

        audit.append(&AuditEvent {
            meta: AuditMetadata::new(
                session_id,
                AuditActor::System,
                AuditSource::Interactive,
                &self.root,
            ),
            data: AuditEventData::SubagentPermissionResolved {
                subagent_id: subagent_id.to_string(),
                allowed: true,
                granted_permissions: requested,
            },
        })?;
        Ok(())
    }

    fn build_report(&self, task: &str) -> anyhow::Result<SubagentReport> {
        let tokens = task
            .split_whitespace()
            .map(|token| token.trim_matches(|c: char| !c.is_alphanumeric()))
            .filter(|token| token.len() >= 3)
            .map(|token| token.to_ascii_lowercase())
            .collect::<Vec<_>>();

        let matches = tokens
            .iter()
            .flat_map(|token| search_in_files(&self.root, token).unwrap_or_default())
            .collect::<Vec<_>>();

        let mut referenced = BTreeSet::new();
        for entry in &matches {
            if referenced.len() >= 3 {
                break;
            }
            referenced.insert(entry.path.clone());
        }
        if referenced.is_empty() {
            for path in glob_paths(&self.root, "**/*")? {
                if path.is_file() {
                    referenced.insert(path);
                }
                if referenced.len() >= 3 {
                    break;
                }
            }
        }

        let referenced_files = referenced.into_iter().collect::<Vec<_>>();
        let key_findings = referenced_files
            .iter()
            .take(3)
            .map(|path| {
                let content = read_file(path).unwrap_or_default();
                format!(
                    "{}: {}",
                    path.display(),
                    truncate(&content.replace('\n', " "), 80)
                )
            })
            .collect::<Vec<_>>();

        Ok(SubagentReport {
            summary: truncate(
                &format!(
                    "Subagent inspected {} files for task: {}",
                    referenced_files.len(),
                    task
                ),
                240,
            ),
            key_findings,
            referenced_files,
            recommended_next_action: "Review the referenced files and continue in the main agent"
                .to_string(),
            artifacts: Vec::new(),
        })
    }
}

fn truncate(text: &str, max_chars: usize) -> String {
    text.chars().take(max_chars).collect()
}
