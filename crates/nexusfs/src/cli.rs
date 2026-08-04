use clap::{Parser, Subcommand};
use std::path::PathBuf;

#[derive(Debug, Parser)]
#[command(name = "nexusfs", version, about = "NexusFS single-binary")]
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

    /// Create a directory.
    Mkdir {
        #[arg(long)]
        config: PathBuf,
        /// Absolute path inside the filesystem, e.g. /docs.
        path: String,
        /// Create missing parent directories.
        #[arg(short = 'p', long)]
        parents: bool,
    },

    /// Copy a local file into the filesystem.
    Put {
        #[arg(long)]
        config: PathBuf,
        /// Local file to read.
        source: PathBuf,
        /// Destination path inside the filesystem, e.g. /docs/a.txt.
        dest: String,
    },

    /// Write a file's contents to stdout.
    Cat {
        #[arg(long)]
        config: PathBuf,
        path: String,
    },

    /// List a directory.
    Ls {
        #[arg(long)]
        config: PathBuf,
        #[arg(default_value = "/")]
        path: String,
    },

    /// Remove a directory entry.
    Rm {
        #[arg(long)]
        config: PathBuf,
        path: String,
    },

    /// Audit the repository: signatures, proofs, and readability of every file.
    Verify {
        #[arg(long)]
        config: PathBuf,
    },

    /// Move or rename an entry.
    Mv {
        #[arg(long)]
        config: PathBuf,
        from: String,
        to: String,
    },
}
