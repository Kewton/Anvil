use std::path::{Path, PathBuf};

use crate::agent::text_tokens;

use super::super::WorkIntent;
use super::super::profile::ProfileSnapshot;

pub(in crate::agent::minimal_step_runner) fn generation_rules(intent: WorkIntent) -> &'static str {
    match intent {
        WorkIntent::Create => {
            "- Profile nextjs/create: preserve a real Next.js app contract. Include next/react/react-dom dependencies, keep scripts.build as next build, and end with a build verification phase. Put a separate kind:\"setup\" dependency step before any npm run build verification when node_modules is not already present; the setup instruction may install dependencies, but verify must not contain npm install. If dependency setup is not allowed or cannot run, stop with dependency_missing instead of claiming build success. If you use Tailwind utility classes or @tailwind directives, include tailwindcss/postcss/autoprefixer and create tailwind.config.* plus postcss.config.*; otherwise use plain CSS and do not write Tailwind utility classes.\n"
        }
        WorkIntent::Fix => {
            "- Profile nextjs/fix: preserve the existing Next.js structure and verifier integrity. Do not weaken build/test scripts to make a failing verifier pass.\n"
        }
        WorkIntent::Investigate => {
            "- Profile nextjs/investigate: inspect the existing app and produce a concrete report. Do not modify source unless the user explicitly asks for fixes.\n"
        }
        _ => {
            "- Profile nextjs: preserve the existing Next.js structure when present. Keep package.json as a Next.js package, keep app/ or pages/ entrypoints, and end with a build verification phase.\n"
        }
    }
}

pub(in crate::agent::minimal_step_runner) fn runtime_contract(intent: WorkIntent) -> &'static str {
    match intent {
        WorkIntent::Create => {
            "- Preserve the workspace as a real Next.js app.\n- Keep next/react/react-dom dependencies in package.json.\n- Keep scripts.build as next build; do not replace it with echo/skip/no-op commands.\n- If npm run build cannot run because dependencies are not installed, report dependency_missing or install dependencies when the step explicitly allows it; do not fake success.\n- If a requested port requirement exists, keep dev/start scripts on that requested port.\n- If using Tailwind utility classes or @tailwind directives, keep the Tailwind toolchain complete: tailwindcss/postcss/autoprefixer dependencies, tailwind.config.*, and postcss.config.*. Otherwise use plain CSS.\n- Do not set tsconfig rootDir to ./src in a way that excludes app/.\n- If source imports use @/* aliases, tsconfig.json must map @/* under compilerOptions.paths; otherwise use relative imports."
        }
        WorkIntent::Fix => {
            "- Preserve the existing Next.js app structure.\n- Keep next/react/react-dom dependencies when already present.\n- Keep scripts.build as next build when already present; do not weaken build/test scripts to hide failures.\n- If npm run build cannot run because dependencies are missing, report dependency_missing or use the existing dependency workflow; do not fake success.\n- Do not set tsconfig rootDir to ./src in a way that excludes app/.\n- If source imports use @/* aliases, tsconfig.json must map @/* under compilerOptions.paths; otherwise use relative imports."
        }
        WorkIntent::Investigate => {
            "- Preserve the existing Next.js app unchanged unless the phase explicitly asks for fixes.\n- Produce concrete findings from inspected files and commands.\n- Separate observed facts from hypotheses.\n- Do not weaken package scripts or test/build checks while investigating."
        }
        _ => {
            "- Preserve the workspace as a Next.js app when one exists.\n- Do not convert package.json to a standalone TypeScript/Node project.\n- Keep next/react/react-dom dependencies when already present.\n- Keep scripts.build as next build when already present.\n- If a requested port requirement exists, keep dev/start scripts on that requested port.\n- Keep styling toolchains internally consistent: @tailwind directives require a real Tailwind dependency and config.\n- Do not set tsconfig rootDir to ./src in a way that excludes app/.\n- If source imports use @/* aliases, tsconfig.json must map @/* under compilerOptions.paths; otherwise use relative imports."
        }
    }
}

pub(in crate::agent::minimal_step_runner) fn snapshot(work_root: &Path) -> ProfileSnapshot {
    let mut lines = Vec::new();
    for path in [
        "package.json",
        "tsconfig.json",
        "app/page.tsx",
        "app/layout.tsx",
        "pages/index.tsx",
        "next.config.js",
    ] {
        if work_root.join(path).exists() {
            lines.push(format!("Existing file: {path}"));
        }
    }
    if let Some(summary) = package_json_summary(&work_root.join("package.json")) {
        lines.push(summary);
    }
    ProfileSnapshot::new(lines, Vec::new())
}

