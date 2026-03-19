use std::process::ExitCode;

use clap::Parser;

fn main() -> ExitCode {
    let mut cli = anvil::config::CliArgs::parse();
    cli.resolve_tag_protocol();
    if let Err(err) = anvil::app::run_with_args(&cli) {
        let code = err.exit_code();
        eprintln!("anvil error: {err}");
        // exit_code 1 = config/startup error → show guidance
        // exit_code 2 = tool execution failure → guidance not needed
        if code == 1 {
            eprintln!();
            eprintln!("{}", anvil::app::error_guidance(&err));
        }
        return ExitCode::from(code);
    }
    ExitCode::SUCCESS
}
