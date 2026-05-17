use std::path::{Path, PathBuf};

use super::quality;

#[cfg(test)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum DeterministicFallbackKind {
    EmptyFrameworkApp,
}

pub(super) fn empty_framework_app_files(request: &str) -> Option<Vec<(PathBuf, String)>> {
    quality::deterministic_empty_framework_app_files(request)
}

#[cfg(test)]
pub(super) fn empty_framework_game_files(request: &str) -> Option<Vec<(PathBuf, String)>> {
    quality::deterministic_empty_framework_game_files(request)
}

pub(super) fn empty_python_cli_files_with_names(
    request: &str,
    script_name: Option<&str>,
    sample_name: Option<&str>,
) -> Option<Vec<(PathBuf, String)>> {
    quality::deterministic_empty_python_cli_files_with_names(request, script_name, sample_name)
}

pub(super) fn fastapi_crud_files(request: &str) -> Option<Vec<(PathBuf, String)>> {
    quality::deterministic_fastapi_crud_files(request)
}

pub(super) fn empty_docs_files(request: &str) -> Option<Vec<(PathBuf, String)>> {
    quality::deterministic_empty_docs_files(request)
}

pub(super) fn playable_ui_repair(
    request: &str,
    target_path: &Path,
    current_content: &str,
) -> Option<String> {
    quality::deterministic_playable_ui_fallback(request, target_path, current_content)
}

pub(super) fn playable_ui_polish(
    request: &str,
    target_path: &Path,
    current_content: &str,
) -> Option<String> {
    quality::deterministic_playable_ui_polish_fallback(request, target_path, current_content)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn deterministic_module_exposes_fallback_boundary() {
        let request = "Reactでタスク管理アプリを作成してください";
        let files = empty_framework_app_files(request).expect("files");
        assert!(files.iter().any(|(path, _)| path.ends_with("src/App.tsx")));
        assert_eq!(
            DeterministicFallbackKind::EmptyFrameworkApp,
            DeterministicFallbackKind::EmptyFrameworkApp
        );
    }
}
