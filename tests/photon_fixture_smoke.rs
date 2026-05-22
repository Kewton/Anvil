use std::collections::HashSet;

use anvil::photon::eval::parse_evaluate_response;
use anvil::photon::prompt::render_context_pack;
use anvil::photon::schema::{
    ContextPackRequest, ContextPackResponse, EvaluateRequest, EvaluateResponse,
};

fn empty_blocked() -> HashSet<String> {
    HashSet::new()
}

fn fixtures_dir() -> std::path::PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/photon")
}

/// F1: context_pack_request.json を ContextPackRequest にデシリアライズし必須フィールドを検証する。
#[test]
fn f1_context_pack_request_loads() {
    let raw = std::fs::read_to_string(fixtures_dir().join("context_pack_request.json")).unwrap();
    let req: ContextPackRequest = serde_json::from_str(&raw).unwrap();
    assert_eq!(req.0["schema_version"], 1);
    assert!(req.0["repo_path"].as_str().is_some_and(|s| !s.is_empty()));
    assert!(req.0["touched_files"].as_array().is_some());
    assert!(req.0["recent_tool_summary"].as_array().is_some());
}

/// F2: context_pack_response.json を render_context_pack に通し有効なセクションが返ることを検証する。
#[test]
fn f2_context_pack_response_renders() {
    let raw = std::fs::read_to_string(fixtures_dir().join("context_pack_response.json")).unwrap();
    let resp: ContextPackResponse = serde_json::from_str(&raw).unwrap();
    let (result, _stats) = render_context_pack(&resp, &empty_blocked());
    assert!(result.is_some(), "expected Some(section) but got None");
    let section = result.unwrap();
    assert!(
        section.starts_with("Photon Context:"),
        "section={section:?}"
    );
}

/// F3: evaluate_request.json を EvaluateRequest にデシリアライズし必須フィールドを検証する。
#[test]
fn f3_evaluate_request_loads() {
    let raw = std::fs::read_to_string(fixtures_dir().join("evaluate_request.json")).unwrap();
    let req: EvaluateRequest = serde_json::from_str(&raw).unwrap();
    assert!(req.0["session_id"].as_str().is_some_and(|s| !s.is_empty()));
    assert!(req.0["turn_index"].is_number());
}

/// F4: evaluate_response.json を parse_evaluate_response に通し admission_decision を検証する。
#[test]
fn f4_evaluate_response_parses() {
    let raw = std::fs::read_to_string(fixtures_dir().join("evaluate_response.json")).unwrap();
    let resp: EvaluateResponse = serde_json::from_str(&raw).unwrap();
    let summary = parse_evaluate_response(&resp);
    assert_eq!(
        summary.admission_decision.as_deref(),
        Some("accepted"),
        "expected admission_decision=Some('accepted')"
    );
}

/// F5: unsafe_raw_log_response.json の破壊的コマンドを render_context_pack が拒否し None を返すことを検証する。
#[test]
fn f5_unsafe_raw_log_rejected() {
    let raw = std::fs::read_to_string(fixtures_dir().join("unsafe_raw_log_response.json")).unwrap();
    let resp: ContextPackResponse = serde_json::from_str(&raw).unwrap();
    let (result, _stats) = render_context_pack(&resp, &empty_blocked());
    assert!(
        result.is_none(),
        "expected None for unsafe fixture but got Some({:?})",
        result
    );
}

/// F6: fixtures/photon/ の全 .json ファイルに secrets / 実ユーザーパスが含まれないことを検証する。
#[test]
fn f6_no_secrets_in_fixtures() {
    let secret_patterns = [
        "/Users/",
        "/home/",
        "C:\\Users\\",
        "/private/var/folders/",
        "/var/folders/",
        "ghp_",
        "github_pat_",
        "AKIA",
        "ASIA",
        "sk-",
        "xoxb-",
        "xoxp-",
        "-----BEGIN",
        "api_key=",
        "token=",
        "password=",
        "secret=",
    ];

    let dir = fixtures_dir();
    let entries =
        std::fs::read_dir(&dir).unwrap_or_else(|e| panic!("failed to read_dir {dir:?}: {e}"));

    let mut checked = 0usize;
    for entry in entries {
        let entry = entry.unwrap();
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("json") {
            continue;
        }
        let content = std::fs::read_to_string(&path)
            .unwrap_or_else(|e| panic!("failed to read {path:?}: {e}"));
        for pattern in &secret_patterns {
            assert!(
                !content.contains(pattern),
                "fixture {path:?} contains forbidden pattern {pattern:?}"
            );
        }
        checked += 1;
    }
    assert!(checked > 0, "no .json files found in {dir:?}");
}
