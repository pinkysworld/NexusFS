use clap::{Parser, Subcommand};
use std::path::PathBuf;

#[derive(Debug, Parser)]
#[command(name = "nexusfs", version, about = "NexusFS single-binary (skeleton)")]
pub struct Cli {
    #[command(subcommand)]
    pub command: Command,
}

#[derive(Debug, Subcommand)]
pub enum Command {
    /// Run the daemon (admin console, storage, optional services).
    Daemon {
        /// Path to config file (TOML).
        #[arg(long)]
        config: PathBuf,
    },

    /// Print basic status from the local repository (head hash, device id).
    Status {
        /// Path to config file (TOML).
        #[arg(long)]
        config: PathBuf,
    },
}
