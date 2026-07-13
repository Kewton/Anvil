use std::sync::OnceLock;

use toml::value::Table;

use super::{
    CapabilityKind, CapabilitySpec, CatalogError, InternalCapability, ParamSpec, ParamType,
    ResolvedCapability, required_path, required_u16,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DataInternalCheck {
    ResultsSchema,
    Reconciliation,
    ClaimsBinding,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProbeCapability {
    Pipeline { entry: String, timeout_seconds: u16 },
    DataRerunConsistency { entry: String, timeout_seconds: u16 },
}

static PIPELINE_PARAMS: [ParamSpec; 2] = [
    ParamSpec {
        name: "entry",
        param_type: ParamType::Path,
        required: true,
        default: None,
    },
    ParamSpec {
        name: "timeout_seconds",
        param_type: ParamType::U16,
        required: true,
        default: None,
    },
];

static REGISTRY: [CapabilitySpec; 5] = [
    CapabilitySpec {
        id: "data_results_schema",
        kind: CapabilityKind::InternalCheck,
        params: &super::NO_PARAMS,
        description: "Validate output/results.json against the fixed data schema.",
    },
    CapabilitySpec {
        id: "data_reconciliation",
        kind: CapabilityKind::InternalCheck,
        params: &super::NO_PARAMS,
        description: "Check input, used, and reasoned exclusion row accounting.",
    },
    CapabilitySpec {
        id: "data_claims_binding",
        kind: CapabilityKind::InternalCheck,
        params: &super::NO_PARAMS,
        description: "Bind every report numeric claim to output/results.json values.",
    },
    CapabilitySpec {
        id: "pipeline_probe",
        kind: CapabilityKind::Probe,
        params: &PIPELINE_PARAMS,
        description: "Run a Python standard-library pipeline with bounded capture.",
    },
    CapabilitySpec {
        id: "data_rerun_consistency",
        kind: CapabilityKind::Probe,
        params: &PIPELINE_PARAMS,
        description: "Rerun a data pipeline and compare the complete results document.",
    },
];

pub(super) fn registry() -> &'static [CapabilitySpec] {
    &REGISTRY
}

pub(super) fn combined_registry(base: &'static [CapabilitySpec]) -> &'static [CapabilitySpec] {
    static COMBINED: OnceLock<Vec<CapabilitySpec>> = OnceLock::new();
    COMBINED.get_or_init(|| base.iter().chain(registry()).copied().collect())
}

pub(super) fn resolve(
    spec: &CapabilitySpec,
    params: &Table,
) -> Result<ResolvedCapability, CatalogError> {
    match spec.id {
        "data_results_schema" => Ok(ResolvedCapability::Internal(InternalCapability::Data(
            DataInternalCheck::ResultsSchema,
        ))),
        "data_reconciliation" => Ok(ResolvedCapability::Internal(InternalCapability::Data(
            DataInternalCheck::Reconciliation,
        ))),
        "data_claims_binding" => Ok(ResolvedCapability::Internal(InternalCapability::Data(
            DataInternalCheck::ClaimsBinding,
        ))),
        "pipeline_probe" => {
            let (entry, timeout_seconds) = pipeline_params(spec, params)?;
            Ok(ResolvedCapability::Probe(ProbeCapability::Pipeline {
                entry,
                timeout_seconds,
            }))
        }
        "data_rerun_consistency" => {
            let (entry, timeout_seconds) = pipeline_params(spec, params)?;
            Ok(ResolvedCapability::Probe(
                ProbeCapability::DataRerunConsistency {
                    entry,
                    timeout_seconds,
                },
            ))
        }
        _ => unreachable!("registry id without resolver: {}", spec.id),
    }
}

fn pipeline_params(spec: &CapabilitySpec, params: &Table) -> Result<(String, u16), CatalogError> {
    let entry = required_path(spec, params, "entry")?;
    if !entry.starts_with("pipeline/") || !entry.ends_with(".py") {
        return Err(invalid(
            spec,
            "entry",
            "expected a .py path under pipeline/",
        ));
    }
    let timeout_seconds = required_u16(spec, params, "timeout_seconds")?;
    if timeout_seconds == 0 {
        return Err(invalid(
            spec,
            "timeout_seconds",
            "must be greater than zero",
        ));
    }
    Ok((entry, timeout_seconds))
}

fn invalid(spec: &CapabilitySpec, parameter: &str, reason: &str) -> CatalogError {
    CatalogError::InvalidParameter {
        id: spec.id.to_string(),
        parameter: parameter.to_string(),
        reason: reason.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use toml::Value;

    use super::*;

    fn params(entry: &str, timeout_seconds: i64) -> Table {
        Table::from_iter([
            ("entry".to_string(), Value::String(entry.to_string())),
            (
                "timeout_seconds".to_string(),
                Value::Integer(timeout_seconds),
            ),
        ])
    }

    #[test]
    fn data_internal_checks_resolve_without_free_form_parameters() {
        for (id, expected) in [
            ("data_results_schema", DataInternalCheck::ResultsSchema),
            ("data_reconciliation", DataInternalCheck::Reconciliation),
            ("data_claims_binding", DataInternalCheck::ClaimsBinding),
        ] {
            assert_eq!(
                super::super::resolve(id, &Table::new()).unwrap(),
                ResolvedCapability::Internal(InternalCapability::Data(expected))
            );
        }
    }

    #[test]
    fn data_probes_resolve_to_typed_adapters() {
        assert_eq!(
            super::super::resolve("pipeline_probe", &params("pipeline/main.py", 30)).unwrap(),
            ResolvedCapability::Probe(ProbeCapability::Pipeline {
                entry: "pipeline/main.py".to_string(),
                timeout_seconds: 30,
            })
        );
        assert_eq!(
            super::super::resolve("data_rerun_consistency", &params("pipeline/main.py", 30))
                .unwrap(),
            ResolvedCapability::Probe(ProbeCapability::DataRerunConsistency {
                entry: "pipeline/main.py".to_string(),
                timeout_seconds: 30,
            })
        );
    }

    #[test]
    fn data_probe_params_reject_zero_timeout_and_non_pipeline_entry() {
        for params in [
            params("pipeline/main.py", 0),
            params("scripts/main.py", 30),
            params("pipeline/main.sh", 30),
        ] {
            assert!(matches!(
                super::super::resolve("pipeline_probe", &params),
                Err(CatalogError::InvalidParameter { .. })
            ));
        }
    }
}
