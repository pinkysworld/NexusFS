//! Filesystem commands.
//!
//! Thin wrappers over `nexusfs_core`'s high-level operations. The verbs here own
//! argument handling and output formatting only — what a write actually *means* is
//! defined once in the core, so the CLI, the S3 facade and the browser build cannot
//! drift apart.

use std::io::Write as _;
use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};

use nexusfs_core::{now_ms, CoreState, EntryType};
use nexusfs_crypto::Identity;

use crate::config::Config;
use crate::daemon::open_core;

/// Open the repository ready for a filesystem command.
fn open_repo(config_path: &Path) -> Result<(CoreState, Identity)> {
    let cfg = Config::load(config_path)?;
    let (core, identity, _token) = open_core(&cfg)?;
    core.require_current_format()?;
    core.bootstrap_if_needed()?;
    Ok((core, identity))
}

pub async fn run_mkdir(config_path: PathBuf, path: String, parents: bool) -> Result<()> {
    let (core, identity) = open_repo(&config_path)?;

    if !parents {
        // Without -p, refuse rather than silently creating intermediate directories.
        let Some((parent, name)) = core.resolve_parent(&path)? else {
            bail!("parent directory of {path} does not exist (use -p to create it)");
        };
        if core.lookup(parent, &name)?.is_some() {
            bail!("{path} already exists");
        }
    }

    core.mkdir_p(&identity, &path, now_ms())?;
    println!("created {path}");
    Ok(())
}

pub async fn run_put(config_path: PathBuf, source: PathBuf, dest: String) -> Result<()> {
    let (core, identity) = open_repo(&config_path)?;

    let data = std::fs::read(&source).with_context(|| format!("read {}", source.display()))?;
    core.write_file(&identity, &dest, &data, now_ms())?;

    println!("wrote {} bytes to {dest}", data.len());
    Ok(())
}

pub async fn run_cat(config_path: PathBuf, path: String) -> Result<()> {
    let (core, _identity) = open_repo(&config_path)?;
    let bytes = core.read_file_path(&path)?;
    std::io::stdout().write_all(&bytes)?;
    Ok(())
}

pub async fn run_ls(config_path: PathBuf, path: String) -> Result<()> {
    let (core, _identity) = open_repo(&config_path)?;

    for entry in core.read_dir_path(&path)? {
        let (marker, size) = match entry.entry_type {
            EntryType::Dir => ("/", String::new()),
            EntryType::File => {
                let size = core
                    .load_inode(entry.inode_id)?
                    .map(|r| r.content.value.size)
                    .unwrap_or(0);
                ("", format!("{size:>10}  "))
            }
        };
        println!("{size}{}{marker}", entry.name);
    }
    Ok(())
}

pub async fn run_rm(config_path: PathBuf, path: String) -> Result<()> {
    let (core, identity) = open_repo(&config_path)?;
    core.remove_path(&identity, &path, now_ms())?;
    println!("removed {path}");
    Ok(())
}

pub async fn run_mv(config_path: PathBuf, from: String, to: String) -> Result<()> {
    let (core, identity) = open_repo(&config_path)?;
    core.rename_path(&identity, &from, &to, now_ms())?;
    println!("moved {from} -> {to}");
    Ok(())
}

pub async fn run_verify(config_path: PathBuf) -> Result<()> {
    let (core, _identity) = open_repo(&config_path)?;
    let report = core.verify_repository()?;

    println!("operations:        {}", report.operations);
    println!(
        "  with proof:      {} ({} without)",
        report.with_proof, report.without_proof
    );
    println!("  malformed proof: {}", report.malformed);
    println!("  bad signature:   {}", report.signature_failures);
    println!(
        "state root:        {}",
        report
            .state_root
            .map(hex::encode)
            .unwrap_or_else(|| "(none)".into())
    );
    println!(
        "encryption:        {}",
        if core.encryption_enabled() {
            "on"
        } else {
            "off"
        }
    );

    if report.unreadable_files.is_empty() {
        println!("files:             all readable");
    } else {
        println!(
            "files:             {} UNREADABLE",
            report.unreadable_files.len()
        );
        for path in &report.unreadable_files {
            println!("  {path}");
        }
    }

    if report.ok() {
        println!("\nrepository verified");
        Ok(())
    } else {
        // A non-zero exit makes this usable from a cron job or CI step.
        bail!("verification found problems")
    }
}

