//! Issue #646: active workspace scope for the current task.
//!
//! `TaskWorkspaceScope` is the deterministic answer to "which subtree of
//! `work_root` is this task allowed to claim ownership over?". It is rebuilt
//! per turn from the active request + filesystem layout and consumed by
//! `artifact_ownership::classify_ownership` to gate which existing files can
//! be promoted to completion evidence (Issue #646 §修正方針 1).
//!
//! Visibility: every export is `pub(super)` and **must not** be re-exported
//! from `src/agent/loop_run.rs` (CLAUDE.md DR3-001).

use std::path::{Component, Path, PathBuf};

/// Names that mark a directory as a project-like subtree. Structure-based
/// detection (Issue #646 §修正方針 1 / 非目標) — never name blacklist.
const PROJECT_MARKER_FILES: &[&str] = &[
    "pyproject.toml",
    "package.json",
    "Cargo.toml",
    "go.mod",
    "pom.xml",
    "build.gradle",
    "build.gradle.kts",
    "composer.json",
    "Gemfile",
    "manage.py",
    "next.config.js",
    "nuxt.config.ts",
    "vite.config.ts",
    "tsconfig.json",
];

const PROJECT_MARKER_DIRS: &[&str] = &[".git", "src", "tests"];

/// Conventional monorepo parent directories. When one of these is the only
/// non-marker child of `work_root` (or sits alongside other monorepo
/// parents), each child of the monorepo parent is treated as its own
/// nested subtree. Avoids "single project root" misclassification of
/// `packages/foo/`, `apps/bar/`, `crates/baz/` (Issue #646 A2).
const MONOREPO_PARENT_DIRS: &[&str] =
    &["packages", "apps", "crates", "services", "modules", "pkgs"];

/// Directories that never contribute project-like signal even if they look
/// like one (dependency / build caches). Skipped by every detection helper.
///
/// SSOT consumed by:
/// - `nested_project_subtrees` (scope detection)
/// - `artifact_ownership::path_in_ignored_top_dir`
/// - `turn::collect_meaningful_workspace_files` (workspace walker)
///
/// Always extend by editing this list; do NOT duplicate the names elsewhere.
pub(super) const IGNORED_TOP_DIRS: &[&str] = &[
    ".git",
    ".anvil",
    ".anvil-state",
    "node_modules",
    "target",
    ".venv",
    "venv",
    "dist",
    "build",
    "__pycache__",
];

/// SSOT predicate: returns `true` when `name` is the file-name component of
/// a directory that is never part of the active workspace (dependency
/// cache, VCS metadata, build output).
pub(super) fn is_workspace_ignored_dir(name: &str) -> bool {
    IGNORED_TOP_DIRS.contains(&name)
}

/// Mode of the active scope. Three deterministic states; the planner reads
/// this through [`TaskWorkspaceScope::contains`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum ScopeMode {
    /// User explicitly named one or more subtrees. Only those subtrees are
    /// in scope; everything else is `OutOfScope`.
    Explicit { paths: Vec<PathBuf> },
    /// `work_root` itself is a single project (manifest at root, no nested
    /// project-like subtrees). Anything inside `work_root` is in scope.
    SingleProjectRoot,
    /// `work_root` contains multiple project-like subtrees and the user
    /// gave no explicit hint. Only direct-root files are in scope; existing
    /// nested subtrees stay `OutOfScope`. This is the fresh-session
    /// parent-directory case from the bug report.
    AmbiguousParent { nested_subtrees: Vec<PathBuf> },
    /// `work_root` has no project-like subtree and the user gave no hint.
    /// Anything inside `work_root` is in scope (greenfield build).
    Greenfield,
}

// Issue #651 Task 3.1: visibility raised to `pub(crate)` so
// `VerifierInputs::workspace_scope` (re-exported via `pub` because
// `SkillInput::Verifier(VerifierInputs<'_>)` lives at the skills-layer
// boundary) does not trip the `private_interfaces` lint. Skill-framework
// crate code MUST NOT actually read the field — the DR1-007 contract is
// documented on `VerifierInputs::workspace_scope` itself.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct TaskWorkspaceScope {
    pub(super) mode: ScopeMode,
}

