//! `anvil sessions {list,show,clean}` subcommand implementation.
//!
//! This module is split into a pure-data layer (metadata scanning, validation,
//! list/clean planning) and a thin I/O layer (`run_list` / `run_show` /
//! `run_clean` / `dispatch`) that formats the planned result to stdout and
//! performs deletions. The pure layer is exhaustively unit-tested; the I/O
//! layer is covered by integration tests in `tests/session_cli_tests.rs`.

use std::fs;
use std::io::{self, IsTerminal};
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use crate::cli::{SessionsAction, TmpTestsAction};
use crate::modes::plan_act::ExecutionMode;
use crate::session::compact::is_compact_summary;
use crate::session::discovery::{SessionDirEntry, iter_session_dirs};
use crate::session::store::{ConversationMessage, SessionSnapshot};
use crate::session::tmp_tests;

pub const UNASSIGNED_WORKSPACE: &str = "unassigned";

// ---------------------------------------------------------------------------
// Data model
// ---------------------------------------------------------------------------

/// Display-only summary of a single session. Built from a `SessionDirEntry`
/// via `SessionMeta::from_entry`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionMeta {
    pub id: String,
    pub workspace_key: String,
    pub updated_at: SystemTime,
    pub message_count: usize,
    pub last_tool_name: Option<String>,
    pub mode: ExecutionMode,
}

impl SessionMeta {
    pub fn from_entry(entry: &SessionDirEntry) -> Self {
        Self {
            id: entry.id.clone(),
            workspace_key: entry.snapshot.workspace_key.clone(),
            updated_at: entry.updated_at,
            message_count: entry.snapshot.messages.len(),
            last_tool_name: last_tool_name(&entry.snapshot.messages, 20),
            mode: entry.snapshot.mode_state.mode,
        }
    }
}

fn last_tool_name(messages: &[ConversationMessage], tail_limit: usize) -> Option<String> {
    for message in messages.iter().rev().take(tail_limit) {
        if message.role == "assistant"
            && let Some(tc) = message.tool_calls.first()
        {
            return Some(tc.name.clone());
        }
        if message.role == "tool"
            && let Some(name) = &message.name
        {
            return Some(name.clone());
        }
    }
    None
}

pub fn scan_session_meta(state_root: &Path) -> Vec<SessionMeta> {
    iter_session_dirs(state_root)
        .iter()
        .map(SessionMeta::from_entry)
        .collect()
}

// ---------------------------------------------------------------------------
// Path confinement helpers
// ---------------------------------------------------------------------------

/// Pure-function check: the caller's id is a UUID v7 literal.
///
/// Rejects non-UUID strings and any UUID version != 7. This mirrors the
/// resolver's "directory name must be a UUID" guard while refusing v4/v1 ids
/// that could be synthesized to point at externally-prepared directories.
pub fn validate_session_id_format(id: &str) -> Result<(), String> {
    let Ok(uuid) = uuid::Uuid::parse_str(id) else {
        return Err(format!("session id must be a UUID v7 string: {id}"));
    };
    if uuid.get_version_num() != 7 {
        return Err(format!("session id must be a UUID v7 string: {id}"));
    }
    Ok(())
}

/// Filesystem-layer check: `state_root/sessions/<id>` exists, is a real
/// directory (not a symlink), and `session.json` resolves inside
/// `state_root/sessions/` after canonicalization.
///
/// Callers must run `validate_session_id_format` before this function. The
/// returned path is the validated session directory (not the `session.json`
/// file) so callers can re-use it for `fs::read_dir` / `fs::remove_dir_all`.
pub fn validate_session_dir(state_root: &Path, id: &str) -> Result<PathBuf, String> {
    let session_dir = state_root.join("sessions").join(id);
    let meta =
        fs::symlink_metadata(&session_dir).map_err(|e| format!("session not found ({id}): {e}"))?;
    if meta.file_type().is_symlink() {
        return Err(format!("session id refers to a symlink: {id}"));
    }
    if !meta.is_dir() {
        return Err(format!("session id is not a directory: {id}"));
    }

    let session_json = session_dir.join("session.json");
    let canon_json = session_json
        .canonicalize()
        .map_err(|e| format!("cannot canonicalize session.json for {id}: {e}"))?;
    let canon_sessions_root = state_root
        .join("sessions")
        .canonicalize()
        .map_err(|e| format!("cannot canonicalize state_root/sessions: {e}"))?;
    if !canon_json.starts_with(&canon_sessions_root) {
        return Err(format!("session path escapes state_root/sessions: {id}"));
    }
    Ok(session_dir)
}

/// Convenience wrapper used by `--resume <ID>` and `sessions show <ID>`.
pub fn validate_explicit_session_id(state_root: &Path, id: &str) -> Result<PathBuf, String> {
    validate_session_id_format(id)?;
    validate_session_dir(state_root, id)
}

// ---------------------------------------------------------------------------
// List planning (pure)
// ---------------------------------------------------------------------------

/// Where a row came from, used to tag output when `--all` includes foreign
/// workspaces.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WorkspaceScope {
    Current,
    Other,
    /// `workspace_key` was empty in the snapshot (very old sessions).
    Unassigned,
}

