use anyhow::{anyhow, bail, Context};
use serde::Serialize;
use serde_json::Value;

pub fn pretty<T: Serialize>(value: &T) -> anyhow::Result<String> {
    Ok(serde_json::to_string_pretty(value)?)
}

pub fn parse_value(name: &str, raw: &str) -> anyhow::Result<Value> {
    serde_json::from_str(raw).with_context(|| format!("failed to parse json for {name}"))
}

pub fn validate_schema_value(
    schema_name: &str,
    schema: &Value,
    instance_name: &str,
    instance: &Value,
) -> anyhow::Result<()> {
    let validator = jsonschema::validator_for(schema)
        .with_context(|| format!("failed to compile json schema {schema_name}"))?;
    let errors: Vec<_> = validator
        .iter_errors(instance)
        .map(|error| error.to_string())
        .collect();

    if errors.is_empty() {
        return Ok(());
    }

    bail!(
        "json schema validation failed for {instance_name} against {schema_name}: {}",
        errors.join("; ")
    );
}

pub fn validate_embedded_json(
    schema_name: &str,
    schema_raw: &str,
    instance_name: &str,
    instance_raw: &str,
) -> anyhow::Result<()> {
    let schema = parse_value(schema_name, schema_raw)?;
    let instance = parse_value(instance_name, instance_raw)?;
    validate_schema_value(schema_name, &schema, instance_name, &instance)
}

pub fn validate_serializable<T: Serialize>(
    schema_name: &str,
    schema_raw: &str,
    instance_name: &str,
    value: &T,
) -> anyhow::Result<()> {
    let schema = parse_value(schema_name, schema_raw)?;
    let instance = serde_json::to_value(value)
        .map_err(|error| anyhow!(error))
        .with_context(|| format!("failed to encode {instance_name} for schema validation"))?;
    validate_schema_value(schema_name, &schema, instance_name, &instance)
}
