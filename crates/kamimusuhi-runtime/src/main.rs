mod commands;
mod config;

use clap::Parser;

use config::{Cli, Command};

fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();
    match cli.command {
        Command::Init { db } => commands::init::run(&db),
        Command::Inspect { db } => commands::inspect::run(&db),
    }
}
