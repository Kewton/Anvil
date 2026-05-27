use std::path::{Path, PathBuf};

pub(crate) fn missing_local_import_symbols(
    work_root: &Path,
    source: &str,
    max_file_bytes: u64,
) -> Vec<(String, String)> {
    let mut missing = Vec::new();
    for line in source.lines() {
        let stripped = strip_inline_comment(line);
        let trimmed = stripped.trim();
        let Some(rest) = trimmed.strip_prefix("from ") else {
            continue;
        };
        let Some((module, imports)) = rest.split_once(" import ") else {
            continue;
        };
        let module = module.trim();
        let Some(module_path) = local_module_path(work_root, module, max_file_bytes) else {
            continue;
        };
        if imports.trim().starts_with('*') {
            continue;
        }
        let Ok(module_source) = std::fs::read_to_string(&module_path) else {
            continue;
        };
        for (imported, _) in from_import_entries(imports) {
            if !identifier_is_safe(imported) {
                continue;
            }
            if module_source_defines_name(&module_source, imported)
                || local_submodule_exists(work_root, module, imported, max_file_bytes)
            {
                continue;
            }
            missing.push((module.to_string(), imported.to_string()));
        }
    }
    missing
}

pub(crate) fn missing_local_import_modules(
    work_root: &Path,
    source: &str,
    max_file_bytes: u64,
) -> Vec<String> {
    let mut missing = Vec::new();
    for line in source.lines() {
        let stripped = strip_inline_comment(line);
        let trimmed = stripped.trim();
        let Some(rest) = trimmed.strip_prefix("from ") else {
            continue;
        };
        let Some((module, _imports)) = rest.split_once(" import ") else {
            continue;
        };
        let module = module.trim();
        if !module_name_is_safe(module)
            || local_module_path(work_root, module, max_file_bytes).is_some()
            || !local_module_root_exists(work_root, module, max_file_bytes)
        {
            continue;
        }
        missing.push(module.to_string());
    }
    missing.sort();
    missing.dedup();
    missing
}

pub(crate) fn imported_scalar_attribute_assumptions(
    work_root: &Path,
    source: &str,
    max_file_bytes: u64,
) -> Vec<String> {
    let mut invalid = Vec::new();
    for line in source.lines() {
        let stripped = strip_inline_comment(line);
        let trimmed = stripped.trim();
        let Some(rest) = trimmed.strip_prefix("from ") else {
            continue;
        };
        let Some((module, imports)) = rest.split_once(" import ") else {
            continue;
        };
        let module = module.trim();
        let Some(module_path) = local_module_path(work_root, module, max_file_bytes) else {
            continue;
        };
        if imports.trim().starts_with('*') {
            continue;
        }
        let Ok(module_source) = std::fs::read_to_string(&module_path) else {
            continue;
        };
        for (imported, local_name) in from_import_entries(imports) {
            if !identifier_is_safe(imported)
                || !identifier_is_safe(local_name)
                || !module_source_definitively_binds_scalar(&module_source, imported)
            {
                continue;
            }
            for attr in imported_name_attribute_usages(source, local_name) {
                invalid.push(format!("{module}.{imported}.{attr}"));
            }
        }
    }
    invalid
}

fn from_import_entries(imports: &str) -> Vec<(&str, &str)> {
    let mut entries = Vec::new();
    for raw in imports.split(',') {
        let cleaned =
            raw.trim_matches(|ch: char| ch == '(' || ch == ')' || ch == '\\' || ch.is_whitespace());
        let parts = cleaned.split_whitespace().collect::<Vec<_>>();
        match parts.as_slice() {
            [imported, "as", alias] => entries.push((*imported, *alias)),
            [imported] => entries.push((*imported, *imported)),
            _ => {}
        }
    }
    entries
}

fn local_module_path(work_root: &Path, module: &str, max_file_bytes: u64) -> Option<PathBuf> {
    if !module_name_is_safe(module) {
        return None;
    }
    let root = std::fs::canonicalize(work_root).ok()?;
    let relative = module_relative_path(module)?;
    let file_candidate = root.join(&relative).with_extension("py");
    if let Some(path) = safe_local_module_candidate(&root, file_candidate, max_file_bytes) {
        return Some(path);
    }
    let package_candidate = root.join(relative).join("__init__.py");
    safe_local_module_candidate(&root, package_candidate, max_file_bytes)
}

