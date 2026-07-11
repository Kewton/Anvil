use std::path::{Path, PathBuf};

use crate::minimal_loop::evidence::required_evidence_for_capability;
use crate::minimal_loop::import_scan::{
    MissingImport, missing_import_target_rel, route_bound_closure,
};
use crate::planner::profile::{is_nextjs_profile, profile_evidence_repair_target_paths};
use crate::planner::runner::StepRunOutcome;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RepairTargetSelectionReason {
    EvidenceMapped,
    ContractAttribute,
    RepairChanged,
    RequiredPath,
    Fallback,
}

impl RepairTargetSelectionReason {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::EvidenceMapped => "evidence_mapped",
            Self::ContractAttribute => "contract_attribute",
            Self::RepairChanged => "repair_changed",
            Self::RequiredPath => "required_path",
            Self::Fallback => "fallback",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RepairTargetSelection {
    pub(crate) selected_targets: Vec<String>,
    pub(crate) selection_reason: RepairTargetSelectionReason,
}

impl RepairTargetSelection {
    #[cfg(test)]
    pub(crate) fn primary_target(&self) -> Option<&str> {
        self.selected_targets.first().map(String::as_str)
    }
}

pub(crate) struct RepairTargetResolutionInput<'a> {
    pub(crate) root: &'a Path,
    pub(crate) profile: &'a str,
    pub(crate) pending_evidence: &'a [String],
    pub(crate) missing_capabilities: &'a [String],
    pub(crate) contract_attribute_paths: &'a [String],
    pub(crate) repair_changed_paths: &'a [String],
    pub(crate) required_paths: &'a [String],
    pub(crate) fallback_paths: &'a [String],
}

#[derive(Debug, Clone, Default)]
pub(crate) struct FinalAcceptanceRepairTargets {
    pub(crate) selected_targets: Vec<String>,
    pub(crate) selection_reason: String,
}

impl FinalAcceptanceRepairTargets {
    pub(crate) fn primary_target(&self) -> Option<&str> {
        self.selected_targets.first().map(String::as_str)
    }
}

pub(crate) struct FinalAcceptanceRepairTargetInput<'a> {
    pub(crate) root: &'a Path,
    pub(crate) profile: &'a str,
    pub(crate) pending_evidence: &'a [String],
    pub(crate) contract_attribute_paths: &'a [String],
    pub(crate) repair_changed_paths: &'a [String],
    pub(crate) required_paths: &'a [String],
}

pub(crate) fn resolve_final_acceptance_repair_targets(
    input: FinalAcceptanceRepairTargetInput<'_>,
) -> FinalAcceptanceRepairTargets {
    resolve_repair_targets(RepairTargetResolutionInput {
        root: input.root,
        profile: input.profile,
        pending_evidence: input.pending_evidence,
        missing_capabilities: &[],
        contract_attribute_paths: input.contract_attribute_paths,
        repair_changed_paths: input.repair_changed_paths,
        required_paths: input.required_paths,
        fallback_paths: &[],
    })
    .map(|selection| FinalAcceptanceRepairTargets {
        selected_targets: selection.selected_targets,
        selection_reason: selection.selection_reason.as_str().to_string(),
    })
    .unwrap_or_default()
}

pub(crate) fn resolve_repair_targets(
    input: RepairTargetResolutionInput<'_>,
) -> Option<RepairTargetSelection> {
    let evidence_keys = repair_evidence_keys(input.pending_evidence, input.missing_capabilities);
    let evidence_mapped_paths =
        profile_evidence_repair_target_paths(input.root, input.profile, &evidence_keys)
            .into_iter()
            .filter(|path| !layout_source_path(path))
            .collect::<Vec<_>>();
    let mut fallback_paths = ordered_non_empty_paths(input.fallback_paths);
    merge_unique_strings(
        &mut fallback_paths,
        &default_repair_target_candidates(input.root, input.profile),
    );
    select_repair_targets_from_paths(RepairTargetPathBuckets {
        evidence_mapped_paths: &evidence_mapped_paths,
        contract_attribute_paths: input.contract_attribute_paths,
        repair_changed_paths: input.repair_changed_paths,
        required_paths: input.required_paths,
        fallback_paths: &fallback_paths,
    })
}

pub(crate) struct RepairTargetPathBuckets<'a> {
    pub(crate) evidence_mapped_paths: &'a [String],
    pub(crate) contract_attribute_paths: &'a [String],
    pub(crate) repair_changed_paths: &'a [String],
    pub(crate) required_paths: &'a [String],
    pub(crate) fallback_paths: &'a [String],
}

