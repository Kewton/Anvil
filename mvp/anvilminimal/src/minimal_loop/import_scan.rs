use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use regex::Regex;
use serde::Serialize;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MissingImport {
    pub source: String,
    pub specifier: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImportedDefinitionExcerpt {
    pub local_name: String,
    pub imported_name: String,
    pub specifier: String,
    pub definition_path: String,
    pub excerpt: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct UnattachedRefCandidateElement {
    pub source: String,
    pub line: usize,
    pub tag: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct UnattachedRefDiagnostic {
    pub diagnostic: String,
    pub name: String,
    pub source: String,
    pub declaration_line: usize,
    pub candidate_elements: Vec<UnattachedRefCandidateElement>,
    pub guidance: String,
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

pub fn imported_symbol_definition_excerpt(
    root: &Path,
    source: &str,
    local_name: &str,
) -> Option<ImportedDefinitionExcerpt> {
    let source_path = root.join(source);
    if !source_path.is_file() || !is_source_path(source) {
        return None;
    }
    let content = std::fs::read_to_string(&source_path).ok()?;
    let import = find_imported_symbol(&content, local_name)?;
    let definition_path = resolve_import_for_source(root, &source_path, &import.specifier)
        .into_iter()
        .find(|path| path.is_file())?;
    let definition_content = std::fs::read_to_string(&definition_path).ok()?;
    let excerpt = exported_definition_excerpt(&definition_content, &import.imported_name)?;
    let definition_rel = definition_path
        .strip_prefix(root)
        .ok()
        .map(normalize_pathbuf)?
        .display()
        .to_string();
    Some(ImportedDefinitionExcerpt {
        local_name: local_name.to_string(),
        imported_name: import.imported_name,
        specifier: import.specifier,
        definition_path: definition_rel,
        excerpt,
    })
}

pub fn route_bound_closure(root: &Path, profile: &str) -> BTreeSet<PathBuf> {
    let all_source_files = collect_route_source_files(root);
    if !matches!(profile, "nextjs" | "next-js" | "next.js") {
        return all_source_files;
    }

    let Some((project_root, project_prefix)) = nextjs_project_root(root) else {
        return all_source_files;
    };
    let entrypoints = nextjs_app_router_entrypoints(&project_root, &project_prefix);
    if entrypoints.is_empty() {
        return all_source_files;
    }

    let mut closure = BTreeSet::new();
    let mut stack = entrypoints;
    while let Some(rel) = stack.pop() {
        if !closure.insert(rel.clone()) {
            continue;
        }
        let full = root.join(&rel);
        if !full.is_file() {
            continue;
        }
        let Ok(content) = std::fs::read_to_string(&full) else {
            continue;
        };
        let Some(parent) = full.parent() else {
            continue;
        };
        for specifier in extract_import_specifiers(&content) {
            let candidates = if is_relative_specifier(&specifier) {
                resolve_route_import(parent, &specifier)
            } else if let Some(alias_path) = specifier.strip_prefix("@/") {
                resolve_workspace_alias_import(&project_root, alias_path)
            } else {
                Vec::new()
            };
            for candidate in candidates {
                if !candidate.is_file() {
                    continue;
                }
                let Ok(candidate_rel) = candidate.strip_prefix(root) else {
                    continue;
                };
                stack.push(normalize_pathbuf(candidate_rel));
                break;
            }
        }
    }
    if closure.is_empty() {
        all_source_files
    } else {
        closure
    }
}

pub fn route_bound_unattached_ref_diagnostics(
    root: &Path,
    profile: &str,
) -> Vec<UnattachedRefDiagnostic> {
    let mut diagnostics = Vec::new();
    for rel in route_bound_closure(root, profile) {
        let rel_text = rel.to_string_lossy().replace('\\', "/");
        if !is_source_path(&rel_text) {
            continue;
        }
        let path = root.join(&rel);
        let Ok(content) = std::fs::read_to_string(&path) else {
            continue;
        };
        diagnostics.extend(unattached_ref_diagnostics_for_source(&rel_text, &content));
    }
    diagnostics.sort_by(|a, b| {
        a.source
            .cmp(&b.source)
            .then_with(|| a.declaration_line.cmp(&b.declaration_line))
            .then_with(|| a.name.cmp(&b.name))
    });
    diagnostics.dedup_by(|a, b| {
        a.source == b.source
            && a.declaration_line == b.declaration_line
            && a.name == b.name
            && a.candidate_elements == b.candidate_elements
    });
    diagnostics
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ImportedSymbol {
    imported_name: String,
    specifier: String,
}

fn find_imported_symbol(content: &str, local_name: &str) -> Option<ImportedSymbol> {
    for statement in import_statements(content) {
        let Some(specifier) = import_statement_specifier(&statement) else {
            continue;
        };
        let Some(head) = statement.split_once(" from ").map(|(head, _)| head) else {
            continue;
        };
        if let Some((named, _)) = head
            .split_once('{')
            .and_then(|(_, rest)| rest.split_once('}'))
        {
            for item in named.split(',') {
                let trimmed = item.trim();
                if trimmed.is_empty() {
                    continue;
                }
                let (imported, local) = trimmed
                    .split_once(" as ")
                    .map(|(imported, local)| (imported.trim(), local.trim()))
                    .unwrap_or((trimmed, trimmed));
                if local == local_name {
                    return Some(ImportedSymbol {
                        imported_name: imported.to_string(),
                        specifier,
                    });
                }
            }
        } else {
            let default_name = head
                .trim_start_matches("import")
                .trim()
                .split(',')
                .next()
                .unwrap_or_default()
                .trim();
            if !default_name.is_empty() && default_name == local_name {
                return Some(ImportedSymbol {
                    imported_name: "default".to_string(),
                    specifier,
                });
            }
        }
    }
    None
}

fn unattached_ref_diagnostics_for_source(
    source: &str,
    content: &str,
) -> Vec<UnattachedRefDiagnostic> {
    let declarations = use_ref_declarations(content);
    if declarations.is_empty() {
        return Vec::new();
    }
    let candidate_elements = jsx_ref_candidate_elements(source, content);
    if candidate_elements.is_empty() {
        return Vec::new();
    }
    declarations
        .into_iter()
        .filter(|decl| ref_passed_across_boundary(content, &decl.name))
        .filter(|decl| !jsx_ref_attached(content, &decl.name))
        .map(|decl| {
            let first = candidate_elements
                .first()
                .expect("candidate_elements checked non-empty");
            let guidance = format!(
                "attach ref={{{}}} to the <{}> at {}:{}",
                decl.name,
                first.tag,
                display_basename(&first.source),
                first.line
            );
            UnattachedRefDiagnostic {
                diagnostic: format!("unattached_ref:{}", decl.name),
                name: decl.name,
                source: source.to_string(),
                declaration_line: decl.line,
                candidate_elements: candidate_elements.clone(),
                guidance,
            }
        })
        .collect()
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct UseRefDeclaration {
    name: String,
    line: usize,
}

fn use_ref_declarations(content: &str) -> Vec<UseRefDeclaration> {
    let re = Regex::new(
        r#"\b(?:const|let|var)\s+([A-Za-z_$][A-Za-z0-9_$]*)\s*=\s*(?:React\.)?useRef(?:\s*<[^;\n=]+>)?\s*\("#,
    )
    .expect("valid useRef declaration regex");
    re.captures_iter(content)
        .filter_map(|captures| {
            let whole = captures.get(0)?;
            let name = captures.get(1)?.as_str().to_string();
            Some(UseRefDeclaration {
                name,
                line: line_number_at(content, whole.start()),
            })
        })
        .collect()
}

fn ref_passed_across_boundary(content: &str, name: &str) -> bool {
    let escaped = regex::escape(name);
    let call = Regex::new(&format!(
        r#"\b(?:use[A-Za-z0-9_$]*|[A-Z][A-Za-z0-9_$]*)\s*\([^;\n{{}})]*\b{escaped}\b[^;\n{{}})]*\)"#
    ))
    .expect("valid ref call regex");
    if call.is_match(content) {
        return true;
    }
    let component_prop = Regex::new(&format!(
        r#"(?is)<[A-Z][A-Za-z0-9_.$]*\b[^>]*\b[A-Za-z_$][A-Za-z0-9_$-]*\s*=\s*\{{\s*{escaped}\s*\}}"#
    ))
    .expect("valid component prop regex");
    component_prop.is_match(content)
}

fn jsx_ref_attached(content: &str, name: &str) -> bool {
    let escaped = regex::escape(name);
    Regex::new(&format!(r#"\bref\s*=\s*\{{\s*{escaped}\s*\}}"#))
        .expect("valid jsx ref regex")
        .is_match(content)
}

fn jsx_ref_candidate_elements(source: &str, content: &str) -> Vec<UnattachedRefCandidateElement> {
    let re = Regex::new(r#"(?is)<(canvas|video|input)\b([^>]*)>"#)
        .expect("valid JSX ref candidate regex");
    let ref_re = Regex::new(r#"(?i)\bref\s*="#).expect("valid JSX ref attribute regex");
    let mut candidates = re
        .captures_iter(content)
        .filter_map(|captures| {
            let whole = captures.get(0)?;
            let tag = captures.get(1)?.as_str().to_ascii_lowercase();
            let attrs = captures.get(2).map_or("", |m| m.as_str());
            (!ref_re.is_match(attrs)).then(|| UnattachedRefCandidateElement {
                source: source.to_string(),
                line: line_number_at(content, whole.start()),
                tag,
            })
        })
        .collect::<Vec<_>>();
    candidates.sort_by(|a, b| {
        element_priority(&a.tag)
            .cmp(&element_priority(&b.tag))
            .then_with(|| a.line.cmp(&b.line))
    });
    candidates
}

fn element_priority(tag: &str) -> usize {
    match tag {
        "canvas" => 0,
        "video" => 1,
        "input" => 2,
        _ => 3,
    }
}

fn line_number_at(content: &str, byte_offset: usize) -> usize {
    content[..byte_offset.min(content.len())]
        .bytes()
        .filter(|byte| *byte == b'\n')
        .count()
        + 1
}

fn display_basename(source: &str) -> String {
    Path::new(source)
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or(source)
        .to_string()
}

fn import_statements(content: &str) -> Vec<String> {
    content
        .split(';')
        .filter_map(|part| {
            let trimmed = part.trim();
            (trimmed.starts_with("import ") && trimmed.contains(" from "))
                .then(|| trimmed.to_string())
        })
        .collect()
}

fn import_statement_specifier(statement: &str) -> Option<String> {
    let (_, rest) = statement.rsplit_once(" from ")?;
    let trimmed = rest.trim();
    let quote = trimmed.chars().next()?;
    if !matches!(quote, '\'' | '"') {
        return None;
    }
    let rest = &trimmed[quote.len_utf8()..];
    let end = rest.find(quote)?;
    Some(rest[..end].to_string())
}

fn resolve_import_for_source(root: &Path, source_path: &Path, specifier: &str) -> Vec<PathBuf> {
    let Some(parent) = source_path.parent() else {
        return Vec::new();
    };
    if is_relative_specifier(specifier) {
        return resolve_route_import(parent, specifier);
    }
    if let Some(alias_path) = specifier.strip_prefix("@/")
        && let Some((project_root, _)) = nextjs_project_root(root)
    {
        return resolve_workspace_alias_import(&project_root, alias_path);
    }
    Vec::new()
}

fn exported_definition_excerpt(content: &str, symbol: &str) -> Option<String> {
    let lines = content.lines().collect::<Vec<_>>();
    if let Some(excerpt) = exported_class_api_surface_excerpt(&lines, symbol) {
        return Some(excerpt);
    }
    if let Some(excerpt) = exported_interface_api_surface_excerpt(&lines, symbol) {
        return Some(excerpt);
    }
    if let Some(excerpt) = exported_object_literal_api_surface_excerpt(&lines, symbol) {
        return Some(excerpt);
    }
    let start = find_exported_definition_start(&lines, symbol)?;
    let end = bounded_definition_end(&lines, start);
    Some(lines[start..end].join("\n"))
}

fn find_exported_definition_start(lines: &[&str], symbol: &str) -> Option<usize> {
    lines.iter().position(|line| {
        let trimmed = line.trim_start();
        let function = format!("export function {symbol}");
        let async_function = format!("export async function {symbol}");
        let const_export = format!("export const {symbol}");
        let class_export = format!("export class {symbol}");
        let interface_export = format!("export interface {symbol}");
        let type_export = format!("export type {symbol}");
        let named_default = if symbol == "default" {
            trimmed.starts_with("export default function")
                || trimmed.starts_with("export default async function")
        } else {
            false
        };
        trimmed.starts_with(&function)
            || trimmed.starts_with(&async_function)
            || trimmed.starts_with(&const_export)
            || trimmed.starts_with(&class_export)
            || trimmed.starts_with(&interface_export)
            || trimmed.starts_with(&type_export)
            || named_default
    })
}

fn exported_class_api_surface_excerpt(lines: &[&str], symbol: &str) -> Option<String> {
    let start = lines.iter().position(|line| {
        let trimmed = line.trim_start();
        trimmed.starts_with(&format!("export class {symbol} "))
            || trimmed.starts_with(&format!("export class {symbol}{{"))
            || trimmed.starts_with(&format!("export class {symbol}<"))
    })?;
    let mut out = vec![format!("Public API surface for `{symbol}`:")];
    out.push(lines[start].trim().trim_end_matches('{').trim().to_string() + " {");
    let mut depth = 0isize;
    let mut seen_open = false;
    for line in lines.iter().skip(start) {
        let trimmed = line.trim();
        let member_depth = depth;
        if seen_open
            && member_depth == 1
            && let Some(signature) = class_member_signature(trimmed)
        {
            out.push(format!("  {signature}"));
            if out.len() >= 31 {
                break;
            }
        }
        for ch in trimmed.chars() {
            match ch {
                '{' => {
                    depth += 1;
                    seen_open = true;
                }
                '}' => depth -= 1,
                _ => {}
            }
        }
        if seen_open && depth <= 0 {
            break;
        }
    }
    if out.len() <= 2 {
        return None;
    }
    out.push("}".to_string());
    Some(out.join("\n"))
}

fn class_member_signature(line: &str) -> Option<String> {
    if line.is_empty()
        || line.starts_with("//")
        || line.starts_with('*')
        || line.starts_with("constructor")
        || line.starts_with("private ")
        || line.starts_with("protected ")
        || line.starts_with('#')
    {
        return None;
    }
    let publicish = line.starts_with("public ")
        || line.starts_with("async ")
        || line.starts_with("static ")
        || line.starts_with("readonly ")
        || line
            .chars()
            .next()
            .is_some_and(|ch| ch.is_ascii_alphabetic() || ch == '_' || ch == '$');
    if !publicish {
        return None;
    }
    let signature = line
        .split_once('{')
        .map(|(head, _)| head)
        .unwrap_or(line)
        .trim()
        .trim_end_matches(';')
        .trim_end_matches(',')
        .trim();
    (!signature.is_empty()
        && (signature.contains('(') || signature.contains(':') || signature.contains('=')))
    .then(|| format!("{signature};"))
}

fn exported_interface_api_surface_excerpt(lines: &[&str], symbol: &str) -> Option<String> {
    let start = lines.iter().position(|line| {
        let trimmed = line.trim_start();
        trimmed.starts_with(&format!("export interface {symbol} "))
            || trimmed.starts_with(&format!("export interface {symbol}{{"))
            || trimmed.starts_with(&format!("export interface {symbol}<"))
    })?;
    let end = bounded_definition_end(lines, start);
    let mut out = vec![format!("Public API surface for `{symbol}`:")];
    for line in lines[start..end].iter().take(30) {
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with("//") {
            continue;
        }
        out.push(trimmed.to_string());
        if out.len() >= 31 {
            break;
        }
    }
    (out.len() > 1).then(|| out.join("\n"))
}

fn exported_object_literal_api_surface_excerpt(lines: &[&str], symbol: &str) -> Option<String> {
    let start = lines.iter().position(|line| {
        let trimmed = line.trim_start();
        trimmed.starts_with(&format!("export const {symbol} = {{"))
            || trimmed.starts_with(&format!("export let {symbol} = {{"))
            || trimmed.starts_with(&format!("export var {symbol} = {{"))
    })?;
    let mut out = vec![format!("Public API surface for `{symbol}`:")];
    out.push(lines[start].trim().to_string());
    let mut depth = 0isize;
    let mut seen_open = false;
    for line in lines.iter().skip(start + 1) {
        let trimmed = line.trim();
        for ch in trimmed.chars() {
            match ch {
                '{' => {
                    depth += 1;
                    seen_open = true;
                }
                '}' => depth -= 1,
                _ => {}
            }
        }
        if !seen_open {
            depth = 1;
            seen_open = true;
        }
        if depth <= 0 {
            break;
        }
        if let Some(signature) = object_member_signature(trimmed) {
            out.push(format!("  {signature}"));
            if out.len() >= 31 {
                break;
            }
        }
    }
    if out.len() <= 2 {
        return None;
    }
    out.push("}".to_string());
    Some(out.join("\n"))
}

fn object_member_signature(line: &str) -> Option<String> {
    if line.is_empty() || line.starts_with("//") || line.starts_with("...") {
        return None;
    }
    let signature = line
        .split_once('{')
        .map(|(head, _)| format!("{} {{ ... }}", head.trim()))
        .unwrap_or_else(|| line.to_string())
        .trim()
        .trim_end_matches(',')
        .trim()
        .to_string();
    (!signature.is_empty()
        && (signature.contains('(') || signature.contains(':') || is_identifier_like(&signature)))
    .then_some(signature)
}

fn is_identifier_like(value: &str) -> bool {
    let mut chars = value.chars();
    let Some(first) = chars.next() else {
        return false;
    };
    (first.is_ascii_alphabetic() || first == '_' || first == '$')
        && chars.all(|ch| ch.is_ascii_alphanumeric() || ch == '_' || ch == '$')
}

fn bounded_definition_end(lines: &[&str], start: usize) -> usize {
    let max_end = (start + 25).min(lines.len());
    let mut brace_depth = 0isize;
    let mut seen_open_brace = false;
    for (offset, line) in lines[start..max_end].iter().enumerate() {
        for ch in line.chars() {
            match ch {
                '{' => {
                    brace_depth += 1;
                    seen_open_brace = true;
                }
                '}' => {
                    brace_depth -= 1;
                }
                _ => {}
            }
        }
        if seen_open_brace && brace_depth <= 0 {
            return start + offset + 1;
        }
    }
    max_end
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

fn resolve_route_import(parent: &Path, specifier: &str) -> Vec<PathBuf> {
    let base = parent.join(specifier);
    let mut candidates = vec![base.clone()];
    for ext in ["tsx", "ts", "jsx", "js", "css"] {
        candidates.push(base.with_extension(ext));
    }
    for ext in ["tsx", "ts", "jsx", "js", "css"] {
        candidates.push(base.join(format!("index.{ext}")));
    }
    candidates
}

fn resolve_workspace_alias_import(project_root: &Path, specifier: &str) -> Vec<PathBuf> {
    let mut candidates = Vec::new();
    for base in [
        project_root.join("src").join(specifier),
        project_root.join(specifier),
    ] {
        candidates.extend(resolve_route_import(
            base.parent().unwrap_or(project_root),
            base.file_name()
                .and_then(|name| name.to_str())
                .unwrap_or_default(),
        ));
    }
    candidates
}

fn collect_route_source_files(root: &Path) -> BTreeSet<PathBuf> {
    let mut out = BTreeSet::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            let name = entry.file_name().to_string_lossy().to_string();
            if should_skip_entry(&name) {
                continue;
            }
            let Ok(file_type) = entry.file_type() else {
                continue;
            };
            if file_type.is_dir() {
                stack.push(path);
            } else if file_type.is_file()
                && let Ok(rel) = path.strip_prefix(root)
                && is_route_source_path(rel)
            {
                out.insert(normalize_pathbuf(rel));
            }
        }
    }
    out
}

fn nextjs_project_root(root: &Path) -> Option<(PathBuf, PathBuf)> {
    if root.join("package.json").is_file()
        || root.join("src/app").is_dir()
        || root.join("app").is_dir()
    {
        return Some((root.to_path_buf(), PathBuf::new()));
    }
    let mut nested = Vec::new();
    let entries = std::fs::read_dir(root).ok()?;
    for entry in entries.flatten() {
        let Ok(file_type) = entry.file_type() else {
            continue;
        };
        if !file_type.is_dir() || entry.path().join("node_modules").is_dir() {
            continue;
        }
        if entry.path().join("package.json").is_file() {
            nested.push((entry.path(), PathBuf::from(entry.file_name())));
        }
    }
    (nested.len() == 1).then(|| nested.remove(0))
}

fn nextjs_app_router_entrypoints(project_root: &Path, project_prefix: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    for app_dir in ["src/app", "app"] {
        collect_nextjs_app_router_entrypoints(
            project_root,
            project_prefix,
            Path::new(app_dir),
            &mut out,
        );
    }
    out
}

fn collect_nextjs_app_router_entrypoints(
    project_root: &Path,
    project_prefix: &Path,
    app_dir: &Path,
    out: &mut Vec<PathBuf>,
) {
    let absolute = project_root.join(app_dir);
    if !absolute.is_dir() {
        return;
    }
    let mut stack = vec![absolute];
    while let Some(dir) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            let Ok(file_type) = entry.file_type() else {
                continue;
            };
            if file_type.is_dir() {
                stack.push(path);
                continue;
            }
            if !file_type.is_file() {
                continue;
            }
            let file_name = entry.file_name().to_string_lossy().to_ascii_lowercase();
            if !matches!(
                file_name.as_str(),
                "page.tsx"
                    | "page.ts"
                    | "page.jsx"
                    | "page.js"
                    | "layout.tsx"
                    | "layout.ts"
                    | "layout.jsx"
                    | "layout.js"
            ) {
                continue;
            }
            let Ok(project_rel) = path.strip_prefix(project_root) else {
                continue;
            };
            out.push(normalize_pathbuf(&project_prefix.join(project_rel)));
        }
    }
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

fn is_route_source_path(path: &Path) -> bool {
    path.extension().is_some_and(|ext| {
        matches!(
            ext.to_str().unwrap_or_default(),
            "tsx" | "ts" | "jsx" | "js" | "mjs" | "cjs" | "css" | "py" | "rs" | "md"
        )
    })
}

fn should_skip_entry(name: &str) -> bool {
    matches!(
        name,
        ".git" | ".anvil" | "target" | "node_modules" | ".next" | "dist" | "build"
    )
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

fn normalize_pathbuf(path: &Path) -> PathBuf {
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
    fn route_bound_unattached_ref_finds_hook_argument_canvas_missing_ref() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("src/app")).unwrap();
        std::fs::write(dir.path().join("package.json"), "{}").unwrap();
        std::fs::write(
            dir.path().join("src/app/page.tsx"),
            r#""use client";
import { useRef } from "react";
import { useGame } from "./useGame";

export default function Page() {
  const canvasRef = useRef<HTMLCanvasElement | null>(null);
  useGame(canvasRef);
  return (
    <main>
      <button>Start</button>
      <canvas width={800} height={600} />
    </main>
  );
}
"#,
        )
        .unwrap();
        std::fs::write(
            dir.path().join("src/app/useGame.ts"),
            "export function useGame(_: unknown) {}\n",
        )
        .unwrap();

        let diagnostics = route_bound_unattached_ref_diagnostics(dir.path(), "nextjs");

        assert_eq!(diagnostics.len(), 1, "{diagnostics:?}");
        let diagnostic = &diagnostics[0];
        assert_eq!(diagnostic.diagnostic, "unattached_ref:canvasRef");
        assert_eq!(diagnostic.source, "src/app/page.tsx");
        assert_eq!(diagnostic.declaration_line, 6);
        assert_eq!(diagnostic.candidate_elements[0].tag, "canvas");
        assert_eq!(diagnostic.candidate_elements[0].line, 11);
        assert_eq!(
            diagnostic.guidance,
            "attach ref={canvasRef} to the <canvas> at page.tsx:11"
        );
    }

    #[test]
    fn route_bound_unattached_ref_ignores_attached_ref() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("src/app")).unwrap();
        std::fs::write(dir.path().join("package.json"), "{}").unwrap();
        std::fs::write(
            dir.path().join("src/app/page.tsx"),
            r#""use client";
import { useRef } from "react";
import { useGame } from "./useGame";
export default function Page() {
  const canvasRef = useRef<HTMLCanvasElement | null>(null);
  useGame(canvasRef);
  return <canvas ref={canvasRef} width={800} height={600} />;
}
"#,
        )
        .unwrap();
        std::fs::write(
            dir.path().join("src/app/useGame.ts"),
            "export function useGame(_: unknown) {}\n",
        )
        .unwrap();

        assert!(route_bound_unattached_ref_diagnostics(dir.path(), "nextjs").is_empty());
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

    #[test]
    fn route_bound_closure_follows_relative_imports_from_next_app_routes() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("src/app")).unwrap();
        std::fs::create_dir_all(dir.path().join("src/components")).unwrap();
        std::fs::write(dir.path().join("package.json"), "{}").unwrap();
        std::fs::write(
            dir.path().join("src/app/page.tsx"),
            r#"import SpaceInvaders from "../components/SpaceInvaders"; export default function Page(){ return <SpaceInvaders/>; }"#,
        )
        .unwrap();
        std::fs::write(
            dir.path().join("src/components/SpaceInvaders.tsx"),
            "export default function SpaceInvaders(){ return <canvas/>; }",
        )
        .unwrap();

        let closure = route_bound_closure(dir.path(), "nextjs");

        assert!(closure.contains(Path::new("src/app/page.tsx")));
        assert!(closure.contains(Path::new("src/components/SpaceInvaders.tsx")));
    }

    #[test]
    fn route_bound_closure_resolves_workspace_alias_imports() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("src/app")).unwrap();
        std::fs::create_dir_all(dir.path().join("src/components")).unwrap();
        std::fs::write(dir.path().join("package.json"), "{}").unwrap();
        std::fs::write(
            dir.path().join("src/app/page.tsx"),
            r#"import SpaceInvaders from "@/components/SpaceInvaders"; export default function Page(){ return <SpaceInvaders/>; }"#,
        )
        .unwrap();
        std::fs::write(
            dir.path().join("src/components/SpaceInvaders.tsx"),
            "export default function SpaceInvaders(){ return <canvas/>; }",
        )
        .unwrap();

        let closure = route_bound_closure(dir.path(), "nextjs");

        assert!(closure.contains(Path::new("src/components/SpaceInvaders.tsx")));
    }

    #[test]
    fn route_bound_closure_respects_nested_next_project_prefix() {
        let dir = tempfile::tempdir().unwrap();
        let app = dir.path().join("space-invaders");
        std::fs::create_dir_all(app.join("src/app")).unwrap();
        std::fs::create_dir_all(app.join("src/components")).unwrap();
        std::fs::write(app.join("package.json"), "{}").unwrap();
        std::fs::write(
            app.join("src/app/page.tsx"),
            r#"import SpaceInvaders from "@/components/SpaceInvaders"; export default function Page(){ return <SpaceInvaders/>; }"#,
        )
        .unwrap();
        std::fs::write(
            app.join("src/components/SpaceInvaders.tsx"),
            "export default function SpaceInvaders(){ return <canvas/>; }",
        )
        .unwrap();

        let closure = route_bound_closure(dir.path(), "nextjs");

        assert!(closure.contains(Path::new("space-invaders/src/app/page.tsx")));
        assert!(closure.contains(Path::new("space-invaders/src/components/SpaceInvaders.tsx")));
    }

    #[test]
    fn route_bound_closure_fail_open_keeps_unparsable_imported_file() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("src/app")).unwrap();
        std::fs::write(dir.path().join("package.json"), "{}").unwrap();
        std::fs::write(
            dir.path().join("src/app/page.tsx"),
            r#"import Broken from "./Broken"; export default function Page(){ return <Broken/>; }"#,
        )
        .unwrap();
        std::fs::write(
            dir.path().join("src/app/Broken.tsx"),
            "import ???\nexport default function Broken(){ return <button/>; }",
        )
        .unwrap();

        let closure = route_bound_closure(dir.path(), "nextjs");

        assert!(closure.contains(Path::new("src/app/Broken.tsx")));
    }
}