impl TaskWorkspaceScope {
    /// Build a scope deterministically from the workspace layout + request.
    /// Both inputs are read once per turn; nothing here touches the network
    /// or persistent state.
    pub(super) fn detect(work_root: &Path, request: &str) -> Self {
        let nested = nested_project_subtrees(work_root);
        let explicit = explicit_subtree_paths_in_request(request, work_root, &nested);
        if !explicit.is_empty() {
            return Self {
                mode: ScopeMode::Explicit { paths: explicit },
            };
        }
        if work_root_is_single_project(work_root, &nested) {
            return Self {
                mode: ScopeMode::SingleProjectRoot,
            };
        }
        if nested.is_empty() {
            return Self {
                mode: ScopeMode::Greenfield,
            };
        }
        Self {
            mode: ScopeMode::AmbiguousParent {
                nested_subtrees: nested,
            },
        }
    }

    /// Whether a normalized workspace-relative path is in active scope.
    /// `relative_path` MUST be canonicalized/normalized by the caller (no
    /// `..` components, forward slashes). Empty string represents the root.
    pub(super) fn contains(&self, relative_path: &str) -> bool {
        let normalized = normalize_relative(relative_path);
        match &self.mode {
            ScopeMode::Explicit { paths } => {
                paths.iter().any(|scope| path_is_inside(&normalized, scope))
            }
            ScopeMode::SingleProjectRoot | ScopeMode::Greenfield => true,
            ScopeMode::AmbiguousParent { nested_subtrees } => !nested_subtrees
                .iter()
                .any(|nested| path_is_inside(&normalized, nested)),
        }
    }

    /// Best-effort short label for log / note emission.
    #[allow(dead_code)]
    pub(super) fn mode_label(&self) -> &'static str {
        match self.mode {
            ScopeMode::Explicit { .. } => "explicit",
            ScopeMode::SingleProjectRoot => "single_project_root",
            ScopeMode::Greenfield => "greenfield",
            ScopeMode::AmbiguousParent { .. } => "ambiguous_parent",
        }
    }
}

fn normalize_relative(relative_path: &str) -> PathBuf {
    let path = Path::new(relative_path);
    let mut out = PathBuf::new();
    for component in path.components() {
        match component {
            Component::Normal(part) => out.push(part),
            // Defensive: callers must hand us a clean relative path. We
            // refuse to silently swallow `..` because the ownership layer
            // treats traversal as `OutOfScope` upstream.
            Component::ParentDir | Component::RootDir | Component::Prefix(_) => {}
            Component::CurDir => {}
        }
    }
    out
}

fn path_is_inside(candidate: &Path, scope: &Path) -> bool {
    if scope.as_os_str().is_empty() {
        return true;
    }
    candidate == scope || candidate.starts_with(scope)
}

/// Returns workspace-relative paths of subdirectories of `work_root` that
/// look like a project root by structure (manifest file or marker dir).
/// Sorted lexicographically and de-duplicated.
///
/// Monorepo aware (Issue #646 A2): when a child directory is one of
/// `MONOREPO_PARENT_DIRS` (`packages/`, `apps/`, `crates/`, …) and is
/// itself NOT project-like, its children are scanned and any that are
/// project-like are returned as `packages/<name>` etc. This stops one
/// `crates/<inner>` from being treated as "work_root is a single project".
pub(super) fn nested_project_subtrees(work_root: &Path) -> Vec<PathBuf> {
    let Ok(read_dir) = std::fs::read_dir(work_root) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for entry in read_dir.flatten() {
        let file_type = match entry.file_type() {
            Ok(ft) => ft,
            Err(_) => continue,
        };
        if !file_type.is_dir() {
            continue;
        }
        let name = entry.file_name();
        let name_str = name.to_string_lossy().to_string();
        if is_workspace_ignored_dir(&name_str) {
            continue;
        }
        let abs = entry.path();
        if directory_is_project_like(&abs) {
            out.push(PathBuf::from(name_str));
            continue;
        }
        if MONOREPO_PARENT_DIRS
            .iter()
            .any(|parent| *parent == name_str)
        {
            // Recurse exactly one level into the monorepo parent so its
            // sibling packages stay distinct subtrees.
            for nested in monorepo_child_subtrees(&abs, &name_str) {
                out.push(nested);
            }
        }
    }
    out.sort();
    out.dedup();
    out
}

fn monorepo_child_subtrees(parent_abs: &Path, parent_rel: &str) -> Vec<PathBuf> {
    let Ok(read_dir) = std::fs::read_dir(parent_abs) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for entry in read_dir.flatten() {
        let Ok(file_type) = entry.file_type() else {
            continue;
        };
        if !file_type.is_dir() {
            continue;
        }
        let child_name = entry.file_name();
        let child_name_str = child_name.to_string_lossy().to_string();
        if is_workspace_ignored_dir(&child_name_str) {
            continue;
        }
        if directory_is_project_like(&entry.path()) {
            out.push(PathBuf::from(format!("{parent_rel}/{child_name_str}")));
        }
    }
    out
}