/// Report unreachable storage, and reclaim it when asked.
///
/// The survey is the default because the failure modes are asymmetric: an unnecessary
/// report costs a walk of the namespace, while an unnecessary delete costs data.
pub async fn run_gc(config_path: PathBuf, apply: bool) -> Result<()> {
    let (core, _identity) = open_repo(&config_path)?;
    let report = core.collect_garbage(!apply)?;

    println!("blobs scanned:  {}", report.blobs_scanned);
    println!("  reachable:    {}", report.reachable);
    println!("  unreachable:  {}", report.unreachable);
    println!(
        "stored:         {} ({} reclaimable)",
        human_bytes(report.bytes_scanned),
        human_bytes(report.bytes_reclaimable)
    );

    if let Some(reason) = &report.refused {
        bail!("refused to collect: {reason}");
    }

    if report.dry_run {
        if report.has_garbage() {
            println!("\nnothing was deleted. Re-run with --apply to reclaim.");
        } else {
            println!("\nnothing to reclaim.");
        }
    } else {
        println!(
            "\ndeleted {} blobs, freeing {}",
            report.deleted,
            human_bytes(report.bytes_deleted)
        );
    }
    Ok(())
}

fn human_bytes(n: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KiB", "MiB", "GiB", "TiB"];
    let mut value = n as f64;
    let mut unit = 0;
    while value >= 1024.0 && unit < UNITS.len() - 1 {
        value /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{n} B")
    } else {
        format!("{value:.1} {}", UNITS[unit])
    }
}

/// Upgrade the on-disk format.
///
/// Separate from opening on purpose: a migration rewrites records in place, so the
/// operator picks the moment rather than discovering it happened.
pub async fn run_migrate(config_path: PathBuf) -> Result<()> {
    let cfg = crate::config::Config::load(&config_path)?;
    let (core, _identity, _token) = crate::daemon::open_core(&cfg)?;

    let found = core.format_version()?;
    println!(
        "on-disk format: {}",
        found
            .map(|v| format!("v{v}"))
            .unwrap_or_else(|| "unstamped (treated as v1)".into())
    );
    println!("this build:     v{}", nexusfs_core::CURRENT_FORMAT_VERSION);

    let applied = core.migrate()?;
    if applied.is_empty() {
        println!("\nalready current; nothing to do");
    } else {
        for v in &applied {
            println!("  migrated to v{v}");
        }
        println!("\nmigration complete");
    }
    Ok(())
}

// --- peer enrolment ----------------------------------------------------------
//
// Trust-on-first-use is convenient and is the wrong default whenever first contact
// could be intercepted. These verbs are what make `net.tofu = false` usable: an
// operator reads a node's identity, enrols it on the other node, and neither ever
// accepts a key it was not told about.

pub async fn run_peer(action: crate::cli::PeerAction) -> Result<()> {
    use crate::cli::PeerAction;

    match action {
        PeerAction::Identity { config } => {
            let (core, identity) = open_repo(&config)?;
            println!("device_id: {:x}", core.device_id.0);
            println!("pubkey:    {}", hex::encode(identity.pubkey_bytes()));
            println!("\nEnrol this node on a peer with:");
            println!(
                "  nexusfs peer add --config <peer-config> {:x} {}",
                core.device_id.0,
                hex::encode(identity.pubkey_bytes())
            );
            Ok(())
        }

        PeerAction::List { config } => {
            let (core, _identity) = open_repo(&config)?;
            let peers = core.enrolled_peers()?;
            if peers.is_empty() {
                println!("no peers enrolled");
                return Ok(());
            }
            for peer in peers {
                println!("{:032x}  {}", peer.device_id.0, hex::encode(peer.pubkey));
            }
            Ok(())
        }

        PeerAction::Add {
            config,
            device,
            pubkey,
            rotate,
        } => {
            let (core, _identity) = open_repo(&config)?;
            let device = parse_device_id(&device)?;
            let key = parse_pubkey(&pubkey)?;

            match core.enrol_peer(device, &key, rotate)? {
                nexusfs_core::Enrolment::Added => println!("enrolled {:x}", device.0),
                nexusfs_core::Enrolment::Unchanged => {
                    println!("{:x} was already enrolled with this key", device.0)
                }
                nexusfs_core::Enrolment::Rotated => {
                    println!("rotated the key for {:x}", device.0)
                }
            }
            Ok(())
        }

        PeerAction::Remove { config, device } => {
            let (core, _identity) = open_repo(&config)?;
            let device = parse_device_id(&device)?;
            if core.revoke_peer(device)? {
                println!("removed {:x}", device.0);
                println!(
                    "note: with net.tofu = true this device will be pinned again on its \
                     next connection"
                );
            } else {
                println!("{:x} was not enrolled", device.0);
            }
            Ok(())
        }
    }
}

