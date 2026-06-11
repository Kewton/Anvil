use anvil::cli::CliArgs;
use anvil::session::sessions_cli::{ShowView, scan_session_meta};
use anvil::session::store::SessionSnapshot;
use clap::Parser;

#[test]
fn engine_minimal_runs_one_ollama_turn() {
    let mut server = mockito::Server::new();
    let tags = server
        .mock("GET", "/api/tags")
        .with_status(200)
        .with_header("content-type", "application/json")
        .with_body(r#"{"models":[{"name":"qwen3:8b","details":{}}]}"#)
        .expect(1)
        .create();
    let generate = server
        .mock("POST", "/api/generate")
        .with_status(200)
        .with_header("content-type", "application/json")
        .with_body(r#"{"response":"minimal ok","done":true}"#)
        .expect(1)
        .create();

    let tmp = tempfile::tempdir().unwrap();
    let state = tempfile::tempdir().unwrap();
    let args = CliArgs::parse_from([
        "anvil",
        "--engine",
        "minimal",
        "--prompt",
        "hello",
        "--cwd",
        tmp.path().to_str().unwrap(),
        "--state-dir",
        state.path().to_str().unwrap(),
        "--ollama-host",
        &server.url(),
    ]);

    anvil::run_cli(args).unwrap();

    tags.assert();
    generate.assert();

    let metas = scan_session_meta(state.path());
    assert_eq!(metas.len(), 1);
    assert_eq!(metas[0].message_count, 2);

    let session_dir = std::fs::read_dir(state.path().join("sessions"))
        .unwrap()
        .next()
        .unwrap()
        .unwrap()
        .path();
    let data = std::fs::read_to_string(session_dir.join("session.json")).unwrap();
    let snapshot: SessionSnapshot = serde_json::from_str(&data).unwrap();
    let show = ShowView::from_snapshot(&snapshot);
    assert_eq!(show.first_user_preview.as_deref(), Some("hello"));
    assert_eq!(
        show.last_assistant_or_tool_preview.as_deref(),
        Some("minimal ok")
    );
}