pub(in crate::agent::minimal_step_runner) fn probe_port(
    work_root: &Path,
    requested_port: Option<u16>,
) -> u16 {
    requested_port
        .or_else(|| {
            let raw = std::fs::read_to_string(work_root.join("package.json")).ok()?;
            text_tokens::script_declared_port_from_package_json(&raw)
        })
        .unwrap_or(3000)
}

pub(in crate::agent::minimal_step_runner) fn verify(
    work_root: &Path,
    _intent: WorkIntent,
    requested_port: Option<u16>,
    failures: &mut Vec<String>,
) {
    let package_path = work_root.join("package.json");
    let app_dir_exists = work_root.join("app").is_dir() || work_root.join("pages").is_dir();
    if package_path.exists() && app_dir_exists {
        let Ok(raw) = std::fs::read_to_string(&package_path) else {
            failures
                .push("contract_violation: package.json exists but could not be read".to_string());
            return;
        };
        if raw.contains("\"next\"") {
            if let Ok(json) = serde_json::from_str::<serde_json::Value>(&raw) {
                let build = json
                    .pointer("/scripts/build")
                    .and_then(|v| v.as_str())
                    .unwrap_or("");
                if !build.is_empty() && build != "next build" {
                    failures.push(format!(
                        "contract_violation: package.json build script is no longer `next build`: {build}"
                    ));
                }
                let dev = json
                    .pointer("/scripts/dev")
                    .and_then(|v| v.as_str())
                    .unwrap_or("");
                let start = json
                    .pointer("/scripts/start")
                    .and_then(|v| v.as_str())
                    .unwrap_or("");
                verify_requested_port_script("dev", dev, "next dev", requested_port, failures);
                verify_requested_port_script(
                    "start",
                    start,
                    "next start",
                    requested_port,
                    failures,
                );
            }
        } else {
            failures.push(
                "contract_violation: package.json no longer contains next dependency".to_string(),
            );
        }
        verify_tailwind_contract(work_root, &raw, failures);
    }
    let tsconfig_path = work_root.join("tsconfig.json");
    if tsconfig_path.exists()
        && app_dir_exists
        && let Ok(raw) = std::fs::read_to_string(tsconfig_path)
    {
        if raw.contains("\"rootDir\"") && raw.contains("\"./src\"") {
            failures.push(
                "contract_violation: tsconfig.json rootDir ./src excludes Next.js app/ files"
                    .to_string(),
            );
        }
        if source_uses_at_alias(work_root) && tsconfig_missing_at_alias_paths(&raw) {
            failures.push(
                "contract_violation: Next.js source imports @/* aliases but tsconfig.json lacks compilerOptions.paths mapping for @/*"
                    .to_string(),
            );
        }
    }
}

fn verify_requested_port_script(
    script_name: &str,
    script: &str,
    expected_command: &str,
    requested_port: Option<u16>,
    failures: &mut Vec<String>,
) {
    let Some(requested_port) = requested_port else {
        return;
    };
    if script.trim().is_empty() {
        return;
    }
    if !script.contains(expected_command) {
        failures.push(format!(
            "contract_violation: package.json {script_name} script must use `{expected_command}` on requested port {requested_port}: {script}"
        ));
        return;
    }
    if text_tokens::requested_port(script) != Some(requested_port) {
        failures.push(format!(
            "contract_violation: package.json {script_name} script must stay on requested port {requested_port}: {script}"
        ));
    }
}

fn verify_tailwind_contract(work_root: &Path, package_json_raw: &str, failures: &mut Vec<String>) {
    if !workspace_uses_tailwind_directives(work_root) {
        return;
    }
    if !package_has_dependency(package_json_raw, "tailwindcss") {
        failures.push(
            "contract_violation: CSS uses @tailwind directives but package.json lacks tailwindcss dependency"
                .to_string(),
        );
    }
    if !package_has_dependency(package_json_raw, "postcss") {
        failures.push(
            "contract_violation: CSS uses @tailwind directives but package.json lacks postcss dependency"
                .to_string(),
        );
    }
    if !package_has_dependency(package_json_raw, "autoprefixer") {
        failures.push(
            "contract_violation: CSS uses @tailwind directives but package.json lacks autoprefixer dependency"
                .to_string(),
        );
    }
    if !has_any_config(
        work_root,
        &[
            "tailwind.config.js",
            "tailwind.config.cjs",
            "tailwind.config.mjs",
            "tailwind.config.ts",
        ],
    ) {
        failures.push(
            "contract_violation: CSS uses @tailwind directives but missing tailwind.config.*"
                .to_string(),
        );
    }
    if !has_any_config(
        work_root,
        &[
            "postcss.config.js",
            "postcss.config.cjs",
            "postcss.config.mjs",
        ],
    ) {
        failures.push(
            "contract_violation: CSS uses @tailwind directives but missing postcss.config.*"
                .to_string(),
        );
    }
}

