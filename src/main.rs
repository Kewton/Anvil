use std::process::ExitCode;

fn main() -> ExitCode {
    if let Err(err) = anvil::app::run() {
        eprintln!("anvil error: {err}");
        eprintln!();
        eprintln!("{}", anvil::app::error_guidance(&err));
        return ExitCode::FAILURE;
    }
    ExitCode::SUCCESS
}
