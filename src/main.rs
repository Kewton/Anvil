use clap::Parser;

fn main() {
    if let Err(err) = anvil::run_cli(anvil::cli::CliArgs::parse()) {
        eprintln!("error: {err}");
        std::process::exit(1);
    }
}
