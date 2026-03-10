# Anvil Rust 型ドラフト

## 方針

- `PermissionMode` は CLI/設定で選ぶプリセットであり、最終判定ロジックそのものではない
- 最終判定は `PermissionPolicy` と `ExecutionContext` から導く
- `AuditEvent` は `event_type` を重複保持せず、`AuditEventData` だけを truth source とする
- append-only 監査ログで必要な情報を失わないよう、最低限のメタデータを型に持たせる
- 監査ログは機械検証可能なイベント列を優先し、自由構造 payload を最小化する
- `HardConfirm` は非対話では常に拒否し、自動許可へ落とさない

## `PermissionMode`

```rust
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum PermissionMode {
    Ask,
    AcceptEdits,
    BypassPermissions,
}

impl Default for PermissionMode {
    fn default() -> Self {
        Self::Ask
    }
}
```

## 権限判定補助型

```rust
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum PermissionCategory {
    Read,
    Edit,
    ExecSafe,
    ExecSensitive,
    ExecDangerous,
    SubagentRead,
    SubagentWrite,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PermissionRequirement {
    Allow,
    Ask,
    SoftConfirm,
    HardConfirm,
    Deny,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InteractionMode {
    Interactive,
    NonInteractive,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NonInteractiveBehavior {
    Deny,
    Allow,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PermissionScope {
    Once,
    Session,
    Command,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PermissionResolveDecision {
    Allow,
    Deny,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExecutionContext {
    pub interaction_mode: InteractionMode,
    pub non_interactive_ask: NonInteractiveBehavior,
    pub non_interactive_soft_confirm: NonInteractiveBehavior,
    pub non_interactive_hard_confirm: NonInteractiveBehavior,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct PermissionPolicy {
    pub mode: PermissionMode,
    pub category: PermissionCategory,
}

impl PermissionPolicy {
    pub fn from_mode(mode: PermissionMode, category: PermissionCategory) -> Self {
        Self { mode, category }
    }

    pub fn base_requirement(&self) -> PermissionRequirement {
        use PermissionCategory::*;
        use PermissionMode::*;
        use PermissionRequirement::*;

        match (self.mode, self.category) {
            (Ask, Read) => Allow,
            (Ask, Edit) => Ask,
            (Ask, ExecSafe) => Ask,
            (Ask, ExecSensitive) => Ask,
            (Ask, ExecDangerous) => HardConfirm,
            (Ask, SubagentRead) => Ask,
            (Ask, SubagentWrite) => Ask,

            (AcceptEdits, Read) => Allow,
            (AcceptEdits, Edit) => Allow,
            (AcceptEdits, ExecSafe) => Ask,
            (AcceptEdits, ExecSensitive) => Ask,
            (AcceptEdits, ExecDangerous) => HardConfirm,
            (AcceptEdits, SubagentRead) => Ask,
            (AcceptEdits, SubagentWrite) => Ask,

            (BypassPermissions, Read) => Allow,
            (BypassPermissions, Edit) => Allow,
            (BypassPermissions, ExecSafe) => Allow,
            (BypassPermissions, ExecSensitive) => SoftConfirm,
            (BypassPermissions, ExecDangerous) => HardConfirm,
            (BypassPermissions, SubagentRead) => Allow,
            (BypassPermissions, SubagentWrite) => Ask,
        }
    }

    pub fn effective_requirement(&self, cx: ExecutionContext) -> PermissionRequirement {
        use InteractionMode::*;
        use NonInteractiveBehavior::*;
        use PermissionRequirement::*;

        let base = self.base_requirement();

        match (cx.interaction_mode, base) {
            (NonInteractive, Ask) => match cx.non_interactive_ask {
                Deny => Deny,
                Allow => Allow,
            },
            (NonInteractive, SoftConfirm) => match cx.non_interactive_soft_confirm {
                Deny => Deny,
                Allow => Allow,
            },
            (NonInteractive, HardConfirm) => Deny,
            _ => base,
        }
    }
}
```

## `AuditEvent`