/// One list-view row; human and `--json` formatters both read this.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ListRow {
    pub id: String,
    pub workspace_key: String,
    pub scope: WorkspaceScope,
    pub updated_at: SystemTime,
    pub message_count: usize,
    pub last_tool_name: Option<String>,
    pub mode: ExecutionMode,
}

pub fn compute_list_rows(metas: &[SessionMeta], current_ws: &str, all: bool) -> Vec<ListRow> {
    let mut rows: Vec<ListRow> = metas
        .iter()
        .filter_map(|m| {
            let scope = if m.workspace_key.is_empty() {
                WorkspaceScope::Unassigned
            } else if m.workspace_key == current_ws {
                WorkspaceScope::Current
            } else {
                WorkspaceScope::Other
            };
            if !all && scope != WorkspaceScope::Current {
                return None;
            }
            Some(ListRow {
                id: m.id.clone(),
                workspace_key: m.workspace_key.clone(),
                scope,
                updated_at: m.updated_at,
                message_count: m.message_count,
                last_tool_name: m.last_tool_name.clone(),
                mode: m.mode,
            })
        })
        .collect();

    // Sort newest first. For equal mtimes, break ties by UUID descending so
    // output is deterministic across platforms (mtime precision varies).
    rows.sort_by(|a, b| {
        b.updated_at
            .cmp(&a.updated_at)
            .then_with(|| b.id.cmp(&a.id))
    });
    rows
}

// ---------------------------------------------------------------------------
// Clean planning (pure)
// ---------------------------------------------------------------------------

/// Normalized `sessions clean` arguments used by `plan_clean`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CleanArgs {
    pub id: Option<String>,
    pub older_than_days: Option<u32>,
    pub keep: Option<usize>,
    pub all: bool,
}

/// The set of sessions that `sessions clean` intends to delete, plus the one
/// that is being explicitly protected (the resolver's current winner).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CleanPlan {
    pub to_delete: Vec<SessionMeta>,
    pub protected_id: Option<String>,
    pub scope_hint: CleanScopeHint,
}

/// Describes which subset of sessions the plan considered. Used by the I/O
/// layer to render dry-run output.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CleanScopeHint {
    CurrentWorkspace,
    AllWorkspaces,
    SingleId,
}

pub fn plan_clean(
    metas: &[SessionMeta],
    current_ws: &str,
    args: &CleanArgs,
    protected_id: Option<&str>,
    now: SystemTime,
) -> Result<CleanPlan, String> {
    // Single-id path: caller has already run validate_session_id_format on
    // args.id, we only need to check workspace and protection.
    if let Some(id) = &args.id {
        if args.older_than_days.is_some() || args.keep.is_some() {
            return Err(
                "--older-than and --keep cannot be combined with an explicit session id"
                    .to_string(),
            );
        }
        let Some(meta) = metas.iter().find(|m| &m.id == id) else {
            return Err(format!("session not found: {id}"));
        };
        if !args.all && meta.workspace_key != current_ws {
            return Err(format!(
                "session {id} belongs to a different workspace; pass --all to clean it"
            ));
        }
        if protected_id == Some(id.as_str()) {
            return Err(format!(
                "refusing to delete the session currently selected by resolve_session_id: {id}"
            ));
        }
        return Ok(CleanPlan {
            to_delete: vec![meta.clone()],
            protected_id: protected_id.map(str::to_string),
            scope_hint: CleanScopeHint::SingleId,
        });
    }

    // Bulk path: workspace filter first.
    let scope_hint = if args.all {
        CleanScopeHint::AllWorkspaces
    } else {
        CleanScopeHint::CurrentWorkspace
    };
    let mut pool: Vec<SessionMeta> = metas
        .iter()
        .filter(|m| args.all || m.workspace_key == current_ws)
        .cloned()
        .collect();

    // Newest first, tie-break by id descending for determinism.
    pool.sort_by(|a, b| {
        b.updated_at
            .cmp(&a.updated_at)
            .then_with(|| b.id.cmp(&a.id))
    });

    let mut to_delete: Vec<SessionMeta> = Vec::new();

    if let Some(days) = args.older_than_days {
        let cutoff = now
            .checked_sub(std::time::Duration::from_secs(days as u64 * 24 * 60 * 60))
            .unwrap_or(SystemTime::UNIX_EPOCH);
        to_delete.extend(pool.iter().filter(|m| m.updated_at < cutoff).cloned());
    }

    if let Some(keep) = args.keep {
        // Pool is already newest-first; anything past index `keep` is dropped.
        to_delete.extend(pool.iter().skip(keep).cloned());
    }

    if args.older_than_days.is_none() && args.keep.is_none() {
        return Err(
            "sessions clean requires --older-than <DAYS>, --keep <N>, or an explicit id"
                .to_string(),
        );
    }

    // Deduplicate by id (in case --older-than and --keep overlap).
    to_delete.sort_by(|a, b| a.id.cmp(&b.id));
    to_delete.dedup_by(|a, b| a.id == b.id);

    // Never remove the currently-resolved session.
    if let Some(pid) = protected_id {
        to_delete.retain(|m| m.id != pid);
    }

    // Re-sort deletion order newest-first for human-readable dry-run output.
    to_delete.sort_by(|a, b| {
        b.updated_at
            .cmp(&a.updated_at)
            .then_with(|| b.id.cmp(&a.id))
    });

    Ok(CleanPlan {
        to_delete,
        protected_id: protected_id.map(str::to_string),
        scope_hint,
    })
}

