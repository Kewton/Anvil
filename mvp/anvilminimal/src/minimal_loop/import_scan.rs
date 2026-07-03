use std::path::{Path, PathBuf};

use regex::Regex;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MissingImport {
    pub source: String,
    pub specifier: String,
}

pub fn scan_relative_imports(root: &Path, paths: &[String]) -> anyhow::Result<Vec<MissingImport>> {
    let mut missing = Vec::new();
    for path in paths {
        if !is_source_path(path) {
            continue;
        }
        let source_path = root.join(path);
        if !source_path.is_file() {
            continue;
        }
        let content = std::fs::read_to_string(&source_path)?;
        let parent = source_path.parent().unwrap_or(root);
        for specifier in extract_import_specifiers(&content) {
            if !is_relative_specifier(&specifier) {
                continue;
            }
            if !resolve_import(parent, &specifier)
                .iter()
                .any(|path| path.exists())
            {
                missing.push(MissingImport {
                    source: path.clone(),
                    specifier,
                });
            }
        }
    }
    Ok(missing)
}

fn extract_import_specifiers(content: &str) -> Vec<String> {
    let patterns = [
        r#"(?m)(?:import|export)\s+(?:type\s+)?[^;]*?\s+from\s*["']([^"']+)["']"#,
        r#"(?m)import\s*["']([^"']+)["']"#,
        r#"(?m)import\s*\(\s*["']([^"']+)["']\s*\)"#,
        r#"(?m)require\s*\(\s*["']([^"']+)["']\s*\)"#,
    ];
    let mut out = Vec::new();
    for pattern in patterns {
        let re = Regex::new(pattern).expect("valid import regex");
        for captures in re.captures_iter(content) {
            if let Some(value) = captures.get(1) {
                out.push(value.as_str().to_string());
            }
        }
    }
    out
}

fn resolve_import(parent: &Path, specifier: &str) -> Vec<PathBuf> {
    let base = parent.join(specifier);
    let mut candidates = vec![base.clone()];
    for ext in ["ts", "tsx", "js", "jsx", "json", "css"] {
        candidates.push(base.with_extension(ext));
    }
    for ext in ["ts", "tsx", "js", "jsx", "json", "css"] {
        candidates.push(base.join(format!("index.{ext}")));
    }
    candidates
}

fn is_relative_specifier(value: &str) -> bool {
    value.starts_with("./") || value.starts_with("../")
}

fn is_source_path(path: &str) -> bool {
    Path::new(path).extension().is_some_and(|ext| {
        matches!(
            ext.to_str().unwrap_or_default(),
            "js" | "jsx" | "ts" | "tsx"
        )
    })
}

pub fn format_missing_import_feedback(missing: &[MissingImport]) -> String {
    let entries = missing
        .iter()
        .map(|item| format!("- {} imports missing `{}`", item.source, item.specifier))
        .collect::<Vec<_>>()
        .join("\n");
    format!(
        "One or more relative imports are unresolved. Create the missing module files or correct the import paths before final response:\n{entries}"
    )
}

pub fn missing_import_target_path(root: &Path, missing: &MissingImport) -> Option<PathBuf> {
    let source_path = root.join(&missing.source);
    let parent = source_path.parent().unwrap_or(root);
    let target = normalize_joined_path(&parent.join(&missing.specifier));
    target.starts_with(root).then_some(target)
}

pub fn missing_import_target_rel(root: &Path, missing: &MissingImport) -> Option<String> {
    let target = missing_import_target_path(root, missing)?;
    target
        .strip_prefix(root)
        .ok()
        .map(|path| path.display().to_string())
}

pub fn format_missing_import_findings(root: &Path, missing: &[MissingImport]) -> Vec<String> {
    missing
        .iter()
        .map(|item| match missing_import_target_rel(root, item) {
            Some(target) => format!(
                "{} imports {} which does not exist - create {}",
                item.source, item.specifier, target
            ),
            None => format!(
                "{} imports {} which does not exist",
                item.source, item.specifier
            ),
        })
        .collect()
}

fn normalize_joined_path(path: &Path) -> PathBuf {
    let mut normalized = PathBuf::new();
    for component in path.components() {
        match component {
            std::path::Component::CurDir => {}
            std::path::Component::ParentDir => {
                normalized.pop();
            }
            other => normalized.push(other.as_os_str()),
        }
    }
    normalized
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn relative_import_scanner_resolves_tsx_extension() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("src")).unwrap();
        std::fs::write(
            dir.path().join("src/page.tsx"),
            r#"import Widget from "./Widget";"#,
        )
        .unwrap();
        std::fs::write(
            dir.path().join("src/Widget.tsx"),
            "export default function Widget(){}",
        )
        .unwrap();
        let missing = scan_relative_imports(dir.path(), &["src/page.tsx".to_string()]).unwrap();
        assert!(missing.is_empty());
    }

    #[test]
    fn relative_import_scanner_resolves_index_file() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("src/widgets")).unwrap();
        std::fs::write(
            dir.path().join("src/page.tsx"),
            r#"export { Widget } from "./widgets";"#,
        )
        .unwrap();
        std::fs::write(
            dir.path().join("src/widgets/index.ts"),
            "export const Widget = 1;",
        )
        .unwrap();
        let missing = scan_relative_imports(dir.path(), &["src/page.tsx".to_string()]).unwrap();
        assert!(missing.is_empty());
    }

    #[test]
    fn relative_import_scanner_ignores_package_imports() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("src")).unwrap();
        std::fs::write(
            dir.path().join("src/page.tsx"),
            r#"import React from "react"; import Widget from "@/Widget";"#,
        )
        .unwrap();
        let missing = scan_relative_imports(dir.path(), &["src/page.tsx".to_string()]).unwrap();
        assert!(missing.is_empty());
    }

    #[test]
    fn relative_import_scanner_resolves_css_and_json_imports() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("src")).unwrap();
        std::fs::write(
            dir.path().join("src/page.tsx"),
            r#"import "./globals.css"; const cfg = require("./config.json");"#,
        )
        .unwrap();
        std::fs::write(dir.path().join("src/globals.css"), "body{}").unwrap();
        std::fs::write(dir.path().join("src/config.json"), "{}").unwrap();
        let missing = scan_relative_imports(dir.path(), &["src/page.tsx".to_string()]).unwrap();
        assert!(missing.is_empty());
    }

    #[test]
    fn missing_relative_import_is_reported() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("src")).unwrap();
        std::fs::write(
            dir.path().join("src/page.tsx"),
            r#"import Widget from "./Widget";"#,
        )
        .unwrap();
        let missing = scan_relative_imports(dir.path(), &["src/page.tsx".to_string()]).unwrap();
        assert_eq!(
            missing,
            vec![MissingImport {
                source: "src/page.tsx".to_string(),
                specifier: "./Widget".to_string()
            }]
        );
    }

    #[test]
    fn missing_import_target_rel_resolves_relative_css_target() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("src/app")).unwrap();
        std::fs::write(
            dir.path().join("src/app/layout.tsx"),
            r#"import "./globals.css";"#,
        )
        .unwrap();
        let missing =
            scan_relative_imports(dir.path(), &["src/app/layout.tsx".to_string()]).unwrap();

        assert_eq!(
            missing_import_target_rel(dir.path(), &missing[0]).as_deref(),
            Some("src/app/globals.css")
        );
        assert_eq!(
            format_missing_import_findings(dir.path(), &missing),
            vec![
                "src/app/layout.tsx imports ./globals.css which does not exist - create src/app/globals.css"
            ]
        );
    }
}
