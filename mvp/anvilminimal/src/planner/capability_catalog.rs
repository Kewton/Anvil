use std::fmt;

use toml::Value;
use toml::value::Table;

use crate::planner::verify;
use crate::tools::path_guard::validate_workspace_relative;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CapabilityKind {
    ShellCheck,
    InternalCheck,
    Probe,
}

impl CapabilityKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::ShellCheck => "ShellCheck",
            Self::InternalCheck => "InternalCheck",
            Self::Probe => "Probe",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ParamType {
    String,
    U16,
    Path,
    PathList,
    Enum(&'static [&'static str]),
}

impl ParamType {
    fn schema_label(self) -> String {
        match self {
            Self::String => "string".to_string(),
            Self::U16 => "u16".to_string(),
            Self::Path => "path".to_string(),
            Self::PathList => "[path]".to_string(),
            Self::Enum(values) => format!("enum[{}]", values.join(",")),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ParamSpec {
    pub name: &'static str,
    pub param_type: ParamType,
    pub required: bool,
    pub default: Option<&'static str>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CapabilitySpec {
    pub id: &'static str,
    pub kind: CapabilityKind,
    pub params: &'static [ParamSpec],
    pub description: &'static str,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ResolvedCapability {
    ShellCheck(String),
    Internal(InternalCapability),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InternalCapability {
    ScaffoldFilesPresent { files: Vec<String> },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CatalogError {
    UnknownId(String),
    UnknownParameter {
        id: String,
        parameter: String,
    },
    MissingParameter {
        id: String,
        parameter: String,
    },
    TypeMismatch {
        id: String,
        parameter: String,
        expected: String,
    },
    InvalidParameter {
        id: String,
        parameter: String,
        reason: String,
    },
    ProbeBindingUnimplemented {
        id: String,
    },
}

impl fmt::Display for CatalogError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnknownId(id) => write!(f, "unknown capability id `{id}`"),
            Self::UnknownParameter { id, parameter } => {
                write!(f, "unknown parameter `{parameter}` for capability `{id}`")
            }
            Self::MissingParameter { id, parameter } => {
                write!(f, "missing parameter `{parameter}` for capability `{id}`")
            }
            Self::TypeMismatch {
                id,
                parameter,
                expected,
            } => write!(
                f,
                "parameter `{parameter}` for capability `{id}` must be {expected}"
            ),
            Self::InvalidParameter {
                id,
                parameter,
                reason,
            } => write!(
                f,
                "invalid parameter `{parameter}` for capability `{id}`: {reason}"
            ),
            Self::ProbeBindingUnimplemented { id } => {
                write!(
                    f,
                    "probe capability `{id}` is registered but not bindable yet"
                )
            }
        }
    }
}

impl std::error::Error for CatalogError {}

static NO_PARAMS: [ParamSpec; 0] = [];
static PORT_PARAMS: [ParamSpec; 1] = [ParamSpec {
    name: "port",
    param_type: ParamType::U16,
    required: true,
    default: None,
}];
static PATTERN_PARAMS: [ParamSpec; 1] = [ParamSpec {
    name: "pattern",
    param_type: ParamType::String,
    required: true,
    default: None,
}];
static HOOK_ATTRIBUTE_VALUES: [&str; 2] = ["action", "state"];
static HOOK_ATTRIBUTE_PARAMS: [ParamSpec; 3] = [
    ParamSpec {
        name: "attribute",
        param_type: ParamType::Enum(&HOOK_ATTRIBUTE_VALUES),
        required: true,
        default: None,
    },
    ParamSpec {
        name: "value",
        param_type: ParamType::String,
        required: true,
        default: None,
    },
    ParamSpec {
        name: "path",
        param_type: ParamType::Path,
        required: true,
        default: None,
    },
];
static SCAFFOLD_FILES_PARAMS: [ParamSpec; 1] = [ParamSpec {
    name: "files",
    param_type: ParamType::PathList,
    required: true,
    default: None,
}];

static REGISTRY: [CapabilitySpec; 7] = [
    CapabilitySpec {
        id: "package_json_port_script",
        kind: CapabilityKind::ShellCheck,
        params: &PORT_PARAMS,
        description: "Check package.json dev/start scripts for the requested port.",
    },
    CapabilitySpec {
        id: "package_json_script_matches",
        kind: CapabilityKind::ShellCheck,
        params: &PATTERN_PARAMS,
        description: "Check a recognized package.json script pattern.",
    },
    CapabilitySpec {
        id: "hook_attribute_present",
        kind: CapabilityKind::ShellCheck,
        params: &HOOK_ATTRIBUTE_PARAMS,
        description: "Check route-bound hook attributes without quote or brace coupling.",
    },
    CapabilitySpec {
        id: "next_build_verify",
        kind: CapabilityKind::ShellCheck,
        params: &NO_PARAMS,
        description: "Run the existing build verifier command.",
    },
    CapabilitySpec {
        id: "scaffold_files_present",
        kind: CapabilityKind::InternalCheck,
        params: &SCAFFOLD_FILES_PARAMS,
        description: "Check that required scaffold files are present.",
    },
    CapabilitySpec {
        id: "browser_readiness",
        kind: CapabilityKind::Probe,
        params: &NO_PARAMS,
        description: "Registered browser readiness probe.",
    },
    CapabilitySpec {
        id: "browser_interaction",
        kind: CapabilityKind::Probe,
        params: &NO_PARAMS,
        description: "Registered browser interaction probe.",
    },
];

pub fn registry() -> &'static [CapabilitySpec] {
    &REGISTRY
}

pub fn resolve(id: &str, params: &Table) -> Result<ResolvedCapability, CatalogError> {
    let spec = registry()
        .iter()
        .find(|candidate| candidate.id == id)
        .ok_or_else(|| CatalogError::UnknownId(id.to_string()))?;
    validate_param_contract(spec, params)?;

    match spec.id {
        "package_json_port_script" => {
            let port = required_u16(spec, params, "port")?;
            Ok(ResolvedCapability::ShellCheck(
                verify::package_json_port_script_check_command(&port.to_string()),
            ))
        }
        "package_json_script_matches" => {
            let pattern = required_string(spec, params, "pattern")?;
            let command = verify::package_json_script_check_command(pattern).ok_or_else(|| {
                CatalogError::InvalidParameter {
                    id: spec.id.to_string(),
                    parameter: "pattern".to_string(),
                    reason: "unsupported package.json script pattern".to_string(),
                }
            })?;
            Ok(ResolvedCapability::ShellCheck(command))
        }
        "hook_attribute_present" => {
            let attribute = required_enum(spec, params, "attribute")?;
            let value = required_string(spec, params, "value")?;
            let path = required_path(spec, params, "path")?;
            let command = verify::hook_attribute_present_check_command(attribute, value, &path)
                .ok_or_else(|| CatalogError::InvalidParameter {
                    id: spec.id.to_string(),
                    parameter: "attribute".to_string(),
                    reason: "unsupported hook attribute assertion".to_string(),
                })?;
            Ok(ResolvedCapability::ShellCheck(command))
        }
        "next_build_verify" => Ok(ResolvedCapability::ShellCheck("npm run build".to_string())),
        "scaffold_files_present" => Ok(ResolvedCapability::Internal(
            InternalCapability::ScaffoldFilesPresent {
                files: required_path_list(spec, params, "files")?,
            },
        )),
        "browser_readiness" | "browser_interaction" => {
            Err(CatalogError::ProbeBindingUnimplemented { id: id.to_string() })
        }
        _ => unreachable!("registry id without resolver: {}", spec.id),
    }
}

fn validate_param_contract(spec: &CapabilitySpec, params: &Table) -> Result<(), CatalogError> {
    for key in params.keys() {
        if !spec.params.iter().any(|param| param.name == key) {
            return Err(CatalogError::UnknownParameter {
                id: spec.id.to_string(),
                parameter: key.to_string(),
            });
        }
    }
    for param in spec.params {
        if param.required && !params.contains_key(param.name) {
            return Err(CatalogError::MissingParameter {
                id: spec.id.to_string(),
                parameter: param.name.to_string(),
            });
        }
    }
    Ok(())
}

fn required_value<'a>(
    spec: &CapabilitySpec,
    params: &'a Table,
    name: &str,
) -> Result<&'a Value, CatalogError> {
    params
        .get(name)
        .ok_or_else(|| CatalogError::MissingParameter {
            id: spec.id.to_string(),
            parameter: name.to_string(),
        })
}