fn parse_device_id(s: &str) -> Result<nexusfs_proto::DeviceId> {
    let trimmed = s.trim().trim_start_matches("0x");
    let value = u128::from_str_radix(trimmed, 16)
        .with_context(|| format!("device id {s:?} is not hexadecimal"))?;
    Ok(nexusfs_proto::DeviceId(value))
}

fn parse_pubkey(s: &str) -> Result<[u8; 32]> {
    let raw = hex::decode(s.trim()).context("public key is not hexadecimal")?;
    if raw.len() != 32 {
        bail!(
            "public key is {} bytes, expected 32 (64 hex characters)",
            raw.len()
        );
    }
    let mut key = [0u8; 32];
    key.copy_from_slice(&raw);
    Ok(key)
}

// --- portable proofs ---------------------------------------------------------
//
// The point of the commitment layer is that a claim about state travels on its own. So
// `prove` writes something self-contained and `check-proof` opens no repository at all —
// if it needed one, the proof would not be proving anything the holder could not already
// see for themselves.

/// What `prove` writes and `check-proof` reads.
#[derive(serde::Serialize, serde::Deserialize)]
struct PortableProof {
    /// Human orientation only; the proof is about an inode, not a path.
    path: String,
    /// Hex state root the inclusion path reaches.
    state_root: String,
    /// Hex device id of the node that produced it, for provenance.
    issuer: String,
    inode: String,
    proof: nexusfs_zk::merkle::InclusionProof,
}

pub async fn run_prove(config_path: PathBuf, path: String, out: Option<PathBuf>) -> Result<()> {
    let (core, _identity) = open_repo(&config_path)?;

    let Some((inode, _)) = core.resolve_path(&path)? else {
        bail!("no such path: {path}");
    };
    let Some(proof) = core.inclusion_proof(inode)? else {
        bail!(
            "{path} resolves to inode {inode:x}, which is not in the committed state; \
             an entry with no content yet has nothing to prove"
        );
    };
    let Some(state_root) = core.get_state_root()? else {
        bail!("this repository has no state root yet");
    };

    let portable = PortableProof {
        path,
        state_root: hex::encode(state_root),
        issuer: format!("{:x}", core.device_id.0),
        inode: format!("{inode:x}"),
        proof,
    };
    let json = serde_json::to_string_pretty(&portable).context("serialize proof")?;

    match out {
        Some(file) => {
            std::fs::write(&file, json).with_context(|| format!("write {}", file.display()))?;
            println!("wrote proof for {} to {}", portable.path, file.display());
        }
        None => println!("{json}"),
    }
    Ok(())
}

pub async fn run_check_proof(file: String, root: Option<String>) -> Result<()> {
    let json = if file == "-" {
        use std::io::Read;
        let mut buf = String::new();
        std::io::stdin()
            .read_to_string(&mut buf)
            .context("read proof from stdin")?;
        buf
    } else {
        std::fs::read_to_string(&file).with_context(|| format!("read {file}"))?
    };

    let portable: PortableProof = serde_json::from_str(&json).context("parse proof")?;

    // An explicit --root is the meaningful check. Without one this only confirms the
    // proof is internally consistent, which says nothing about whether the root is a
    // state anyone else agrees with — so say so rather than printing a bare "valid".
    let expected_hex = root.as_ref().unwrap_or(&portable.state_root);
    let raw = hex::decode(expected_hex.trim()).context("state root is not hexadecimal")?;
    if raw.len() != 32 {
        bail!("state root is {} bytes, expected 32", raw.len());
    }
    let mut expected = [0u8; 32];
    expected.copy_from_slice(&raw);

    nexusfs_zk::merkle::check(&portable.proof, &expected)?;

    println!("path:       {}", portable.path);
    println!("inode:      {}", portable.inode);
    println!("issuer:     {}", portable.issuer);
    println!("state root: {}", hex::encode(expected));
    println!("object:     {}", hex::encode(portable.proof.value));
    println!("path steps: {}", portable.proof.steps.len());

    if root.is_some() {
        println!("\nproof holds against the supplied state root");
    } else {
        println!(
            "\nproof is internally consistent. It was checked against the root recorded \
             inside it, so this does not establish that root is one you trust — pass \
             --root to check against a root you obtained independently."
        );
    }
    Ok(())
}
