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
}

impl PackageManifestSummary {
    pub(super) fn has_script(&self, name: &str) -> bool {
        self.scripts
            .get(&name.to_ascii_lowercase())
            .is_some_and(|script| !script.trim().is_empty())
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

    Ok(PackageManifestSummary { scripts, packages })
}

pub(super) fn package_manifest_readiness(
    source: &str,
) -> Result<PackageManifestSummary, PackageManifestParseError> {
    parse_package_manifest_summary(source)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_scripts_and_dependencies() {
        let summary = parse_package_manifest_summary(
            r#"{
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
    }

    #[test]
    fn missing_scripts_is_ready_but_not_bindable() {
        let summary = package_manifest_readiness(r#"{"name":"app"}"#).unwrap();
        assert!(!summary.has_script("test"));
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