pub(crate) fn select_repair_targets_from_paths(
    buckets: RepairTargetPathBuckets<'_>,
) -> Option<RepairTargetSelection> {
    for (paths, reason) in [
        (
            buckets.evidence_mapped_paths,
            RepairTargetSelectionReason::EvidenceMapped,
        ),
        (
            buckets.contract_attribute_paths,
            RepairTargetSelectionReason::ContractAttribute,
        ),
        (
            buckets.repair_changed_paths,
            RepairTargetSelectionReason::RepairChanged,
        ),
        (
            buckets.required_paths,
            RepairTargetSelectionReason::RequiredPath,
        ),
        (
            buckets.fallback_paths,
            RepairTargetSelectionReason::Fallback,
        ),
    ] {
        let selected_targets = ordered_non_empty_paths(paths);
        if !selected_targets.is_empty() {
            return Some(RepairTargetSelection {
                selected_targets,
                selection_reason: reason,
            });
        }
    }
    None
}

pub(crate) fn repair_evidence_keys(
    pending_evidence: &[String],
    missing_capabilities: &[String],
) -> Vec<String> {
    let mut out = Vec::new();
    for key in pending_evidence {
        push_normalized_evidence_keys(&mut out, key);
    }
    for capability in missing_capabilities {
        for evidence in required_evidence_for_capability(capability) {
            push_unique_trimmed(&mut out, &evidence);
        }
    }
    out
}

pub(crate) fn missing_import_target_paths(root: &Path, missing: &[MissingImport]) -> Vec<String> {
    missing
        .iter()
        .filter_map(|missing| missing_import_target_rel(root, missing))
        .collect()
}

pub(crate) fn ensure_session_error_repair_target(outcome: &mut StepRunOutcome) {
    if !outcome.repair_targets.is_empty() {
        return;
    }
    if !outcome.observed_missing_evidence.is_empty()
        || !outcome.observed_missing_obligations.is_empty()
    {
        outcome
            .repair_targets
            .push("required_evidence_missing".to_string());
    } else if !outcome.observed_missing_capabilities.is_empty() {
        outcome
            .repair_targets
            .push("capability_missing".to_string());
    }
}

pub(crate) fn missing_obligation_targets_from_text(text: &str) -> Vec<String> {
    let Some((_, rest)) = text.split_once("missing_required_obligation_target:") else {
        return Vec::new();
    };
    let end = rest
        .find(|ch: char| ch.is_whitespace() || matches!(ch, ';' | ')' | ']'))
        .unwrap_or(rest.len());
    rest[..end]
        .split(',')
        .filter_map(|value| value.split(':').next())
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
        .collect()
}

fn push_normalized_evidence_keys(out: &mut Vec<String>, raw: &str) {
    let mut found = false;
    for token in raw.split(|ch: char| !(ch.is_ascii_alphanumeric() || ch == '_')) {
        if canonical_evidence_token(token) {
            push_unique_trimmed(out, token);
            found = true;
        }
    }
    if !found {
        push_unique_trimmed(out, raw);
    }
}

fn canonical_evidence_token(token: &str) -> bool {
    token.ends_with("_evidence")
        && !matches!(
            token,
            "missing_required_evidence"
                | "unsupported_required_evidence"
                | "weak_source_evidence"
                | "weak_verification_evidence"
        )
}

pub(crate) fn default_repair_target_candidates(root: &Path, profile: &str) -> Vec<String> {
    let mut out = Vec::new();
    for path in route_bound_source_candidates(root, profile) {
        push_unique_trimmed(&mut out, &path);
    }
    for path in nextjs_route_entry_candidates(root) {
        push_unique_trimmed(&mut out, &path);
    }
    for candidate in [
        "src/app/page.tsx",
        "src/app/page.jsx",
        "src/app/page.ts",
        "src/app/page.js",
        "app/page.tsx",
        "app/page.jsx",
        "app/page.ts",
        "app/page.js",
        "pages/index.tsx",
        "pages/index.jsx",
        "pages/index.ts",
        "pages/index.js",
        "src/pages/index.tsx",
        "src/pages/index.jsx",
        "src/pages/index.ts",
        "src/pages/index.js",
        "package.json",
        "tsconfig.json",
        "postcss.config.js",
        "postcss.config.mjs",
        "tailwind.config.js",
        "tailwind.config.ts",
    ] {
        push_unique_trimmed(&mut out, candidate);
    }
    out
}

pub(crate) fn final_acceptance_snapshot_candidates(root: &Path, profile: &str) -> Vec<String> {
    let mut out = default_repair_target_candidates(root, profile);
    for candidate in [
        "next.config.js",
        "next.config.mjs",
        "next.config.ts",
        "vite.config.js",
        "vite.config.ts",
        "Cargo.toml",
        "pyproject.toml",
    ] {
        push_unique_trimmed(&mut out, candidate);
    }
    out
}