fn required_string<'a>(
    spec: &CapabilitySpec,
    params: &'a Table,
    name: &str,
) -> Result<&'a str, CatalogError> {
    match required_value(spec, params, name)? {
        Value::String(value) => Ok(value),
        _ => Err(type_mismatch(spec, name, ParamType::String)),
    }
}

fn required_u16(spec: &CapabilitySpec, params: &Table, name: &str) -> Result<u16, CatalogError> {
    match required_value(spec, params, name)? {
        Value::Integer(value) if (0..=u16::MAX as i64).contains(value) => Ok(*value as u16),
        _ => Err(type_mismatch(spec, name, ParamType::U16)),
    }
}

fn required_enum<'a>(
    spec: &CapabilitySpec,
    params: &'a Table,
    name: &str,
) -> Result<&'a str, CatalogError> {
    let value = required_string(spec, params, name)?;
    let Some(param) = spec.params.iter().find(|param| param.name == name) else {
        return Err(CatalogError::MissingParameter {
            id: spec.id.to_string(),
            parameter: name.to_string(),
        });
    };
    let ParamType::Enum(values) = param.param_type else {
        return Err(type_mismatch(spec, name, param.param_type));
    };
    if values.iter().any(|allowed| allowed == &value) {
        Ok(value)
    } else {
        Err(CatalogError::InvalidParameter {
            id: spec.id.to_string(),
            parameter: name.to_string(),
            reason: format!("expected one of {}", values.join(", ")),
        })
    }
}

