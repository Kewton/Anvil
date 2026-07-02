//! Shared Cargo manifest summary used by setup validation and evidence binding.
//!
//! This is a typed boundary for the small subset of `Cargo.toml` Anvil needs to
//! reason about before invoking Cargo. It intentionally does not infer the user
//! objective or repair strategy.

use std::collections::BTreeSet;

use super::generated_test_guard::{parse_toml_string_value, strip_toml_comment};

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(super) struct CargoManifestSummary {
    pub(super) package_declared: bool,
    pub(super) package_name: Option<String>,
    pub(super) workspace_declared: bool,
    pub(super) lib_declared: bool,
    pub(super) lib_name: Option<String>,
    pub(super) bin_names: BTreeSet<String>,
    pub(super) package_target_section_declared: bool,
    pub(super) dependencies: BTreeSet<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum CargoManifestParseError {
    MalformedTableHeader,
    PackageNameMalformed,
}

impl CargoManifestParseError {
    pub(super) fn message(self) -> &'static str {
        match self {
            CargoManifestParseError::MalformedTableHeader => "malformed table header",
            CargoManifestParseError::PackageNameMalformed => "[package] name is malformed",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum CargoManifestReadinessError {
    Parse(CargoManifestParseError),
    TargetSectionRequiresPackage,
    PackageNameMissing,
    MissingPackageOrWorkspace,
}

impl CargoManifestReadinessError {
    pub(super) fn message(self) -> &'static str {
        match self {
            CargoManifestReadinessError::Parse(err) => err.message(),
            CargoManifestReadinessError::TargetSectionRequiresPackage => {
                "target sections require a [package] section"
            }
            CargoManifestReadinessError::PackageNameMissing => {
                "[package] does not declare a non-empty name"
            }
            CargoManifestReadinessError::MissingPackageOrWorkspace => {
                "missing [package] or [workspace] section"
            }
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ManifestSection {
    Package,
    Lib,
    Bin,
    Dependencies,
    Other,
}

pub(super) fn normalize_cargo_ident(name: &str) -> String {
    name.replace('-', "_")
}

pub(super) fn parse_cargo_manifest_summary(
    manifest_source: &str,
) -> Result<CargoManifestSummary, CargoManifestParseError> {
    let mut summary = CargoManifestSummary::default();
    let mut section = ManifestSection::Other;

    for raw_line in manifest_source.lines() {
        let line = strip_toml_comment(raw_line).trim();
        if line.is_empty() {
            continue;
        }
        if line.starts_with('[') && !line.ends_with(']') {
            return Err(CargoManifestParseError::MalformedTableHeader);
        }
        if let Some(inner) = line.strip_prefix("[[").and_then(|s| s.strip_suffix("]]")) {
            section = classify_array_table_header(inner.trim(), &mut summary);
            continue;
        }
        if let Some(inner) = line.strip_prefix('[').and_then(|s| s.strip_suffix(']')) {
            section = classify_table_header(inner.trim(), &mut summary);
            continue;
        }

        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        let key = key.trim();
        match section {
            ManifestSection::Package if key == "name" => {
                let Some(value) = parse_toml_string_value(value.trim()) else {
                    return Err(CargoManifestParseError::PackageNameMalformed);
                };
                summary.package_name = (!value.is_empty()).then_some(value);
            }
            ManifestSection::Lib if key == "name" => {
                summary.lib_name = parse_toml_string_value(value.trim()).filter(|s| !s.is_empty());
            }
            ManifestSection::Bin if key == "name" => {
                if let Some(name) = parse_toml_string_value(value.trim()).filter(|s| !s.is_empty())
                {
                    summary.bin_names.insert(name);
                }
            }
            ManifestSection::Dependencies if !key.is_empty() => {
                summary
                    .dependencies
                    .insert(normalize_cargo_ident(strip_quotes(key)));
            }
            _ => {}
        }
    }

    Ok(summary)
}

pub(super) fn cargo_manifest_readiness(
    manifest_source: &str,
) -> Result<CargoManifestSummary, CargoManifestReadinessError> {
    let summary = parse_cargo_manifest_summary(manifest_source)
        .map_err(CargoManifestReadinessError::Parse)?;

    if summary.package_target_section_declared && !summary.package_declared {
        return Err(CargoManifestReadinessError::TargetSectionRequiresPackage);
    }
    if summary.package_declared && summary.package_name.is_none() {
        return Err(CargoManifestReadinessError::PackageNameMissing);
    }
    if !summary.package_declared && !summary.workspace_declared {
        return Err(CargoManifestReadinessError::MissingPackageOrWorkspace);
    }

    Ok(summary)
}

fn classify_array_table_header(
    header: &str,
    summary: &mut CargoManifestSummary,
) -> ManifestSection {
    match header {
        "bin" => {
            summary.package_target_section_declared = true;
            ManifestSection::Bin
        }
        "bench" | "test" => {
            summary.package_target_section_declared = true;
            ManifestSection::Other
        }
        _ => ManifestSection::Other,
    }
}

fn classify_table_header(header: &str, summary: &mut CargoManifestSummary) -> ManifestSection {
    match header {
        "package" => {
            summary.package_declared = true;
            ManifestSection::Package
        }
        "workspace" => {
            summary.workspace_declared = true;
            ManifestSection::Other
        }
        "lib" => {
            summary.lib_declared = true;
            summary.package_target_section_declared = true;
            ManifestSection::Lib
        }
        "dependencies" | "dev-dependencies" | "build-dependencies" => ManifestSection::Dependencies,
        other => {
            if let Some(dep) = dependency_subtable_key(other) {
                summary
                    .dependencies
                    .insert(normalize_cargo_ident(strip_quotes(dep)));
                ManifestSection::Other
            } else if other.ends_with(".dependencies") {
                ManifestSection::Dependencies
            } else {
                ManifestSection::Other
            }
        }
    }
}

fn dependency_subtable_key(header: &str) -> Option<&str> {
    for prefix in ["dependencies.", "dev-dependencies.", "build-dependencies."] {
        if let Some(rest) = header.strip_prefix(prefix) {
            return Some(rest);
        }
    }
    None
}

fn strip_quotes(value: &str) -> &str {
    value.trim().trim_matches('"').trim_matches('\'')
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_package_lib_bin_and_dependencies() {
        let summary = parse_cargo_manifest_summary(
            "[package]\nname = \"demo-app\"\n\n[lib]\nname = \"demo_lib\"\n\n[[bin]]\nname = \"demo-cli\"\n\n[dependencies]\nserde = \"1\"\n[dev-dependencies.pretty_assertions]\nversion = \"1\"\n",
        )
        .unwrap();

        assert_eq!(summary.package_name.as_deref(), Some("demo-app"));
        assert!(summary.lib_declared);
        assert_eq!(summary.lib_name.as_deref(), Some("demo_lib"));
        assert!(summary.bin_names.contains("demo-cli"));
        assert!(summary.dependencies.contains("serde"));
        assert!(summary.dependencies.contains("pretty_assertions"));
    }

    #[test]
    fn readiness_accepts_workspace_only_manifest() {
        let summary = cargo_manifest_readiness("[workspace]\nmembers = []\n").unwrap();
        assert!(summary.workspace_declared);
        assert_eq!(summary.package_name, None);
    }

    #[test]
    fn readiness_rejects_target_sections_without_package() {
        let err = cargo_manifest_readiness("[[bin]]\nname = \"cli\"\n").unwrap_err();
        assert_eq!(
            err,
            CargoManifestReadinessError::TargetSectionRequiresPackage
        );
    }

    #[test]
    fn readiness_rejects_package_without_name() {
        let err = cargo_manifest_readiness("[package]\nversion = \"0.1.0\"\n").unwrap_err();
        assert_eq!(err, CargoManifestReadinessError::PackageNameMissing);
    }

    #[test]
    fn parse_rejects_malformed_package_name() {
        let err = parse_cargo_manifest_summary("[package]\nname = demo\n").unwrap_err();
        assert_eq!(err, CargoManifestParseError::PackageNameMalformed);
    }
}
