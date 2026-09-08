//! Runtime CLI configuration (plan §13).

use std::path::PathBuf;

use clap::{Parser, Subcommand};

#[derive(Parser, Debug)]
#[command(
    name = "kamimusuhi-runtime",
    about = "Kamimusuhi continuity vertical slice runtime"
)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Command,
}

#[derive(Subcommand, Debug)]
pub enum Command {
    /// Create (if absent) and migrate a canonical database, and ensure a
    /// root individual exists.
    Init {
        #[arg(long)]
        db: PathBuf,
    },
    /// Read-only summary of a canonical database's individuals/heads.
    Inspect {
        #[arg(long)]
        db: PathBuf,
    },
}
