use clap::Parser;

fn main() {
    let cli = anvilminimal::cli::Cli::parse();
    if let Err(err) = anvilminimal::run(cli) {
        eprintln!("error: {err:#}");
        std::process::exit(1);
    }
}