// ---------------------------------------------------------------------------
// Show helpers (pure)
// ---------------------------------------------------------------------------

/// Short, non-secret preview fields derived from a `SessionSnapshot`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ShowView {
    pub id: String,
    pub workspace_key: String,
    pub active_root: Option<PathBuf>,
    pub message_count: usize,
    pub first_user_preview: Option<String>,
    pub last_assistant_or_tool_preview: Option<String>,
    pub checkpoint_count: usize,
    pub mode: ExecutionMode,
    pub active_plan_path: Option<PathBuf>,
}

const SHOW_PREVIEW_CHARS: usize = 200;

impl ShowView {
    pub fn from_snapshot(snap: &SessionSnapshot) -> Self {
        Self {
            id: snap.id.clone(),
            workspace_key: snap.workspace_key.clone(),
            active_root: snap.active_root.clone(),
            message_count: snap.messages.len(),
            first_user_preview: first_user_preview(&snap.messages),
            last_assistant_or_tool_preview: last_assistant_or_tool_preview(&snap.messages),
            checkpoint_count: snap.checkpoints.len(),
            mode: snap.mode_state.mode,
            active_plan_path: snap.mode_state.active_plan_path.clone(),
        }
    }
}

fn first_user_preview(messages: &[ConversationMessage]) -> Option<String> {
    messages
        .iter()
        .find(|m| m.role == "user" && !is_compact_summary(m))
        .map(|m| preview(&m.content))
}

fn last_assistant_or_tool_preview(messages: &[ConversationMessage]) -> Option<String> {
    for m in messages.iter().rev() {
        if m.role == "assistant" && !m.content.is_empty() {
            return Some(preview(&m.content));
        }
        if m.role == "assistant"
            && let Some(tc) = m.tool_calls.first()
        {
            return Some(format!("tool_call:{}", tc.name));
        }
        if m.role == "tool" {
            let label = m.name.as_deref().unwrap_or("tool");
            return Some(format!("{label}:{}", preview(&m.content)));
        }
    }
    None
}

fn preview(s: &str) -> String {
    let collapsed: String = s.chars().filter(|c| *c != '\n').collect();
    collapsed.chars().take(SHOW_PREVIEW_CHARS).collect()
}

// ---------------------------------------------------------------------------
// Dispatch and I/O layer
// ---------------------------------------------------------------------------

/// `dispatch` for `anvil sessions ...`.
///
/// `workspace_root` is the canonicalized cwd-or-`--cwd` path that the caller
/// resolved before constructing `state_root` / `current_ws`. tmp-tests
/// `promote` writes its destination relative to this path, never
/// `std::env::current_dir()` (CB-003: the two could diverge when `--cwd`
/// was passed and anvil is invoked from a sibling directory).
pub fn dispatch(
    state_root: &Path,
    current_ws: &str,
    workspace_root: &Path,
    action: SessionsAction,
) -> Result<(), String> {
    match action {
        SessionsAction::List { all, json } => run_list(state_root, current_ws, all, json),
        SessionsAction::Show { id, all, json } => run_show(state_root, current_ws, &id, all, json),
        SessionsAction::Clean {
            id,
            older_than_days,
            keep,
            all,
            force,
        } => {
            if let Some(ref explicit) = id {
                validate_session_id_format(explicit)?;
            }
            let args = CleanArgs {
                id,
                older_than_days,
                keep,
                all,
            };
            run_clean(state_root, current_ws, args, force)
        }
        SessionsAction::TmpTests { action } => match action {
            TmpTestsAction::Promote {
                session,
                test_id,
                force,
                yes,
            } => run_tmp_tests_promote(
                state_root,
                current_ws,
                workspace_root,
                &session,
                &test_id,
                force,
                yes,
            ),
            TmpTestsAction::Discard { session, test_id } => {
                run_tmp_tests_discard(state_root, current_ws, &session, &test_id)
            }
            TmpTestsAction::List { session } => {
                run_tmp_tests_list(state_root, current_ws, &session)
            }
        },
    }
}

pub fn run_list(state_root: &Path, current_ws: &str, all: bool, json: bool) -> Result<(), String> {
    let metas = scan_session_meta(state_root);
    let rows = compute_list_rows(&metas, current_ws, all);
    if json {
        println!("{}", render_list_json(&rows));
    } else {
        print_list_human(&rows, all);
    }
    Ok(())
}

pub fn run_show(
    state_root: &Path,
    current_ws: &str,
    id: &str,
    all: bool,
    json: bool,
) -> Result<(), String> {
    let session_dir = validate_explicit_session_id(state_root, id)?;
    let session_json = session_dir.join("session.json");
    let data = fs::read_to_string(&session_json)
        .map_err(|e| format!("failed to read session.json for {id}: {e}"))?;
    let snap: SessionSnapshot = serde_json::from_str(&data)
        .map_err(|e| format!("failed to parse session.json for {id}: {e}"))?;

    if !all && !snap.workspace_key.is_empty() && snap.workspace_key != current_ws {
        return Err(format!(
            "session {id} belongs to a different workspace; pass --all to view it"
        ));
    }

    let view = ShowView::from_snapshot(&snap);
    if json {
        println!("{}", render_show_json(&view));
    } else {
        print_show_human(&view);
    }
    Ok(())
}