```rust
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;
use std::path::PathBuf;
use uuid::Uuid;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AuditActor {
    User,
    MainAgent,
    Subagent,
    System,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AuditSource {
    Interactive,
    OneShot,
    SlashCommand,
    Replay,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AuditMetadata {
    pub schema_version: u16,
    pub event_id: String,
    pub ts: DateTime<Utc>,
    pub session_id: String,
    pub parent_session_id: Option<String>,
    pub turn_id: Option<String>,
    pub actor: AuditActor,
    pub source: AuditSource,
    pub cwd: Option<PathBuf>,
}

impl AuditMetadata {
    pub fn new(
        session_id: impl Into<String>,
        actor: AuditActor,
        source: AuditSource,
        cwd: PathBuf,
    ) -> Self {
        Self {
            schema_version: 1,
            event_id: format!("evt_{}", Uuid::new_v4().simple()),
            ts: Utc::now(),
            session_id: session_id.into(),
            parent_session_id: None,
            turn_id: None,
            actor,
            source,
            cwd: Some(cwd),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuditEvent {
    pub meta: AuditMetadata,
    pub data: AuditEventData,
}

impl AuditEvent {
    pub fn with_inherited_cwd(mut self) -> Self {
        self.meta.cwd = None;
        self
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum AuditEventData {
    SessionStarted {
        model: String,
        permission_mode: PermissionMode,
    },
    SessionEnded {
        reason: String,
    },
    PermissionRequested {
        request_id: String,
        permission_mode: PermissionMode,
        category: PermissionCategory,
        requirement: PermissionRequirement,
        target: String,
        reason: String,
    },
    PermissionResolved {
        request_id: String,
        decision: PermissionResolveDecision,
        scope: PermissionScope,
        applies_to: String,
        approver_id: Option<String>,
    },
    ToolCallReceived {
        tool_name: String,
        tool_call_id: String,
        raw_args_preview: String,
        parse_status: ParseStatus,
    },
    ToolExecution {
        tool_name: String,
        tool_call_id: String,
        category: Option<PermissionCategory>,
        args: ToolExecutionArgs,
        command_digest: Option<String>,
    },
    ToolResult {
        tool_call_id: String,
        status: ToolResultStatus,
        exit_code: Option<i32>,
        duration_ms: u64,
        output_ref: Option<PathBuf>,
        output_digest: Option<String>,
        changed_files: Vec<ChangedFile>,
    },
    ToolBlocked {
        tool_name: String,
        tool_call_id: Option<String>,
        reason: String,
    },
    SubagentStarted {
        subagent_id: String,
        task: String,
        granted_permissions: Vec<PermissionCategory>,
        input_summary: String,
    },
    SubagentPermissionRequested {
        subagent_id: String,
        request_id: String,
        category: PermissionCategory,
        requirement: PermissionRequirement,
        target: String,
    },
    SubagentPermissionResolved {
        subagent_id: String,
        request_id: String,
        decision: PermissionResolveDecision,
        approver_id: Option<String>,
    },
    SubagentFinished {
        subagent_id: String,
        executed_tools: Vec<String>,
        changed_files: Vec<ChangedFile>,
        report_summary: String,
        report_ref: Option<PathBuf>,
        report_digest: Option<String>,
    },
    MemoryUpdated {
        memory_file: String,
        operation: MemoryOperation,
        summary: String,
    },
    CustomCommandInvoked {
        command_name: String,
        command_source: PathBuf,
        command_schema: String,
        resolved_args: BTreeMap<String, TypedArgValue>,
    },
    Error {
        code: String,
        message: String,
        detail: Option<String>,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ToolResultStatus {
    Ok,
    Error,
    Cancelled,
    TimedOut,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MemoryOperation {
    Add,
    Edit,
    Remove,
    Normalize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ParseStatus {
    Parsed,
    Rejected,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "tool", rename_all = "snake_case")]
pub enum ToolExecutionArgs {
    Read {
        path: PathBuf,
        offset: Option<u64>,
        limit: Option<u64>,
    },
    Write {
        path: PathBuf,
        content_ref: Option<PathBuf>,
    },
    Edit {
        path: PathBuf,
        edit_count: Option<u32>,
    },
    Exec {
        command: String,
        argv: Vec<String>,
        workdir: Option<PathBuf>,
    },
    Search {
        path: Option<PathBuf>,
        query: String,
    },
    Glob {
        pattern: String,
        root: Option<PathBuf>,
    },
    Diff {
        target: Option<String>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChangedFile {
    pub path: PathBuf,
    pub change_kind: FileChangeKind,
    pub detection_method: ChangeDetectionMethod,
    pub before_digest: Option<String>,
    pub after_digest: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FileChangeKind {
    Created,
    Modified,
    Deleted,
    Renamed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ChangeDetectionMethod {
    ToolReported,
    SnapshotDiff,
    GitDiff,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TypedArgValue {
    String(String),
    Integer(i64),
    Float(f64),
    Boolean(bool),
    Path(PathBuf),
    StringList(Vec<String>),
    Object(BTreeMap<String, TypedArgValue>),
}
```

## 補足

- `PermissionMode` は UX 向けプリセットであり、`PermissionPolicy` が実効判定を持つ
- 非対話時の挙動は `ExecutionContext` 側で明示し、仕様との差分を埋め込めるようにする
- ただし `HardConfirm` は非対話で常に拒否し、実行側へ倒さない
- `argv`, `command_digest`, `output_digest`, `ChangedFile` を持たせ、監査粒度を上げる
- `ToolExecutionArgs` と `TypedArgValue` で自由構造 payload を減らす
- `cwd` は毎イベント必須にせず、必要時のみ付与する
- `ChangedFile.detection_method` で変更検出手段を追跡する
- `schema_version` を持たせ、監査ログの後方互換を扱えるようにする
