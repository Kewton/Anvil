//! Typed summary for `package.json` setup artifacts.
//!
//! This module owns JSON-manifest interpretation. Callers such as setup
//! validation, evidence binding, and verifier detection consume the summary
//! instead of each adding their own `package.json` string checks.

use std::collections::{BTreeMap, BTreeSet};

use serde_json::Value;

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub(super) struct PackageManifestSummary {
    pub(super) scripts: BTreeMap<String, String>,
    pub(super) packages: BTreeSet<String>,
    pub(super) module_type: Option<PackageModuleType>,
}

impl PackageManifestSummary {
    pub(super) fn has_script(&self, name: &str) -> bool {
        self.scripts
            .get(&name.to_ascii_lowercase())
            .is_some_and(|script| !script.trim().is_empty())
    }

    pub(super) fn test_script_runner_binding(&self) -> Option<PackageTestScriptRunnerBinding> {
        let script = self.scripts.get("test")?;
        let required = package_runner_required_by_test_script(script)?;
        let bound = self.packages.contains(&required);
        Some(PackageTestScriptRunnerBinding {
            reference: format!("scripts.test.runner:{required}"),
            required_runner: required,
            bound,
            candidates: self.packages.iter().cloned().collect(),
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct PackageTestScriptRunnerBinding {
    pub(super) reference: String,
    pub(super) required_runner: String,
    pub(super) bound: bool,
    pub(super) candidates: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum PackageModuleType {
    EsModule,
    CommonJs,
}

impl PackageModuleType {
    fn from_package_json_type(value: &str) -> Option<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "module" => Some(Self::EsModule),
            "commonjs" => Some(Self::CommonJs),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum PackageManifestParseError {
    InvalidJson,
    TopLevelNotObject,
    ScriptsNotObject,
}

impl PackageManifestParseError {
    pub(super) fn message(&self) -> &'static str {
        match self {
            Self::InvalidJson => "not valid JSON",
            Self::TopLevelNotObject => "top-level value must be an object",
            Self::ScriptsNotObject => "scripts must be an object when declared",
        }
    }
}

pub(super) fn parse_package_manifest_summary(
    source: &str,
) -> Result<PackageManifestSummary, PackageManifestParseError> {
    let value = serde_json::from_str::<Value>(source)
        .map_err(|_| PackageManifestParseError::InvalidJson)?;
    let Value::Object(object) = value else {
        return Err(PackageManifestParseError::TopLevelNotObject);
    };

    let scripts = match object.get("scripts") {
        Some(Value::Object(scripts)) => scripts
            .iter()
            .filter_map(|(name, value)| {
                value
                    .as_str()
                    .map(str::trim)
                    .filter(|script| !script.is_empty())
                    .map(|script| (name.to_ascii_lowercase(), script.to_string()))
            })
            .collect::<BTreeMap<_, _>>(),
        Some(_) => return Err(PackageManifestParseError::ScriptsNotObject),
        None => BTreeMap::new(),
    };

    let module_type = object
        .get("type")
        .and_then(Value::as_str)
        .and_then(PackageModuleType::from_package_json_type);

    let mut packages = BTreeSet::new();
    for section in [
        "dependencies",
        "devDependencies",
        "peerDependencies",
        "optionalDependencies",
    ] {
        if let Some(Value::Object(deps)) = object.get(section) {
            packages.extend(deps.keys().map(|name| name.to_ascii_lowercase()));
        }
    }

    Ok(PackageManifestSummary {
        scripts,
        packages,
        module_type,
    })
}

pub(super) fn package_manifest_readiness(
    source: &str,
) -> Result<PackageManifestSummary, PackageManifestParseError> {
    parse_package_manifest_summary(source)
}

fn package_runner_required_by_test_script(script: &str) -> Option<String> {
    let tokens = shell_like_words(script);
    let first = tokens.first()?.to_ascii_lowercase();
    if first == "npm" && npm_test_script_is_recursive(&tokens) {
        return Some("scripts.test".to_string());
    }
    if first == "node" {
        return node_command_package_runner_requirement(&tokens[1..]);
    }
    node_test_runner_package_name(&first)
}

fn npm_test_script_is_recursive(tokens: &[String]) -> bool {
    matches!(
        tokens,
        [npm, test, ..] if npm.eq_ignore_ascii_case("npm") && test.eq_ignore_ascii_case("test")
    ) || matches!(
        tokens,
        [npm, run, test, ..]
            if npm.eq_ignore_ascii_case("npm")
                && run.eq_ignore_ascii_case("run")
                && test.eq_ignore_ascii_case("test")
    )
}

fn node_command_package_runner_requirement(tokens: &[String]) -> Option<String> {
    let mut idx = 0;
    while idx < tokens.len() {
        let token = tokens[idx].as_str();
        if token == "--test" {
            return None;
        }
        if let Some(name) = package_name_from_node_modules_path(token) {
            return Some(name);
        }
        if token.starts_with('-') {
            idx += 1;
            continue;
        }
        return None;
    }
    None
}

fn node_test_runner_package_name(token: &str) -> Option<String> {
    let candidate = token
        .trim()
        .trim_start_matches("./")
        .trim_start_matches("node_modules/.bin/")
        .to_ascii_lowercase();
    [
        "jest", "vitest", "mocha", "ava", "tap", "tape", "uvu", "jasmine",
    ]
    .contains(&candidate.as_str())
    .then_some(candidate)
}

fn package_name_from_node_modules_path(token: &str) -> Option<String> {
    let normalized = token.replace('\\', "/");
    let after = normalized.split("node_modules/").nth(1)?;
    if after.starts_with(".bin/") {
        return node_test_runner_package_name(after.trim_start_matches(".bin/"));
    }
    if after.starts_with('@') {
        let mut parts = after.split('/');
        let scope = parts.next()?;
        let name = parts.next()?;
        return Some(format!("{scope}/{name}").to_ascii_lowercase());
    }
    after
        .split('/')
        .next()
        .filter(|name| !name.is_empty())
        .map(str::to_ascii_lowercase)
}

fn shell_like_words(script: &str) -> Vec<String> {
    let mut words = Vec::new();
    let mut current = String::new();
    let mut quote: Option<char> = None;
    let mut escaped = false;
    for ch in script.chars() {
        if escaped {
            current.push(ch);
            escaped = false;
            continue;
        }
        if ch == '\\' {
            escaped = true;
            continue;
        }
        if let Some(active) = quote {
            if ch == active {
                quote = None;
            } else {
                current.push(ch);
            }
            continue;
        }
        if matches!(ch, '"' | '\'') {
            quote = Some(ch);
            continue;
        }
        if ch.is_whitespace() {
            if !current.is_empty() {
                words.push(std::mem::take(&mut current));
            }
            continue;
        }
        current.push(ch);
    }
    if !current.is_empty() {
        words.push(current);
    }
    words
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_scripts_and_dependencies() {
        let summary = parse_package_manifest_summary(
            r#"{
              "type": "module",
              "scripts": { "Test": "node --test", "build": "vite build" },
              "dependencies": { "react": "latest" },
              "devDependencies": { "vitest": "^1.0.0" }
            }"#,
        )
        .unwrap();
        assert!(summary.has_script("test"));
        assert!(summary.has_script("TEST"));
        assert!(summary.has_script("build"));
        assert!(summary.packages.contains("react"));
        assert!(summary.packages.contains("vitest"));
        assert_eq!(summary.module_type, Some(PackageModuleType::EsModule));
    }

    #[test]
    fn test_script_runner_binding_detects_missing_direct_runner_package() {
        let summary =
            parse_package_manifest_summary(r#"{"scripts":{"test":"vitest run"}}"#).unwrap();
        let binding = summary
            .test_script_runner_binding()
            .expect("runner binding");

        assert_eq!(binding.required_runner, "vitest");
        assert_eq!(binding.reference, "scripts.test.runner:vitest");
        assert!(!binding.bound);
        assert!(binding.candidates.is_empty());
    }

    #[test]
    fn test_script_runner_binding_binds_declared_runner_package() {
        let summary = parse_package_manifest_summary(
            r#"{"scripts":{"test":"jest"},"devDependencies":{"jest":"^30.0.0"}}"#,
        )
        .unwrap();
        let binding = summary
            .test_script_runner_binding()
            .expect("runner binding");

        assert_eq!(binding.required_runner, "jest");
        assert!(binding.bound);
        assert_eq!(binding.candidates, vec!["jest".to_string()]);
    }

    #[test]
    fn test_script_runner_binding_detects_node_modules_runner_path() {
        let summary = parse_package_manifest_summary(
            r#"{"scripts":{"test":"node --experimental-vm-modules node_modules/jest/bin/jest.js"}}"#,
        )
        .unwrap();
        let binding = summary
            .test_script_runner_binding()
            .expect("runner binding");

        assert_eq!(binding.required_runner, "jest");
        assert!(!binding.bound);
    }

    #[test]
    fn native_node_test_script_has_no_package_runner_requirement() {
        let summary =
            parse_package_manifest_summary(r#"{"scripts":{"test":"node --test"}}"#).unwrap();

        assert!(summary.test_script_runner_binding().is_none());
    }

    #[test]
    fn missing_scripts_is_ready_but_not_bindable() {
        let summary = package_manifest_readiness(r#"{"name":"app"}"#).unwrap();
        assert!(!summary.has_script("test"));
        assert_eq!(summary.module_type, None);
    }

    #[test]
    fn parses_commonjs_module_type() {
        let summary = package_manifest_readiness(r#"{"type":"commonjs"}"#).unwrap();
        assert_eq!(summary.module_type, Some(PackageModuleType::CommonJs));
    }

    #[test]
    fn rejects_invalid_json() {
        let err = parse_package_manifest_summary(r#"{"scripts":"#).unwrap_err();
        assert_eq!(err.message(), "not valid JSON");
    }

    #[test]
    fn rejects_non_object_root() {
        let err = parse_package_manifest_summary("[]").unwrap_err();
        assert_eq!(err.message(), "top-level value must be an object");
    }

    #[test]
    fn rejects_non_object_scripts() {
        let err = parse_package_manifest_summary(r#"{"scripts":"oops"}"#).unwrap_err();
        assert_eq!(err.message(), "scripts must be an object when declared");
    }
}