pub fn run_clean(
    state_root: &Path,
    current_ws: &str,
    args: CleanArgs,
    force: bool,
) -> Result<(), String> {
    let metas = scan_session_meta(state_root);
    let protected = crate::resolve_session_id(state_root, current_ws, false);
    let plan = plan_clean(
        &metas,
        current_ws,
        &args,
        Some(&protected),
        SystemTime::now(),
    )?;
    execute_clean_plan(state_root, &plan, force)
}

pub fn execute_clean_plan(state_root: &Path, plan: &CleanPlan, force: bool) -> Result<(), String> {
    if plan.to_delete.is_empty() {
        println!("no sessions match the cleanup criteria");
        if let Some(pid) = &plan.protected_id {
            println!("(protected current session: {pid})");
        }
        return Ok(());
    }

    if !force {
        println!("dry-run: would delete {} session(s):", plan.to_delete.len());
        for m in &plan.to_delete {
            println!(
                "  - {id}  ws={ws}  messages={n}  mode={mode:?}",
                id = m.id,
                ws = if m.workspace_key.is_empty() {
                    UNASSIGNED_WORKSPACE
                } else {
                    &m.workspace_key
                },
                n = m.message_count,
                mode = m.mode
            );
        }
        if let Some(pid) = &plan.protected_id {
            println!("(protected current session: {pid})");
        }
        println!("pass --force to actually delete.");
        return Ok(());
    }

    let mut deleted = 0usize;
    for m in &plan.to_delete {
        // TOCTOU guard: re-validate filesystem state right before rm.
        let path = validate_session_dir(state_root, &m.id)?;
        fs::remove_dir_all(&path).map_err(|e| format!("failed to remove {}: {e}", m.id))?;
        println!("deleted {}", m.id);
        deleted += 1;
    }
    println!("removed {deleted} session(s)");
    Ok(())
}

// ---------------------------------------------------------------------------
// TmpTests handlers (Issue #458)
// ---------------------------------------------------------------------------

/// Read `state_root/sessions/<id>/session.json` and assert it belongs to
/// `current_ws`. Used by tmp-tests handlers to refuse cross-workspace
/// promote / discard / list.
fn require_session_in_workspace(
    state_root: &Path,
    session_id: &str,
    current_ws: &str,
) -> Result<PathBuf, String> {
    let session_dir = validate_explicit_session_id(state_root, session_id)?;
    let session_json = session_dir.join("session.json");
    let data = fs::read_to_string(&session_json)
        .map_err(|err| format!("failed to read session.json for {session_id}: {err}"))?;
    let snap: SessionSnapshot = serde_json::from_str(&data)
        .map_err(|err| format!("failed to parse session.json for {session_id}: {err}"))?;
    if !snap.workspace_key.is_empty() && snap.workspace_key != current_ws {
        return Err(format!(
            "session {session_id} belongs to a different workspace; tmp-tests operations \
             are confined to the current workspace"
        ));
    }
    Ok(session_dir)
}

fn tmp_tests_root_for(session_dir: &Path) -> PathBuf {
    session_dir.join("tmp-tests")
}

/// CLI handler for `anvil sessions tmp-tests promote`.
///
/// Approval policy (CB-002 fix):
///
/// * `--yes` is the explicit approval signal for the promote operation
///   itself. It is wired to `auto_approve` in `promote_tmp_test`.
/// * `interactive_approval` is `io::stdin().is_terminal()` so a human
///   running anvil from a TTY does not need `--yes`.
/// * Without `--yes` and without a TTY, promote is rejected. This means a
///   non-interactive cron / CI job that has not opted in cannot silently
///   write to the workspace.
/// * `--force` is **separate** from approval: it is the override for the
///   "destination already exists" collision case (and only allows
///   overwriting a regular file — symlinks are still rejected).
///
/// `workspace_root` is the canonicalized current workspace path the parent
/// dispatcher resolved (CB-003 fix: never `std::env::current_dir()` here).
pub fn run_tmp_tests_promote(
    state_root: &Path,
    current_ws: &str,
    workspace_root: &Path,
    session_id: &str,
    test_id: &str,
    force: bool,
    yes: bool,
) -> Result<(), String> {
    let session_dir = require_session_in_workspace(state_root, session_id, current_ws)?;
    let tmp_tests_root = tmp_tests_root_for(&session_dir);

    let auto_approve = yes;
    let interactive_approval = io::stdin().is_terminal();

    let dst = tmp_tests::promote_tmp_test(
        workspace_root,
        &tmp_tests_root,
        test_id,
        force,
        auto_approve,
        interactive_approval,
    )
    .map_err(|err| {
        eprintln!("error: {err}");
        err
    })?;
    println!("promoted {test_id} -> {}", dst.display());
    Ok(())
}

pub fn run_tmp_tests_discard(
    state_root: &Path,
    current_ws: &str,
    session_id: &str,
    test_id: &str,
) -> Result<(), String> {
    let session_dir = require_session_in_workspace(state_root, session_id, current_ws)?;
    let tmp_tests_root = tmp_tests_root_for(&session_dir);
    tmp_tests::discard_tmp_test(&tmp_tests_root, test_id).map_err(|err| {
        eprintln!("error: {err}");
        err
    })?;
    println!("discarded {test_id}");
    Ok(())
}

