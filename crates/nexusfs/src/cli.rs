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

    /// Report unreachable storage, and optionally reclaim it.
    ///
    /// Surveys by default. Deletion needs `--apply`, because a mistake here removes
    /// data rather than merely reporting it.
    Gc {
        #[arg(long)]
        config: PathBuf,
        /// Actually delete the unreachable blobs.
        #[arg(long)]
        apply: bool,
    },

    /// Upgrade the on-disk format to what this build expects.
    ///
    /// Back the data directory up first: a migration rewrites records in place.
    Migrate {
        #[arg(long)]
        config: PathBuf,
    },

    /// Manage which peer devices this node will accept operations from.
    Peer {
        #[command(subcommand)]
        action: PeerAction,
    },

    /// Move or rename an entry.
    Mv {
        #[arg(long)]
        config: PathBuf,
        from: String,
        to: String,
    },
}

#[derive(Subcommand, Debug)]
pub enum PeerAction {
    /// Show this node's own device id and public key, for enrolling it elsewhere.
    Identity {
        #[arg(long)]
        config: PathBuf,
    },

    /// List enrolled peers.
    List {
        #[arg(long)]
        config: PathBuf,
    },

    /// Enrol a peer's key ahead of first contact, so `tofu` can stay off.
    Add {
        #[arg(long)]
        config: PathBuf,
        /// Device id in hex, as printed by `peer identity`.
        device: String,
        /// ed25519 public key in hex (64 characters).
        pubkey: String,
        /// Replace an existing, different key. Required for a deliberate rotation.
        #[arg(long)]
        rotate: bool,
    },

    /// Forget a peer's pinned key.
    Remove {
        #[arg(long)]
        config: PathBuf,
        device: String,
    },
}
