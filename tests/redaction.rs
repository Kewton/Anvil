use std::collections::BTreeMap;

use anvil::state::audit::{redact_map, redact_value};

#[test]
fn redacts_sensitive_scalar_values() {
    assert_eq!(redact_value("api_token", "secret-123"), "[REDACTED]");
    assert_eq!(redact_value("normal", "visible"), "visible");
}

#[test]
fn redacts_sensitive_fields_in_map() {
    let mut input = BTreeMap::new();
    input.insert("query".to_string(), "rg main".to_string());
    input.insert("authorization".to_string(), "Bearer token".to_string());

    let output = redact_map(&input);

    assert_eq!(output["query"], "rg main");
    assert_eq!(output["authorization"], "[REDACTED]");
}
