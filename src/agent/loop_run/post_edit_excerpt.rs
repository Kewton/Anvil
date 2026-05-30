//! Issue #636 bounded post-edit excerpt reader extracted from
//! `turn.rs` (parent #680).
//!
//! Hosts the workspace-confined, cap-bounded post-Edit/Write excerpt
//! reader used by `repo_edit_observation::observe_evidence_from_repo_edit`
//! to capture a behavior-coverage snippet for
//! `plan_artifact_recovery`.
//!
//! Path confinement (defense-in-depth: callers already pass a
//! normalized relative path, but we re-resolve here):
//! - `resolve_user_path(work_root, relative_path)` + canonical root +
//!   `strip_prefix` rejects absolute / `..` escape / out-of-workspace
//!   symlinks / canonicalize failures / non-files.
//!
//! Cap-before-read:
//! - `open_excerpt_file_nofollow` + `Read::take(MAX_ARTIFACT_EXCERPT_BYTES + 1)`
//!   so we never read more than 8 KiB + 1 byte from disk.
//!   `std::fs::read` / `read_to_string` are intentionally avoided.
//!
//! Content guards:
//! - UTF-8 invalid → `None`. Embedded NUL → `None` (non-text).
//! - When the cap boundary splits a multi-byte UTF-8 character we
//!   truncate down to the last valid char boundary instead of giving
//!   up (CB-001 / Issue #636 Phase 4).
//! - `session::feedback::mask_secrets` then
//!   `session::feedback::mask_header_family` stacked, matching the
//!   redactor SSOT used elsewhere (DR4-002).
//! - Post-masking re-truncation on a UTF-8 char boundary so masking
//!   expansion can never blow past `MAX_ARTIFACT_EXCERPT_BYTES`.
//!
//! TOCTOU hardening: on Unix we open with `O_NOFOLLOW` so a symlink
//! swap between the path confinement check and the open call cannot
//! redirect us outside the workspace (CB-002 / Issue #636 Phase 4).
//!
//! Originally an `impl Agent` method; converted to a free function
//! taking `&Agent`, matching the `actor_loop_flow` / `reply_retry` /
//! earlier vertical-slice precedent. `pub(super)` limited / no facade
//! re-export (DR3-001).

use std::io::Read;

use super::Agent;
use super::file_excerpt::{
    open_excerpt_file_nofollow, truncate_on_char_boundary, utf8_prefix_respecting_cap,
};
use crate::safety::path_guard::resolve_user_path;

pub(super) fn bounded_post_edit_excerpt(agent: &Agent, relative_path: &str) -> Option<String> {
    let target = resolve_user_path(&agent.work_root, relative_path).ok()?;
    let root = std::fs::canonicalize(&agent.work_root).ok()?;
    if target.strip_prefix(&root).is_err() {
        return None;
    }
    if !target.is_file() {
        return None;
    }
    let cap = super::task_contract::MAX_ARTIFACT_EXCERPT_BYTES;
    let file = open_excerpt_file_nofollow(&target)?;
    let mut buf: Vec<u8> = Vec::with_capacity(cap + 1);
    file.take((cap as u64) + 1).read_to_end(&mut buf).ok()?;
    if buf.contains(&0u8) {
        return None;
    }
    let text = utf8_prefix_respecting_cap(&buf, cap)?.to_string();
    let masked = crate::session::feedback::mask_header_family(
        &crate::session::feedback::mask_secrets(&text),
    );
    Some(truncate_on_char_boundary(masked, cap))
}