pub(crate) fn profile_invariant_excerpt_candidates(
    root: &Path,
    profile: &str,
    tailwind_failure: bool,
) -> Vec<String> {
    if !is_nextjs_profile(profile) {
        return Vec::new();
    }
    let defaults = if tailwind_failure {
        vec![
            "package.json",
            "postcss.config.js",
            "postcss.config.cjs",
            "postcss.config.mjs",
            "postcss.config",
            "tailwind.config.js",
            "tailwind.config.cjs",
            "tailwind.config.mjs",
            "tailwind.config.ts",
            "src/app/layout.tsx",
            "src/app/layout.jsx",
            "src/app/layout.ts",
            "src/app/layout.js",
            "app/layout.tsx",
            "app/layout.jsx",
            "app/layout.ts",
            "app/layout.js",
            "src/app/globals.css",
            "src/app/global.css",
            "app/globals.css",
            "app/global.css",
            "src/styles/globals.css",
            "styles/globals.css",
        ]
    } else {
        vec![
            "package.json",
            "tsconfig.json",
            "src/app/page.tsx",
            "src/app/layout.tsx",
            "app/page.tsx",
            "app/layout.tsx",
        ]
    };
    let mut out = ordered_non_empty_paths(&defaults);
    merge_unique_strings(&mut out, &default_repair_target_candidates(root, profile));
    out
}

fn route_bound_source_candidates(root: &Path, profile: &str) -> Vec<String> {
    let route_bound = route_bound_closure(root, profile)
        .into_iter()
        .filter_map(normalized_source_path)
        .collect::<Vec<_>>();
    let mut out = Vec::new();
    for rel in route_bound
        .iter()
        .filter(|rel| route_entry_source_path(rel))
    {
        push_unique_trimmed(&mut out, rel);
    }
    for rel in route_bound.iter().filter(|rel| !layout_source_path(rel)) {
        push_unique_trimmed(&mut out, rel);
    }
    for rel in route_bound {
        push_unique_trimmed(&mut out, &rel);
    }
    out
}

fn nextjs_route_entry_candidates(root: &Path) -> Vec<String> {
    let mut out = Vec::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            let name = entry.file_name().to_string_lossy().to_string();
            if name == "node_modules" || name == ".anvil" || name.starts_with('.') {
                continue;
            }
            let Ok(file_type) = entry.file_type() else {
                continue;
            };
            if file_type.is_dir() {
                stack.push(path);
            } else if file_type.is_file()
                && let Ok(rel) = path.strip_prefix(root)
                && let Some(rel) = normalized_source_path(rel.to_path_buf())
                && route_entry_source_path(&rel)
            {
                push_unique_trimmed(&mut out, &rel);
            }
            if out.len() >= 128 {
                break;
            }
        }
    }
    out.sort();
    out
}

fn normalized_source_path(path: PathBuf) -> Option<String> {
    let rel = path.to_string_lossy().replace('\\', "/");
    let ext = Path::new(&rel).extension().and_then(|ext| ext.to_str())?;
    matches!(ext, "tsx" | "ts" | "jsx" | "js" | "mjs" | "cjs" | "css").then_some(rel)
}

fn route_entry_source_path(path: &str) -> bool {
    let name = Path::new(path)
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase();
    matches!(
        name.as_str(),
        "page.tsx"
            | "page.ts"
            | "page.jsx"
            | "page.js"
            | "index.tsx"
            | "index.ts"
            | "index.jsx"
            | "index.js"
    )
}

fn layout_source_path(path: &str) -> bool {
    let name = Path::new(path)
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase();
    matches!(
        name.as_str(),
        "layout.tsx" | "layout.ts" | "layout.jsx" | "layout.js"
    )
}

fn ordered_non_empty_paths(paths: &[impl AsRef<str>]) -> Vec<String> {
    let mut out = Vec::new();
    for path in paths {
        push_unique_trimmed(&mut out, path.as_ref());
    }
    out
}

fn merge_unique_strings(out: &mut Vec<String>, values: &[String]) {
    for value in values {
        push_unique_trimmed(out, value);
    }
}

