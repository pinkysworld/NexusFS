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

    /// Emit a portable proof that a path holds its current content.
    ///
    /// The result verifies against the state root alone — no repository needed.
    Prove {
        #[arg(long)]
        config: PathBuf,
        /// Path to prove. Omit when using --inode.
        path: Option<String>,
        /// Prove about an inode instead of a path.
        ///
        /// The only way to ask about something that is *not* there: an absent entry has
        /// no path to resolve. Pair an inclusion proof against an old root with an
        /// absence proof against a new one to demonstrate a deletion.
        #[arg(long)]
        inode: Option<String>,
        /// Write to a file instead of stdout.
        #[arg(long)]
        out: Option<PathBuf>,
    },

    /// Check a proof emitted by `prove`.
    ///
    /// Reads no repository: the proof and the root it is checked against are enough.
    CheckProof {
        /// File written by `prove`, or `-` for stdin.
        file: String,
        /// Expected state root in hex. Defaults to the one recorded in the proof, which
        /// checks internal consistency but not that the root is one you trust.
        #[arg(long)]
        root: Option<String>,
    },

    /// Manage which peer devices this node will accept operations from.
    Peer {
        #[command(subcommand)]
        action: PeerAction,
    },

    /// Re-seal existing encrypted files to the peers enrolled now.
    ///
    /// Enrolling a peer makes it a recipient of everything written afterwards; this
    /// brings files already on disk up to date. It grants access and never withdraws
    /// it — the ciphertext does not change, so anyone who already held a key still
    /// holds it.
    Share {
        #[arg(long)]
        config: PathBuf,
        /// Actually re-seal. Without this, report what would be done.
        #[arg(long)]
        apply: bool,
    },

    /// Re-encrypt content under fresh keys, sealed to the peers enrolled now.
    ///
    /// This is what makes removing a peer mean something: `share` adds recipients and
    /// cannot take one away, because the ciphertext does not change. Rotation changes
    /// it. Expensive — every byte is read, encrypted again and written again — and it
    /// cannot withdraw what a device already copied.
    Rotate {
        #[arg(long)]
        config: PathBuf,
        /// Rotate one file rather than everything encrypted.
        #[arg(long)]
        path: Option<String>,
        /// Actually re-encrypt. Without this, report what would be done.
        #[arg(long)]
        apply: bool,
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
        /// X25519 sealing key in hex (64 characters), as printed by `peer identity`.
        ///
        /// Optional: a peer enrolled without one replicates and verifies normally, it
        /// just cannot be made a recipient of newly written encrypted content.
        seal_key: Option<String>,
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