fn required_path(
    spec: &CapabilitySpec,
    params: &Table,
    name: &str,
) -> Result<String, CatalogError> {
    let value = required_string(spec, params, name)?;
    validate_path_param(spec, name, value)?;
    Ok(value.to_string())
}

fn required_path_list(
    spec: &CapabilitySpec,
    params: &Table,
    name: &str,
) -> Result<Vec<String>, CatalogError> {
    let Value::Array(values) = required_value(spec, params, name)? else {
        return Err(type_mismatch(spec, name, ParamType::PathList));
    };
    if values.is_empty() {
        return Err(CatalogError::InvalidParameter {
            id: spec.id.to_string(),
            parameter: name.to_string(),
            reason: "path list may not be empty".to_string(),
        });
    }
    values
        .iter()
        .map(|value| match value {
            Value::String(path) => {
                validate_path_param(spec, name, path)?;
                Ok(path.to_string())
            }
            _ => Err(type_mismatch(spec, name, ParamType::PathList)),
        })
        .collect()
}

fn validate_path_param(spec: &CapabilitySpec, name: &str, path: &str) -> Result<(), CatalogError> {
    validate_workspace_relative(path).map_err(|err| CatalogError::InvalidParameter {
        id: spec.id.to_string(),
        parameter: name.to_string(),
        reason: err.to_string(),
    })
}

