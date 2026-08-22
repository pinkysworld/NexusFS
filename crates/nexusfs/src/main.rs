#![forbid(unsafe_code)]

mod cli;
mod config;
mod daemon;
mod fsops;

use anyhow::Result;
use clap::Parser;
use tracing_subscriber::EnvFilter;

use crate::cli::{Cli, Command};

#[tokio::main]
async fn main() -> Result<()> {
    // `info` rather than the `ERROR`-only default an empty `RUST_LOG` gives, because
    // that default made the daemon start in silence: no "admin listening on", no
    // "replication enabled", and — worst of the three — no warning that
    // trust-on-first-use would pin whichever key connected first. `RUST_LOG` still
    // overrides, so anything quieter or louder is one env var away.
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")),
        )
        .init();

    let cli = Cli::parse();

    match cli.command {
        Command::Daemon { config } => daemon::run_daemon(config).await,
        Command::Status { config } => daemon::run_status(config).await,
        Command::Mkdir {
            config,
            path,
            parents,
        } => fsops::run_mkdir(config, path, parents).await,
        Command::Put {
            config,
            source,
            dest,
        } => fsops::run_put(config, source, dest).await,
        Command::Cat { config, path } => fsops::run_cat(config, path).await,
        Command::Ls { config, path } => fsops::run_ls(config, path).await,
        Command::Rm { config, path } => fsops::run_rm(config, path).await,
        Command::Mv { config, from, to } => fsops::run_mv(config, from, to).await,
        Command::Verify { config } => fsops::run_verify(config).await,
        Command::Gc { config, apply } => fsops::run_gc(config, apply).await,
        Command::Migrate { config } => fsops::run_migrate(config).await,
        Command::Peer { action } => fsops::run_peer(action).await,
        Command::Prove {
            config,
            path,
            inode,
            out,
        } => fsops::run_prove(config, path, inode, out).await,
        Command::CheckProof { file, root } => fsops::run_check_proof(file, root).await,
    }
}