fn local_submodule_exists(work_root: &Path, module: &str, name: &str, max_file_bytes: u64) -> bool {
    if !module_name_is_safe(module) || !identifier_is_safe(name) {
        return false;
    }
    let combined = format!("{module}.{name}");
    local_module_path(work_root, &combined, max_file_bytes).is_some()
}

fn local_module_root_exists(work_root: &Path, module: &str, max_file_bytes: u64) -> bool {
    let Some(root_name) = module.split('.').next() else {
        return false;
    };
    if !identifier_is_safe(root_name) {
        return false;
    }
    let Ok(root) = std::fs::canonicalize(work_root) else {
        return false;
    };
    let file_candidate = root.join(root_name).with_extension("py");
    if safe_local_module_candidate(&root, file_candidate, max_file_bytes).is_some() {
        return true;
    }
    let dir_candidate = root.join(root_name);
    if !dir_candidate.is_dir() {
        return false;
    }
    let Ok(canonical) = std::fs::canonicalize(dir_candidate) else {
        return false;
    };
    canonical.strip_prefix(root).is_ok()
}

fn safe_local_module_candidate(
    root: &Path,
    candidate: PathBuf,
    max_file_bytes: u64,
) -> Option<PathBuf> {
    if !candidate.is_file() {
        return None;
    }
    let metadata = std::fs::metadata(&candidate).ok()?;
    if metadata.len() > max_file_bytes {
        return None;
    }
    let canonical = std::fs::canonicalize(candidate).ok()?;
    canonical.strip_prefix(root).ok()?;
    Some(canonical)
}

fn module_relative_path(module: &str) -> Option<PathBuf> {
    let mut relative = PathBuf::new();
    for part in module.split('.') {
        if !identifier_is_safe(part) {
            return None;
        }
        relative.push(part);
    }
    Some(relative)
}

fn module_name_is_safe(module: &str) -> bool {
    !module.is_empty() && !module.starts_with('.') && module.split('.').all(identifier_is_safe)
}

fn module_source_defines_name(source: &str, name: &str) -> bool {
    for line in source.lines() {
        if line.chars().next().is_some_and(char::is_whitespace) {
            continue;
        }
        let stripped = strip_inline_comment(line);
        let trimmed = stripped.trim();
        if trimmed.is_empty() {
            continue;
        }
        if trimmed.starts_with(&format!("def {name}("))
            || trimmed.starts_with(&format!("async def {name}("))
            || trimmed.starts_with(&format!("class {name}("))
            || trimmed.starts_with(&format!("class {name}:"))
        {
            return true;
        }
        if top_level_assignment_defines_name(trimmed, name)
            || top_level_import_defines_name(trimmed, name)
        {
            return true;
        }
    }
    false
}

fn top_level_assignment_defines_name(line: &str, name: &str) -> bool {
    let Some((assignment_head, _)) = line.split_once('=') else {
        return false;
    };
    let binding_head = assignment_head
        .split_once(':')
        .map(|(head, _)| head)
        .unwrap_or(assignment_head)
        .trim();
    binding_head == name
}

fn module_source_definitively_binds_scalar(source: &str, name: &str) -> bool {
    for line in source.lines() {
        if line.chars().next().is_some_and(char::is_whitespace) {
            continue;
        }
        let stripped = strip_inline_comment(line);
        let trimmed = stripped.trim();
        let Some((assignment_head, expression)) = trimmed.split_once('=') else {
            continue;
        };
        let binding_head = assignment_head
            .split_once(':')
            .map(|(head, _)| head)
            .unwrap_or(assignment_head)
            .trim();
        if binding_head == name && expression_is_scalar_literal(expression.trim()) {
            return true;
        }
    }
    false
}

