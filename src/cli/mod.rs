mod app;
mod commands;
mod flags;
mod output;

pub use app::Cli;

pub fn run() -> anyhow::Result<()> {
    let cli = <Cli as clap::Parser>::parse();
    commands::execute(cli)
}
