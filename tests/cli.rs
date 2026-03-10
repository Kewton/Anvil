use clap::Parser;

use anvil::cli::{Cli, PermissionModeArg, ProviderArg};

#[test]
fn parses_one_shot_arguments() {
    let cli = Cli::parse_from([
        "anvil",
        "-p",
        "build a game",
        "--provider",
        "ollama",
        "--model",
        "qwen3.5:35b",
        "--permission-mode",
        "accept-edits",
    ]);

    assert_eq!(cli.prompt.as_deref(), Some("build a game"));
    assert_eq!(cli.provider, ProviderArg::Ollama);
    assert_eq!(cli.model, "qwen3.5:35b");
    assert_eq!(cli.permission_mode, PermissionModeArg::AcceptEdits);
}