fn expression_is_scalar_literal(expression: &str) -> bool {
    let expression = expression.trim().trim_end_matches(',');
    if matches!(expression, "True" | "False" | "None") {
        return true;
    }
    if (expression.starts_with('"') && expression.ends_with('"'))
        || (expression.starts_with('\'') && expression.ends_with('\''))
    {
        return true;
    }
    let normalized = expression.replace('_', "");
    normalized.parse::<i64>().is_ok() || normalized.parse::<f64>().is_ok()
}

fn imported_name_attribute_usages(source: &str, name: &str) -> Vec<String> {
    let mut attrs = Vec::new();
    let pattern = format!("{name}.");
    for line in source.lines() {
        let stripped = strip_inline_comment(line);
        if stripped.trim_start().starts_with("from ") {
            continue;
        }
        for (idx, _) in stripped.match_indices(&pattern) {
            if stripped[..idx]
                .chars()
                .next_back()
                .is_some_and(|ch| identifier_char(ch) || ch == '.')
            {
                continue;
            }
            let attr_start = idx + pattern.len();
            let attr = stripped[attr_start..]
                .chars()
                .take_while(|ch| identifier_char(*ch))
                .collect::<String>();
            if identifier_is_safe(&attr) {
                attrs.push(attr);
            }
        }
    }
    attrs
}

fn top_level_import_defines_name(line: &str, name: &str) -> bool {
    if let Some(rest) = line.strip_prefix("import ") {
        return rest.split(',').any(|raw| {
            let parts: Vec<_> = raw.split_whitespace().collect();
            match parts.as_slice() {
                [module, "as", alias] => *alias == name && module_name_is_safe(module),
                [module] => module
                    .split('.')
                    .next()
                    .is_some_and(|root| root == name && module_name_is_safe(module)),
                _ => false,
            }
        });
    }
    if let Some(rest) = line.strip_prefix("from ") {
        let Some((_module, imports)) = rest.split_once(" import ") else {
            return false;
        };
        return imports.split(',').any(|raw| {
            let parts: Vec<_> = raw.split_whitespace().collect();
            match parts.as_slice() {
                [imported, "as", alias] => *alias == name && identifier_is_safe(imported),
                [imported] => *imported == name && identifier_is_safe(imported),
                _ => false,
            }
        });
    }
    false
}

fn strip_inline_comment(line: &str) -> String {
    line.split_once('#')
        .map(|(head, _)| head)
        .unwrap_or(line)
        .to_string()
}

fn identifier_is_safe(name: &str) -> bool {
    let mut chars = name.chars();
    let Some(first) = chars.next() else {
        return false;
    };
    (first == '_' || first.is_ascii_alphabetic())
        && chars.all(|ch| ch == '_' || ch.is_ascii_alphanumeric())
}

fn identifier_char(ch: char) -> bool {
    ch == '_' || ch.is_ascii_alphanumeric()
}

#[cfg(test)]
mod tests {
    use std::fs;

    use super::*;

    const MAX_FILE_BYTES: u64 = 256 * 1024;

    #[test]
    fn missing_local_import_symbols_detects_absent_name() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("app.py"), "def create_item():\n    pass\n").unwrap();

        let missing = missing_local_import_symbols(
            dir.path(),
            "from app import create_item, missing_item\n",
            MAX_FILE_BYTES,
        );

        assert_eq!(
            missing,
            vec![("app".to_string(), "missing_item".to_string())]
        );
    }

    #[test]
    fn missing_local_import_modules_requires_local_root() {
        let dir = tempfile::tempdir().unwrap();
        fs::create_dir(dir.path().join("app")).unwrap();

        let missing = missing_local_import_modules(
            dir.path(),
            "from app.missing import create_item\nfrom external_lib.foo import bar\n",
            MAX_FILE_BYTES,
        );

        assert_eq!(missing, vec!["app.missing".to_string()]);
    }

    #[test]
    fn scalar_attribute_assumptions_detects_scalar_member_access() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("settings.py"), "COUNTER = 0\n").unwrap();

        let invalid = imported_scalar_attribute_assumptions(
            dir.path(),
            "from settings import COUNTER as counter\ncounter.value = 1\n",
            MAX_FILE_BYTES,
        );

        assert_eq!(invalid, vec!["settings.COUNTER.value".to_string()]);
    }
}