fn directory_is_project_like(dir: &Path) -> bool {
    for marker in PROJECT_MARKER_FILES {
        if dir.join(marker).is_file() {
            return true;
        }
    }
    for marker in PROJECT_MARKER_DIRS {
        if dir.join(marker).is_dir() {
            return true;
        }
    }
    false
}

fn work_root_is_single_project(work_root: &Path, nested: &[PathBuf]) -> bool {
    if !nested.is_empty() {
        return false;
    }
    directory_is_project_like(work_root)
}

/// Find tokens in `request` that match an existing nested subtree name.
/// Conservative: only matches when the token exactly equals (or contains as
/// a path segment) a known subtree name from `nested`. This is the only
/// path on which a pre-existing subtree can be promoted into the active
/// scope (Issue #646 §修正方針 1: "ユーザーが明示した path がある場合のみ").
fn explicit_subtree_paths_in_request(
    request: &str,
    work_root: &Path,
    nested: &[PathBuf],
) -> Vec<PathBuf> {
    if nested.is_empty() {
        return Vec::new();
    }
    let mut out = Vec::new();
    for subtree in nested {
        let Some(name) = subtree.to_str() else {
            continue;
        };
        if name.is_empty() {
            continue;
        }
        if request_mentions_subtree(request, name) {
            // Re-validate it's still a real project-like subtree on disk
            // — defense in depth against TOCTOU between detection and use.
            if directory_is_project_like(&work_root.join(name)) {
                out.push(PathBuf::from(name));
            }
        }
    }
    out.sort();
    out.dedup();
    out
}

