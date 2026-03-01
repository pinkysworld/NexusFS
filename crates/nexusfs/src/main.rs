#![forbid(unsafe_code)]

mod cli;
mod config;
mod daemon;

use anyhow::Result;
use clap::Parser;
use tracing_subscriber::EnvFilter;

use crate::cli::{Cli, Command};

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::from_default_env())
        .init();

    let cli = Cli::parse();

    match cli.command {
        Command::Daemon { config } => daemon::run_daemon(config).await,
        Command::Status { config } => daemon::run_status(config).await,
    }
}
