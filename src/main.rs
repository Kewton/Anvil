use std::process::ExitCode;

use clap::Parser;

fn main() -> ExitCode {
    let cli = anvil::config::CliArgs::parse();
    if let Err(err) = anvil::app::run_with_args(&cli) {
        eprintln!("anvil error: {err}");
        eprintln!();
        eprintln!("{}", anvil::app::error_guidance(&err));
        return ExitCode::FAILURE;
    }
    ExitCode::SUCCESS
}