fn type_mismatch(spec: &CapabilitySpec, name: &str, expected: ParamType) -> CatalogError {
    CatalogError::TypeMismatch {
        id: spec.id.to_string(),
        parameter: name.to_string(),
        expected: expected.schema_label(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::planner::profile::build_oracle_for_command;
    use crate::planner::profiles::nextjs;

    #[test]
    fn registry_contract_snapshot_is_stable() {
        assert_eq!(
            registry_contract_snapshot(),
            "\
package_json_port_script | ShellCheck
  - port: u16 required default=-
package_json_script_matches | ShellCheck
  - pattern: string required default=-
hook_attribute_present | ShellCheck
  - attribute: enum[action,state] required default=-
  - value: string required default=-
  - path: path required default=-
next_build_verify | ShellCheck
scaffold_files_present | InternalCheck
  - files: [path] required default=-
browser_readiness | Probe
browser_interaction | Probe
"
        );
    }

    #[test]
    fn resolve_rejects_unknown_missing_extra_and_badly_typed_params() {
        assert!(matches!(
            resolve("does_not_exist", &Table::new()),
            Err(CatalogError::UnknownId(_))
        ));
        assert!(matches!(
            resolve("package_json_port_script", &Table::new()),
            Err(CatalogError::MissingParameter { parameter, .. }) if parameter == "port"
        ));

        let mut extra = table_with("port", Value::Integer(3011));
        extra.insert(
            "command".to_string(),
            Value::String("echo free shell".to_string()),
        );
        assert!(matches!(
            resolve("package_json_port_script", &extra),
            Err(CatalogError::UnknownParameter { parameter, .. }) if parameter == "command"
        ));

        assert!(matches!(
            resolve(
                "package_json_port_script",
                &table_with("port", Value::String("3011".to_string()))
            ),
            Err(CatalogError::TypeMismatch { parameter, .. }) if parameter == "port"
        ));

        let mut bad_enum = hook_params("href", "primary", "src/app/page.tsx");
        assert!(matches!(
            resolve("hook_attribute_present", &bad_enum),
            Err(CatalogError::InvalidParameter { parameter, .. }) if parameter == "attribute"
        ));
        bad_enum.insert("attribute".to_string(), Value::String("action".to_string()));
        bad_enum.insert("path".to_string(), Value::String("../page.tsx".to_string()));
        assert!(matches!(
            resolve("hook_attribute_present", &bad_enum),
            Err(CatalogError::InvalidParameter { parameter, .. }) if parameter == "path"
        ));
    }

    #[test]
    fn shell_check_resolvers_match_existing_command_generators() {
        assert_eq!(
            shell(
                "package_json_port_script",
                table_with("port", Value::Integer(3011))
            ),
            verify::package_json_port_script_check_command("3011")
        );
        assert_eq!(
            shell(
                "package_json_script_matches",
                table_with("pattern", Value::String("next build".to_string()))
            ),
            verify::package_json_script_check_command("next build").unwrap()
        );
        assert_eq!(
            shell(
                "hook_attribute_present",
                hook_params("action", "primary", "src/app/page.tsx")
            ),
            verify::hook_attribute_present_check_command("action", "primary", "src/app/page.tsx")
                .unwrap()
        );
        assert_eq!(
            shell(
                "hook_attribute_present",
                hook_params("state", "", "src/app/page.tsx")
            ),
            verify::hook_attribute_present_check_command("state", "", "src/app/page.tsx").unwrap()
        );
        let build = shell("next_build_verify", Table::new());
        assert_eq!(build, "npm run build");
        assert!(build_oracle_for_command(None, &build).is_some());
    }

    #[test]
    fn scaffold_files_resolves_to_internal_adapter() {
        let files = vec![
            Value::String("package.json".to_string()),
            Value::String("src/app/page.tsx".to_string()),
        ];
        assert_eq!(
            resolve(
                "scaffold_files_present",
                &table_with("files", Value::Array(files))
            )
            .unwrap(),
            ResolvedCapability::Internal(InternalCapability::ScaffoldFilesPresent {
                files: vec!["package.json".to_string(), "src/app/page.tsx".to_string()]
            })
        );
    }

    #[test]
    fn probe_capabilities_are_registered_but_not_bindable_yet() {
        for id in ["browser_readiness", "browser_interaction"] {
            assert!(registry().iter().any(|spec| spec.id == id));
            assert!(matches!(
                resolve(id, &Table::new()),
                Err(CatalogError::ProbeBindingUnimplemented { id: actual }) if actual == id
            ));
        }
    }

    #[test]
    fn nextjs_port_script_template_uses_catalog_command_byte_for_byte() {
        let dir = tempfile::tempdir().unwrap();
        let prompt = "Original ultra goal: Build a browser app on port 4321\n\
Phase id: package-scripts\n\
Phase task: Configure package.json dev/start scripts for the requested port";

        let template = nextjs::deterministic_step_plan(prompt, dir.path(), prompt).unwrap();
        let verify = &template.plan.steps[1].verify;
        let expected = shell(
            "package_json_port_script",
            table_with("port", Value::Integer(4321)),
        );

        assert_eq!(template.template_id, "nextjs-port-scripts");
        assert_eq!(verify, &vec![expected.clone()]);
        assert_eq!(
            verify[0],
            verify::package_json_port_script_check_command("4321")
        );
    }

    fn registry_contract_snapshot() -> String {
        let mut out = String::new();
        for spec in registry() {
            out.push_str(spec.id);
            out.push_str(" | ");
            out.push_str(spec.kind.as_str());
            out.push('\n');
            for param in spec.params {
                out.push_str("  - ");
                out.push_str(param.name);
                out.push_str(": ");
                out.push_str(&param.param_type.schema_label());
                out.push(' ');
                out.push_str(if param.required {
                    "required"
                } else {
                    "optional"
                });
                out.push_str(" default=");
                out.push_str(param.default.unwrap_or("-"));
                out.push('\n');
            }
        }
        out
    }

    fn shell(id: &str, params: Table) -> String {
        match resolve(id, &params).unwrap() {
            ResolvedCapability::ShellCheck(command) => command,
            other => panic!("expected ShellCheck, got {other:?}"),
        }
    }

    fn table_with(name: &str, value: Value) -> Table {
        let mut table = Table::new();
        table.insert(name.to_string(), value);
        table
    }

    fn hook_params(attribute: &str, value: &str, path: &str) -> Table {
        let mut table = Table::new();
        table.insert(
            "attribute".to_string(),
            Value::String(attribute.to_string()),
        );
        table.insert("value".to_string(), Value::String(value.to_string()));
        table.insert("path".to_string(), Value::String(path.to_string()));
        table
    }
}
