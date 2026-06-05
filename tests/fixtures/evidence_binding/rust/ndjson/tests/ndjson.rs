// Issue #989 fixture: imports `ndjson`, but the lib is renamed to `nd_json`.
// Inert fixture data (never compiled).
use ndjson::parse_lines;

#[test]
fn parses_lines() {
    let rows = parse_lines("{\"a\":1}\n{\"a\":2}\n");
    assert_eq!(rows.len(), 2);
}