pub fn run_tmp_tests_list(
    state_root: &Path,
    current_ws: &str,
    session_id: &str,
) -> Result<(), String> {
    let session_dir = require_session_in_workspace(state_root, session_id, current_ws)?;
    let tmp_tests_root = tmp_tests_root_for(&session_dir);
    let tests = tmp_tests::list_tmp_tests(&tmp_tests_root).map_err(|err| {
        eprintln!("error: {err}");
        err
    })?;
    if tests.is_empty() {
        println!("no tmp-tests found");
        return Ok(());
    }
    println!(
        "ID                                              STATUS     CREATED_AT  RELATIVE_PATH"
    );
    for t in &tests {
        let status = match t.status {
            tmp_tests::TmpTestStatus::Draft => "draft",
            tmp_tests::TmpTestStatus::Promoted => "promoted",
            tmp_tests::TmpTestStatus::Discarded => "discarded",
        };
        println!(
            "{id:48} {status:<10} {ts:<10} {rel}",
            id = t.id,
            status = status,
            ts = t.created_at,
            rel = t.relative_path.display(),
        );
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Rendering helpers (kept inline; they're straightforward enough that a
// separate module would just add indirection).
// ---------------------------------------------------------------------------

fn print_list_human(rows: &[ListRow], include_ws: bool) {
    if rows.is_empty() {
        println!("no sessions found");
        return;
    }
    if include_ws {
        println!(
            "ID                                    UPDATED_AT       MESSAGES  LAST_TOOL           MODE     WORKSPACE"
        );
        for r in rows {
            println!(
                "{id:38} {ts:16} {n:>8}  {tool:<18}  {mode:<8} {ws}",
                id = r.id,
                ts = fmt_mtime(r.updated_at),
                n = r.message_count,
                tool = r.last_tool_name.as_deref().unwrap_or("-"),
                mode = format!("{:?}", r.mode),
                ws = workspace_label(&r.workspace_key, r.scope),
            );
        }
    } else {
        println!(
            "ID                                    UPDATED_AT       MESSAGES  LAST_TOOL           MODE"
        );
        for r in rows {
            println!(
                "{id:38} {ts:16} {n:>8}  {tool:<18}  {mode:?}",
                id = r.id,
                ts = fmt_mtime(r.updated_at),
                n = r.message_count,
                tool = r.last_tool_name.as_deref().unwrap_or("-"),
                mode = r.mode,
            );
        }
    }
}

fn workspace_label(key: &str, scope: WorkspaceScope) -> String {
    match scope {
        WorkspaceScope::Unassigned => UNASSIGNED_WORKSPACE.to_string(),
        WorkspaceScope::Current | WorkspaceScope::Other => key.to_string(),
    }
}

fn fmt_mtime(t: SystemTime) -> String {
    match t.duration_since(SystemTime::UNIX_EPOCH) {
        Ok(dur) => dur.as_secs().to_string(),
        Err(_) => "?".to_string(),
    }
}

fn print_show_human(v: &ShowView) {
    println!("id:             {}", v.id);
    println!(
        "workspace_key:  {}",
        if v.workspace_key.is_empty() {
            UNASSIGNED_WORKSPACE
        } else {
            &v.workspace_key
        }
    );
    println!(
        "active_root:    {}",
        v.active_root
            .as_ref()
            .map(|p| p.display().to_string())
            .unwrap_or_else(|| "-".to_string())
    );
    println!("messages:       {}", v.message_count);
    println!(
        "first_user:     {}",
        v.first_user_preview.as_deref().unwrap_or("-")
    );
    println!(
        "last_turn:      {}",
        v.last_assistant_or_tool_preview.as_deref().unwrap_or("-")
    );
    println!("checkpoints:    {}", v.checkpoint_count);
    println!("mode:           {:?}", v.mode);
    println!(
        "active_plan:    {}",
        v.active_plan_path
            .as_ref()
            .map(|p| p.display().to_string())
            .unwrap_or_else(|| "-".to_string())
    );
}

fn render_list_json(rows: &[ListRow]) -> String {
    let arr: Vec<serde_json::Value> = rows
        .iter()
        .map(|r| {
            serde_json::json!({
                "id": r.id,
                "workspace_key": r.workspace_key,
                "workspace_scope": match r.scope {
                    WorkspaceScope::Current => "current",
                    WorkspaceScope::Other => "other",
                    WorkspaceScope::Unassigned => "unassigned",
                },
                "updated_at_unix": match r.updated_at.duration_since(SystemTime::UNIX_EPOCH) {
                    Ok(d) => d.as_secs(),
                    Err(_) => 0,
                },
                "messages": r.message_count,
                "last_tool": r.last_tool_name,
                "mode": format!("{:?}", r.mode),
            })
        })
        .collect();
    serde_json::to_string_pretty(&serde_json::Value::Array(arr)).unwrap_or_else(|_| "[]".into())
}

fn render_show_json(v: &ShowView) -> String {
    let value = serde_json::json!({
        "id": v.id,
        "workspace_key": v.workspace_key,
        "active_root": v.active_root.as_ref().map(|p| p.display().to_string()),
        "messages": v.message_count,
        "first_user_preview": v.first_user_preview,
        "last_turn_preview": v.last_assistant_or_tool_preview,
        "checkpoints": v.checkpoint_count,
        "mode": format!("{:?}", v.mode),
        "active_plan_path": v.active_plan_path.as_ref().map(|p| p.display().to_string()),
    });
    serde_json::to_string_pretty(&value).unwrap_or_else(|_| "{}".into())
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::modes::plan_act::ModeState;
    use std::time::Duration;
    use tempfile::TempDir;

    fn meta(id: &str, ws: &str, age_secs: u64, now: SystemTime, messages: usize) -> SessionMeta {
        SessionMeta {
            id: id.to_string(),
            workspace_key: ws.to_string(),
            updated_at: now
                .checked_sub(Duration::from_secs(age_secs))
                .unwrap_or(SystemTime::UNIX_EPOCH),
            message_count: messages,
            last_tool_name: None,
            mode: ExecutionMode::Act,
        }
    }

    fn v7() -> String {
        uuid::Uuid::now_v7().to_string()
    }

    #[test]
    fn validate_format_accepts_v7() {
        assert!(validate_session_id_format(&v7()).is_ok());
    }

    #[test]
    fn validate_format_rejects_v4() {
        // Hardcoded UUID v4 literal (version nibble 4). The uuid crate's
        // v4 feature is not enabled in this workspace, so we cannot call
        // `Uuid::new_v4()` — a literal is equivalent for this assertion.
        let v4 = "0190f3c0-1111-4a00-8000-000000000000";
        assert!(validate_session_id_format(v4).is_err());
    }

    #[test]
    fn validate_format_rejects_junk() {
        assert!(validate_session_id_format("").is_err());
        assert!(validate_session_id_format("not-a-uuid").is_err());
        assert!(validate_session_id_format("../evil").is_err());
    }

    #[test]
    fn validate_dir_rejects_missing() {
        let tmp = TempDir::new().unwrap();
        let id = v7();
        assert!(validate_session_dir(tmp.path(), &id).is_err());
    }

    #[cfg(unix)]
    #[test]
    fn validate_dir_rejects_symlink() {
        let tmp = TempDir::new().unwrap();
        let sessions = tmp.path().join("sessions");
        std::fs::create_dir_all(&sessions).unwrap();

        let real_id = v7();
        let real_dir = sessions.join(&real_id);
        std::fs::create_dir_all(&real_dir).unwrap();
        std::fs::write(real_dir.join("session.json"), "{}").unwrap();

        let link_id = v7();
        std::os::unix::fs::symlink(&real_dir, sessions.join(&link_id)).unwrap();

        let err = validate_session_dir(tmp.path(), &link_id).unwrap_err();
        assert!(err.contains("symlink"), "unexpected error: {err}");
    }

    #[test]
    fn compute_list_filters_current_workspace() {
        let now = SystemTime::now();
        let metas = vec![
            meta("id-a", "ws-x", 100, now, 3),
            meta("id-b", "ws-y", 50, now, 2),
        ];
        let rows = compute_list_rows(&metas, "ws-x", false);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].id, "id-a");
    }

    #[test]
    fn compute_list_all_includes_unassigned() {
        let now = SystemTime::now();
        let metas = vec![
            meta("id-a", "ws-x", 100, now, 3),
            meta("id-b", "", 50, now, 2),
        ];
        let rows = compute_list_rows(&metas, "ws-x", true);
        assert_eq!(rows.len(), 2);
        let unassigned = rows
            .iter()
            .find(|r| r.scope == WorkspaceScope::Unassigned)
            .unwrap();
        assert_eq!(unassigned.id, "id-b");
    }

    #[test]
    fn compute_list_sort_newest_first_tiebreak_uuid_desc() {
        let now = SystemTime::now();
        // equal mtime -> uuid desc
        let metas = vec![
            meta("aaaa", "ws-x", 10, now, 1),
            meta("bbbb", "ws-x", 10, now, 1),
        ];
        let rows = compute_list_rows(&metas, "ws-x", false);
        assert_eq!(rows[0].id, "bbbb");
        assert_eq!(rows[1].id, "aaaa");
    }

    #[test]
    fn plan_clean_rejects_bulk_without_criteria() {
        let now = SystemTime::now();
        let metas = vec![meta("id-a", "ws-x", 10, now, 1)];
        let args = CleanArgs::default();
        assert!(plan_clean(&metas, "ws-x", &args, None, now).is_err());
    }

    #[test]
    fn plan_clean_older_than_selects_stale() {
        let now = SystemTime::now();
        let metas = vec![
            meta("young", "ws-x", 60, now, 1),              // 1 min old
            meta("old", "ws-x", 60 * 60 * 24 * 40, now, 1), // 40 days
        ];
        let args = CleanArgs {
            older_than_days: Some(30),
            ..Default::default()
        };
        let plan = plan_clean(&metas, "ws-x", &args, None, now).unwrap();
        assert_eq!(plan.to_delete.len(), 1);
        assert_eq!(plan.to_delete[0].id, "old");
    }

    #[test]
    fn plan_clean_keep_n_retains_newest() {
        let now = SystemTime::now();
        let metas = vec![
            meta("new1", "ws-x", 10, now, 1),
            meta("new2", "ws-x", 20, now, 1),
            meta("old1", "ws-x", 1000, now, 1),
            meta("old2", "ws-x", 2000, now, 1),
        ];
        let args = CleanArgs {
            keep: Some(2),
            ..Default::default()
        };
        let plan = plan_clean(&metas, "ws-x", &args, None, now).unwrap();
        let ids: Vec<&str> = plan.to_delete.iter().map(|m| m.id.as_str()).collect();
        assert_eq!(ids, vec!["old1", "old2"]);
    }

    #[test]
    fn plan_clean_excludes_protected() {
        let now = SystemTime::now();
        let metas = vec![
            meta("latest", "ws-x", 10, now, 1),
            meta("old", "ws-x", 60 * 60 * 24 * 40, now, 1),
        ];
        let args = CleanArgs {
            keep: Some(0),
            ..Default::default()
        };
        let plan = plan_clean(&metas, "ws-x", &args, Some("latest"), now).unwrap();
        assert_eq!(plan.to_delete.len(), 1);
        assert_eq!(plan.to_delete[0].id, "old");
    }

    #[test]
    fn plan_clean_single_id_rejects_cross_workspace() {
        let now = SystemTime::now();
        let metas = vec![meta("id-a", "ws-other", 10, now, 1)];
        let args = CleanArgs {
            id: Some("id-a".to_string()),
            all: false,
            ..Default::default()
        };
        assert!(plan_clean(&metas, "ws-x", &args, None, now).is_err());
    }

    #[test]
    fn plan_clean_single_id_requires_no_combos() {
        let now = SystemTime::now();
        let metas = vec![meta("id-a", "ws-x", 10, now, 1)];
        let args = CleanArgs {
            id: Some("id-a".to_string()),
            older_than_days: Some(1),
            ..Default::default()
        };
        assert!(plan_clean(&metas, "ws-x", &args, None, now).is_err());
    }

    #[test]
    fn plan_clean_single_id_all_allows_cross_workspace() {
        let now = SystemTime::now();
        let metas = vec![meta("id-a", "ws-other", 10, now, 1)];
        let args = CleanArgs {
            id: Some("id-a".to_string()),
            all: true,
            ..Default::default()
        };
        let plan = plan_clean(&metas, "ws-x", &args, None, now).unwrap();
        assert_eq!(plan.to_delete.len(), 1);
    }

    #[test]
    fn plan_clean_refuses_to_delete_protected_single_id() {
        let now = SystemTime::now();
        let metas = vec![meta("id-a", "ws-x", 10, now, 1)];
        let args = CleanArgs {
            id: Some("id-a".to_string()),
            ..Default::default()
        };
        assert!(plan_clean(&metas, "ws-x", &args, Some("id-a"), now).is_err());
    }

    #[test]
    fn show_view_builds_from_snapshot() {
        let snap = SessionSnapshot {
            id: "sid".into(),
            workspace_key: "ws-x".into(),
            active_root: Some(PathBuf::from("/tmp/work")),
            mode_state: ModeState {
                mode: ExecutionMode::Plan,
                active_plan_path: Some(PathBuf::from("/tmp/work/.anvil/plan.md")),
                task_profile: crate::modes::plan_act::TaskProfile::Generic,
                work_mode: crate::modes::plan_act::WorkMode::Auto,
                plan_stage: crate::modes::plan_act::PlanStage::Stage1,
            },
            messages: vec![
                ConversationMessage::system("sys".into()),
                ConversationMessage::user("hello world\nextra".into()),
                ConversationMessage::assistant("hi".into(), vec![]),
            ],
            checkpoints: vec!["cp1".into()],
            native_tools_disabled: false,
            working_memory: Default::default(),
            last_feedback: None,
            eligible_feedback_recorded_this_turn: false,
            last_anvil_score: None,
            unsafe_blocks_this_turn: 0,
            consecutive_no_progress_turns: 0,
            repo_edit_succeeded_this_turn: false,
            touched_files_at_turn_start: Vec::new(),
        };
        let v = ShowView::from_snapshot(&snap);
        assert_eq!(v.id, "sid");
        assert_eq!(v.workspace_key, "ws-x");
        assert_eq!(v.message_count, 3);
        assert_eq!(v.first_user_preview.as_deref(), Some("hello worldextra"));
        assert_eq!(v.checkpoint_count, 1);
        assert_eq!(v.mode, ExecutionMode::Plan);
    }

    // -- Issue #458: tmp-tests handler workspace confinement ----------------

    fn write_session_json(state_root: &Path, session_id: &str, ws: &str) {
        let session_dir = state_root.join("sessions").join(session_id);
        std::fs::create_dir_all(&session_dir).unwrap();
        let snap = SessionSnapshot {
            id: session_id.to_string(),
            workspace_key: ws.to_string(),
            ..SessionSnapshot::default()
        };
        std::fs::write(
            session_dir.join("session.json"),
            serde_json::to_string_pretty(&snap).unwrap(),
        )
        .unwrap();
    }

    #[test]
    fn run_tmp_tests_list_rejects_invalid_uuid() {
        let tmp = TempDir::new().unwrap();
        let err = run_tmp_tests_list(tmp.path(), "ws-x", "not-a-uuid").unwrap_err();
        assert!(err.contains("UUID"), "got: {err}");
    }

    #[test]
    fn run_tmp_tests_list_rejects_missing_session() {
        let tmp = TempDir::new().unwrap();
        let id = uuid::Uuid::now_v7().to_string();
        let err = run_tmp_tests_list(tmp.path(), "ws-x", &id).unwrap_err();
        assert!(
            err.contains("not found") || err.contains("session"),
            "got: {err}"
        );
    }

    #[test]
    fn run_tmp_tests_list_rejects_cross_workspace() {
        let tmp = TempDir::new().unwrap();
        let id = uuid::Uuid::now_v7().to_string();
        write_session_json(tmp.path(), &id, "ws-other");
        let err = run_tmp_tests_list(tmp.path(), "ws-x", &id).unwrap_err();
        assert!(err.contains("different workspace"), "got: {err}");
    }

    #[test]
    fn run_tmp_tests_discard_rejects_cross_workspace() {
        let tmp = TempDir::new().unwrap();
        let id = uuid::Uuid::now_v7().to_string();
        write_session_json(tmp.path(), &id, "ws-other");
        let err =
            run_tmp_tests_discard(tmp.path(), "ws-x", &id, "tmp_aaaaaaaaaaaaaaaa").unwrap_err();
        assert!(err.contains("different workspace"), "got: {err}");
    }

    #[test]
    fn run_tmp_tests_promote_rejects_cross_workspace() {
        let tmp = TempDir::new().unwrap();
        let id = uuid::Uuid::now_v7().to_string();
        write_session_json(tmp.path(), &id, "ws-other");
        let workspace = tmp.path().join("workspace");
        std::fs::create_dir_all(&workspace).unwrap();
        let err = run_tmp_tests_promote(
            tmp.path(),
            "ws-x",
            &workspace,
            &id,
            "tmp_aaaaaaaaaaaaaaaa",
            false,
            true,
        )
        .unwrap_err();
        assert!(err.contains("different workspace"), "got: {err}");
    }

    #[test]
    fn run_tmp_tests_list_returns_ok_for_session_in_current_workspace() {
        let tmp = TempDir::new().unwrap();
        let id = uuid::Uuid::now_v7().to_string();
        write_session_json(tmp.path(), &id, "ws-x");
        // Ok with empty output (no tmp-tests yet).
        run_tmp_tests_list(tmp.path(), "ws-x", &id).unwrap();
    }

    // -- Issue #458 / Codex CB-002: non-interactive --yes guard ----------------
    // CB-002 regression: the CLI must reject promote when stdin is not a TTY
    // and `--yes` was not passed, because the `auto_approve || interactive_approval`
    // gate inside promote_tmp_test would otherwise accept any caller.
    //
    // We can verify the *underlying* gate directly by calling
    // `promote_tmp_test` with auto_approve=false and interactive_approval=false
    // — that is exactly what the CLI passes when `cargo test` runs (no TTY)
    // without `--yes`.
    #[test]
    fn promote_rejects_non_interactive_without_yes_at_lifecycle_layer() {
        let tmp = TempDir::new().unwrap();
        let workspace = tmp.path().join("workspace");
        std::fs::create_dir_all(&workspace).unwrap();
        let tmp_tests_root = tmp.path().join("tmp-tests");
        let tt =
            tmp_tests::create_generated_test(&tmp_tests_root, "src/test_x.rs", b"body").unwrap();
        let err = tmp_tests::promote_tmp_test(
            &workspace,
            &tmp_tests_root,
            &tt.id,
            false,
            false, // auto_approve=false (no --yes)
            false, // interactive_approval=false (no TTY)
        )
        .unwrap_err();
        assert!(
            err.contains("approval") || err.contains("--yes"),
            "got: {err}"
        );
        assert!(!workspace.join("src/test_x.rs").exists());
    }

    // CB-003 regression: run_tmp_tests_promote must use the `workspace_root`
    // arg, not std::env::current_dir(). We verify by promoting into a
    // workspace path that is unrelated to the test process's cwd and
    // confirming the file lands inside that path.
    #[test]
    fn run_tmp_tests_promote_uses_explicit_workspace_root_not_cwd() {
        let tmp = TempDir::new().unwrap();
        let id = uuid::Uuid::now_v7().to_string();
        write_session_json(tmp.path(), &id, "ws-x");
        // Build a tmp-test under the session's tmp-tests root.
        let session_dir = tmp.path().join("sessions").join(&id);
        let tmp_tests_root = session_dir.join("tmp-tests");
        let tt =
            tmp_tests::create_generated_test(&tmp_tests_root, "promoted.rs", b"hello").unwrap();
        // Use a workspace_root that is NOT the process cwd.
        let workspace = tmp.path().join("explicit-workspace");
        std::fs::create_dir_all(&workspace).unwrap();

        // auto_approve=true via `--yes` so the gate passes deterministically.
        run_tmp_tests_promote(
            tmp.path(),
            "ws-x",
            &workspace,
            &id,
            &tt.id,
            false,
            true, // --yes
        )
        .unwrap();

        // Body landed under the explicit workspace, not the process cwd.
        let landed = workspace.join("promoted.rs");
        assert!(
            landed.exists(),
            "promote must use workspace_root arg, not env::current_dir(): {landed:?}"
        );
    }
}
