use anvil::cli::CliArgs;
use clap::Parser;

#[test]
fn engine_minimal_returns_not_implemented_before_ollama_startup() {
    let tmp = tempfile::tempdir().unwrap();
    let args = CliArgs::parse_from([
        "anvil",
        "--engine",
        "minimal",
        "--prompt",
        "hello",
        "--cwd",
        tmp.path().to_str().unwrap(),
    ]);

    let err = anvil::run_cli(args).expect_err("minimal engine should be a stub");
    assert_eq!(err, "minimal engine is not implemented yet");
}