fn push_unique_trimmed(out: &mut Vec<String>, value: &str) {
    let trimmed = value.trim().trim_start_matches("./").replace('\\', "/");
    if trimmed.is_empty()
        || trimmed.starts_with('/')
        || trimmed.contains('\0')
        || trimmed.contains("..")
        || out.iter().any(|existing| existing == &trimmed)
    {
        return;
    }
    out.push(trimmed);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write_nextjs_route(dir: &Path) {
        std::fs::create_dir_all(dir.join("src/app")).unwrap();
        std::fs::write(dir.join("package.json"), "{}").unwrap();
        std::fs::write(
            dir.join("src/app/page.tsx"),
            r#"export default function Page(){ return <main data-anvil-action="primary">Game</main>; }"#,
        )
        .unwrap();
        std::fs::write(
            dir.join("src/app/layout.tsx"),
            "export default function Layout({children}){ return <html><body>{children}</body></html>; }",
        )
        .unwrap();
    }

    #[test]
    fn pending_restart_evidence_beats_package_first_required_paths() {
        let dir = tempfile::tempdir().unwrap();
        write_nextjs_route(dir.path());

        let selection = resolve_repair_targets(RepairTargetResolutionInput {
            root: dir.path(),
            profile: "nextjs",
            pending_evidence: &["restart_or_recoverable_state_evidence".to_string()],
            missing_capabilities: &[],
            contract_attribute_paths: &[],
            repair_changed_paths: &[],
            required_paths: &["package.json".to_string(), "src/app/page.tsx".to_string()],
            fallback_paths: &["package.json".to_string()],
        })
        .unwrap();

        assert_eq!(selection.primary_target(), Some("src/app/page.tsx"));
        assert_eq!(
            selection.selection_reason,
            RepairTargetSelectionReason::EvidenceMapped
        );
        assert!(
            !selection
                .selected_targets
                .contains(&"package.json".to_string())
        );
    }

    #[test]
    fn contract_attribute_beats_repair_changed_and_required_paths() {
        let selection = select_repair_targets_from_paths(RepairTargetPathBuckets {
            evidence_mapped_paths: &[],
            contract_attribute_paths: &["src/app/page.tsx".to_string()],
            repair_changed_paths: &["src/hooks/game.ts".to_string()],
            required_paths: &["package.json".to_string()],
            fallback_paths: &[],
        })
        .unwrap();

        assert_eq!(selection.primary_target(), Some("src/app/page.tsx"));
        assert_eq!(
            selection.selection_reason,
            RepairTargetSelectionReason::ContractAttribute
        );
    }

    #[test]
    fn default_candidates_prefer_route_entry_over_layout() {
        let dir = tempfile::tempdir().unwrap();
        write_nextjs_route(dir.path());

        let selection = resolve_repair_targets(RepairTargetResolutionInput {
            root: dir.path(),
            profile: "nextjs",
            pending_evidence: &[],
            missing_capabilities: &[],
            contract_attribute_paths: &[],
            repair_changed_paths: &[],
            required_paths: &[],
            fallback_paths: &[],
        })
        .unwrap();

        assert_eq!(selection.primary_target(), Some("src/app/page.tsx"));
        assert_ne!(selection.primary_target(), Some("src/app/layout.tsx"));
    }

    #[test]
    fn final_acceptance_resolution_records_selected_target_and_reason() {
        let dir = tempfile::tempdir().unwrap();
        write_nextjs_route(dir.path());

        let selection = resolve_final_acceptance_repair_targets(FinalAcceptanceRepairTargetInput {
            root: dir.path(),
            profile: "nextjs",
            pending_evidence: &["restart_or_recoverable_state_evidence".to_string()],
            contract_attribute_paths: &[],
            repair_changed_paths: &[],
            required_paths: &["package.json".to_string(), "src/app/page.tsx".to_string()],
        });

        assert_eq!(selection.primary_target(), Some("src/app/page.tsx"));
        assert_eq!(selection.selected_targets, vec!["src/app/page.tsx"]);
        assert_eq!(selection.selection_reason, "evidence_mapped");
    }

    #[test]
    fn final_acceptance_normalizes_prefixed_restart_evidence() {
        let dir = tempfile::tempdir().unwrap();
        write_nextjs_route(dir.path());

        for pending_evidence in [
            "weak_source_evidence:restart_or_recoverable_state_evidence:restart handler does not reset entities",
            "route_unbound_capability_artifact:src/game.tsx:restart_or_recoverable_state_evidence",
        ] {
            let selection =
                resolve_final_acceptance_repair_targets(FinalAcceptanceRepairTargetInput {
                    root: dir.path(),
                    profile: "nextjs",
                    pending_evidence: &[pending_evidence.to_string()],
                    contract_attribute_paths: &[],
                    repair_changed_paths: &[],
                    required_paths: &["package.json".to_string(), "src/app/page.tsx".to_string()],
                });

            assert_eq!(selection.primary_target(), Some("src/app/page.tsx"));
            assert_eq!(selection.selected_targets, vec!["src/app/page.tsx"]);
            assert_eq!(
                selection.selection_reason, "evidence_mapped",
                "pending evidence was not normalized: {pending_evidence}"
            );
        }
    }
}
