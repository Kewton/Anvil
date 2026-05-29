//! Workspace-relative path normalization + package.json/lock sync
//! helpers extracted from `turn.rs` (parent #680).
//!
//! Hosts:
//!
//! * `normalize_memory_path(raw_path, work_root) -> String` — projects
//!   a raw path token into a workspace-relative key suitable for
//!   `WorkingMemory.touched_files` (slash-normalized, prefix-stripped).
//!   Used as the SSOT for keying touched-file relevance lookups.
//! * `normalize_exploration_path(raw_path, work_root) -> String` —
//!   similar shape but tuned for the plan-mode exploration repeat
//!   detector (component-filter pass + canonical-root fallback).
//! * `sync_package_json_with_existing_lock(work_root, relative, content)
//!   -> String` — when a deterministic scaffold rewrites
//!   `package.json` in a worktree that already has a `package-lock.json`,
//!   replace dependency sections so the lock stays internally
//!   consistent.
//!
//! `pub(super)` limited / no facade re-export (DR3-001).

use std::path::Path;

use crate::safety::path_guard::resolve_user_path;

pub(super) fn normalize_exploration_path(raw_path: &str, work_root: &Path) -> String {
    let input = Path::new(raw_path);
    if input.is_relative() {
        let cleaned = input
            .components()
            .filter_map(|component| match component {
                std::path::Component::Normal(part) => Some(part.to_string_lossy().to_string()),
                _ => None,
            })
            .collect::<Vec<_>>();
        if !cleaned.is_empty() {
            return cleaned.join("/");
        }
    }

    if let Ok(resolved) = resolve_user_path(work_root, raw_path) {
        let canonical_root = std::fs::canonicalize(work_root).ok();
        let canonical_resolved = std::fs::canonicalize(&resolved).ok();
        if let (Some(root), Some(resolved_path)) = (canonical_root, canonical_resolved)
            && let Ok(relative) = resolved_path.strip_prefix(root)
        {
            return relative.to_string_lossy().replace('\\', "/");
        }
        if let Ok(relative) = resolved.strip_prefix(work_root) {
            return relative.to_string_lossy().replace('\\', "/");
        }
        return resolved.to_string_lossy().replace('\\', "/");
    }

    raw_path.trim().replace('\\', "/")
}

pub(super) fn sync_package_json_with_existing_lock(
    work_root: &Path,
    relative: &Path,
    package_content: String,
) -> String {
    if relative != Path::new("package.json") {
        return package_content;
    }
    let Ok(lock_content) = std::fs::read_to_string(work_root.join("package-lock.json")) else {
        return package_content;
    };
    let Ok(mut package) = serde_json::from_str::<serde_json::Value>(&package_content) else {
        return package_content;
    };
    let Ok(lock) = serde_json::from_str::<serde_json::Value>(&lock_content) else {
        return package_content;
    };
    let Some(root_package) = lock
        .get("packages")
        .and_then(|packages| packages.get(""))
        .and_then(serde_json::Value::as_object)
    else {
        return package_content;
    };
    let Some(package_object) = package.as_object_mut() else {
        return package_content;
    };

    let mut replaced_any = false;
    for section in [
        "dependencies",
        "devDependencies",
        "optionalDependencies",
        "peerDependencies",
    ] {
        if let Some(lock_section) = root_package.get(section) {
            package_object.insert(section.to_string(), lock_section.clone());
            replaced_any = true;
        } else {
            package_object.remove(section);
        }
    }
    if !replaced_any {
        return package_content;
    }

    serde_json::to_string_pretty(&package)
        .map(|json| format!("{json}\n"))
        .unwrap_or(package_content)
}

pub(super) fn normalize_memory_path(raw_path: &str, work_root: &Path) -> String {
    let path = Path::new(raw_path);
    if let Ok(resolved) = resolve_user_path(work_root, raw_path)
        && let Ok(relative) = resolved.strip_prefix(work_root)
    {
        return relative.to_string_lossy().replace('\\', "/");
    }
    let canonical_root = std::fs::canonicalize(work_root).ok();
    let canonical_path = std::fs::canonicalize(path)
        .ok()
        .or_else(|| resolve_user_path(work_root, raw_path).ok());
    if let (Some(root), Some(candidate)) = (canonical_root, canonical_path)
        && let Ok(relative) = candidate.strip_prefix(root)
    {
        return relative.to_string_lossy().replace('\\', "/");
    }
    if let Ok(relative) = path.strip_prefix(work_root) {
        return relative.to_string_lossy().replace('\\', "/");
    }
    raw_path.replace('\\', "/")
}
