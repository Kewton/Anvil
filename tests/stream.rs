use anvil::models::stream::{NdjsonStreamParser, SseStreamParser, parse_ndjson_events};
use serde_json::Value;

#[test]
fn parse_ndjson_events_accepts_split_chunks() {
    let mut parser = NdjsonStreamParser::default();

    let first = parser.push::<Value>(r#"{"message":{"content":"hel"#);
    assert!(first.is_empty());

    let second = parser.push::<Value>(
        "lo\"},\"done\":false}\n{\"message\":{\"content\":\" world\"},\"done\":false}\n",
    );
    let third = parser.push::<Value>("{\"done\":true}\n");

    assert_eq!(second.len(), 2);
    assert_eq!(third.len(), 1);
    assert!(parser.finish::<Value>().is_empty());
}

#[test]
fn parse_ndjson_events_rejects_malformed_json() {
    let err = parse_ndjson_events::<Value>("{not-json}\n").unwrap_err();
    assert!(format!("{err}").contains("invalid NDJSON"));
}

#[test]
fn parse_sse_events_accepts_data_payloads() {
    let mut parser = SseStreamParser::default();

    let first = parser.push::<Value>("data: {\"choices\":[{\"delta\":{\"content\":\"hel");
    assert!(first.is_empty());
    let second = parser.push::<Value>(
        "lo\"}}]}\n\ndata: {\"choices\":[{\"delta\":{\"content\":\" world\"}}]}\n\n",
    );

    assert_eq!(second.len(), 2);
    assert!(parser.finish::<Value>().is_empty());
}