fn request_mentions_subtree(request: &str, name: &str) -> bool {
    // Match `name` as a whole word / path segment. Reject substring noise
    // (e.g. `0517` should not match because the subtree was `0517_003`).
    let mut start = 0;
    while let Some(idx) = request[start..].find(name) {
        let abs_idx = start + idx;
        let before = request[..abs_idx].chars().next_back();
        let after_idx = abs_idx + name.len();
        let after = request[after_idx..].chars().next();
        let before_ok = before.is_none_or(|c| !c.is_ascii_alphanumeric() && c != '_');
        let after_ok = after.is_none_or(|c| !c.is_ascii_alphanumeric() && c != '_');
        if before_ok && after_ok {
            return true;
        }
        start = abs_idx + name.len();
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    fn touch(path: &Path) {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).unwrap();
        }
        std::fs::write(path, "").unwrap();
    }

    #[test]
    fn greenfield_when_workspace_is_empty() {
        let dir = tempdir().unwrap();
        let scope = TaskWorkspaceScope::detect(dir.path(), "FastAPIでCRUDのAPIを開発してください");
        assert!(matches!(scope.mode, ScopeMode::Greenfield));
        assert!(scope.contains("app/main.py"));
    }

    #[test]
    fn single_project_root_when_manifest_at_root() {
        let dir = tempdir().unwrap();
        touch(&dir.path().join("pyproject.toml"));
        let scope = TaskWorkspaceScope::detect(dir.path(), "READMEを更新してください");
        assert!(matches!(scope.mode, ScopeMode::SingleProjectRoot));
        assert!(scope.contains("app/main.py"));
        assert!(scope.contains("README.md"));
    }

    #[test]
    fn ambiguous_parent_excludes_existing_nested_subtree() {
        let dir = tempdir().unwrap();
        touch(&dir.path().join("0517_003/pyproject.toml"));
        touch(&dir.path().join("0517_003/app/main.py"));
        let scope = TaskWorkspaceScope::detect(dir.path(), "FastAPIでCRUDのAPIを開発してください");
        match &scope.mode {
            ScopeMode::AmbiguousParent { nested_subtrees } => {
                assert_eq!(nested_subtrees, &vec![PathBuf::from("0517_003")]);
            }
            other => panic!("expected AmbiguousParent, got {other:?}"),
        }
        // Pre-existing subtree files are out of scope.
        assert!(!scope.contains("0517_003/app/main.py"));
        assert!(!scope.contains("0517_003/README.md"));
        // Fresh artifacts at the parent root are in scope.
        assert!(scope.contains("app/main.py"));
        assert!(scope.contains("README.md"));
    }

    #[test]
    fn explicit_subtree_mention_promotes_it_into_scope() {
        let dir = tempdir().unwrap();
        touch(&dir.path().join("0517_003/pyproject.toml"));
        touch(&dir.path().join("0517_003/app/main.py"));
        // User explicitly references the existing subtree by name.
        let scope = TaskWorkspaceScope::detect(dir.path(), "0517_003を修正してください");
        match &scope.mode {
            ScopeMode::Explicit { paths } => {
                assert_eq!(paths, &vec![PathBuf::from("0517_003")]);
            }
            other => panic!("expected Explicit, got {other:?}"),
        }
        assert!(scope.contains("0517_003/app/main.py"));
        // Files outside the explicit subtree are out of scope.
        assert!(!scope.contains("other/file.py"));
    }

    #[test]
    fn explicit_token_rejects_substring_noise() {
        let dir = tempdir().unwrap();
        touch(&dir.path().join("0517_003/pyproject.toml"));
        // "0517" alone must not match the subtree "0517_003".
        let scope = TaskWorkspaceScope::detect(dir.path(), "0517を分析してください");
        assert!(!matches!(scope.mode, ScopeMode::Explicit { .. }));
    }

    #[test]
    fn nested_subtrees_detected_via_marker_dirs() {
        let dir = tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("svc_a/src")).unwrap();
        std::fs::create_dir_all(dir.path().join("svc_b/tests")).unwrap();
        let nested = nested_project_subtrees(dir.path());
        assert!(nested.contains(&PathBuf::from("svc_a")));
        assert!(nested.contains(&PathBuf::from("svc_b")));
    }

    #[test]
    fn ignored_top_dirs_are_not_nested_subtrees() {
        let dir = tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("node_modules/foo")).unwrap();
        std::fs::create_dir_all(dir.path().join(".venv/lib")).unwrap();
        touch(&dir.path().join("node_modules/foo/package.json"));
        let nested = nested_project_subtrees(dir.path());
        assert!(!nested.contains(&PathBuf::from("node_modules")));
        assert!(!nested.contains(&PathBuf::from(".venv")));
    }

    #[test]
    fn monorepo_packages_layout_yields_nested_subtrees_per_package() {
        let dir = tempdir().unwrap();
        // `packages/<name>` layout — neither `packages/` itself nor
        // `work_root` is project-like, but each package has its own
        // marker file.
        touch(&dir.path().join("packages/api/package.json"));
        touch(&dir.path().join("packages/web/package.json"));
        let nested = nested_project_subtrees(dir.path());
        assert!(nested.contains(&PathBuf::from("packages/api")));
        assert!(nested.contains(&PathBuf::from("packages/web")));
        let scope = TaskWorkspaceScope::detect(dir.path(), "update the API service");
        match &scope.mode {
            ScopeMode::AmbiguousParent { nested_subtrees } => {
                assert!(nested_subtrees.contains(&PathBuf::from("packages/api")));
                assert!(nested_subtrees.contains(&PathBuf::from("packages/web")));
            }
            other => panic!("expected AmbiguousParent for monorepo layout, got {other:?}"),
        }
        // A sibling package's artifact stays out of scope.
        assert!(!scope.contains("packages/web/src/index.ts"));
        assert!(!scope.contains("packages/api/src/index.ts"));
    }

    #[test]
    fn monorepo_crates_layout_yields_nested_subtrees() {
        let dir = tempdir().unwrap();
        touch(&dir.path().join("crates/core/Cargo.toml"));
        touch(&dir.path().join("crates/cli/Cargo.toml"));
        let nested = nested_project_subtrees(dir.path());
        assert!(nested.contains(&PathBuf::from("crates/core")));
        assert!(nested.contains(&PathBuf::from("crates/cli")));
    }

    #[test]
    fn ambiguous_parent_with_two_subtrees_excludes_both() {
        let dir = tempdir().unwrap();
        touch(&dir.path().join("svc_a/pyproject.toml"));
        touch(&dir.path().join("svc_b/package.json"));
        let scope = TaskWorkspaceScope::detect(dir.path(), "全部修正してください");
        match &scope.mode {
            ScopeMode::AmbiguousParent { nested_subtrees } => {
                assert!(nested_subtrees.contains(&PathBuf::from("svc_a")));
                assert!(nested_subtrees.contains(&PathBuf::from("svc_b")));
            }
            other => panic!("expected AmbiguousParent, got {other:?}"),
        }
        assert!(!scope.contains("svc_a/app/main.py"));
        assert!(!scope.contains("svc_b/index.js"));
        assert!(scope.contains("README.md"));
    }
}