fn workspace_uses_tailwind_directives(work_root: &Path) -> bool {
    discover_style_files(work_root, work_root, 0)
        .into_iter()
        .any(|relative| {
            std::fs::read_to_string(work_root.join(relative))
                .is_ok_and(|raw| raw.contains("@tailwind "))
        })
}

fn package_has_dependency(raw: &str, name: &str) -> bool {
    let Ok(json) = serde_json::from_str::<serde_json::Value>(raw) else {
        return false;
    };
    ["dependencies", "devDependencies", "peerDependencies"]
        .into_iter()
        .any(|section| {
            json.get(section)
                .and_then(|value| value.get(name))
                .is_some()
        })
}

fn has_any_config(work_root: &Path, names: &[&str]) -> bool {
    names.iter().any(|name| work_root.join(name).is_file())
}

fn tsconfig_missing_at_alias_paths(raw: &str) -> bool {
    let Ok(json) = serde_json::from_str::<serde_json::Value>(raw) else {
        return true;
    };
    let compiler = json
        .get("compilerOptions")
        .and_then(|value| value.as_object());
    let has_alias = compiler
        .and_then(|map| map.get("paths"))
        .and_then(|value| value.get("@/*"))
        .and_then(|value| value.as_array())
        .is_some_and(|items| {
            items
                .iter()
                .filter_map(|item| item.as_str())
                .any(|item| item == "./*" || item == "*")
        });
    !has_alias
}

fn source_uses_at_alias(work_root: &Path) -> bool {
    for relative in discover_source_files(work_root, work_root, 0) {
        let path = work_root.join(relative);
        if let Ok(raw) = std::fs::read_to_string(path)
            && (raw.contains("from '@/") || raw.contains("from \"@/"))
        {
            return true;
        }
    }
    false
}

fn discover_source_files(root: &Path, current: &Path, depth: usize) -> Vec<PathBuf> {
    if depth > 4 {
        return Vec::new();
    }
    let Ok(entries) = std::fs::read_dir(current) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if name.starts_with('.') || name == "node_modules" || name == "target" {
            continue;
        }
        if path.is_dir() {
            out.extend(discover_source_files(root, &path, depth + 1));
        } else if is_source_file(&path)
            && let Ok(relative) = path.strip_prefix(root)
        {
            out.push(relative.to_path_buf());
        }
    }
    out
}

fn discover_style_files(root: &Path, current: &Path, depth: usize) -> Vec<PathBuf> {
    if depth > 4 {
        return Vec::new();
    }
    let Ok(entries) = std::fs::read_dir(current) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if name.starts_with('.') || name == "node_modules" || name == "target" {
            continue;
        }
        if path.is_dir() {
            out.extend(discover_style_files(root, &path, depth + 1));
        } else if path
            .extension()
            .and_then(|value| value.to_str())
            .is_some_and(|ext| ext.eq_ignore_ascii_case("css"))
            && let Ok(relative) = path.strip_prefix(root)
        {
            out.push(relative.to_path_buf());
        }
    }
    out
}

fn is_source_file(path: &Path) -> bool {
    let Some(ext) = path.extension().and_then(|value| value.to_str()) else {
        return false;
    };
    matches!(
        ext.to_ascii_lowercase().as_str(),
        "ts" | "tsx" | "js" | "jsx" | "mjs" | "cjs"
    )
}

fn package_json_summary(path: &Path) -> Option<String> {
    let raw = std::fs::read_to_string(path).ok()?;
    let json = serde_json::from_str::<serde_json::Value>(&raw).ok()?;
    let build = json
        .pointer("/scripts/build")
        .and_then(|value| value.as_str())
        .unwrap_or("none");
    let dev = json
        .pointer("/scripts/dev")
        .and_then(|value| value.as_str())
        .unwrap_or("none");
    let deps = json
        .get("dependencies")
        .and_then(|value| value.as_object())
        .map(|map| map.keys().take(8).cloned().collect::<Vec<_>>().join(", "))
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| "none".to_string());
    Some(format!(
        "package.json scripts: build=`{build}`, dev=`{dev}`; dependencies: {deps}"
    ))
}
