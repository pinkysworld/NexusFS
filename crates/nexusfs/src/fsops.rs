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
    // Counted separately from blobs rather than folded in, because the two are not
    // equally recoverable if this gets it wrong: a blob can be fetched from a peer
    // again, a record cannot.
    println!("records:        {}", report.records_scanned);
    println!("  orphaned:     {}", report.records_unreachable);

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
            "\ndeleted {} blobs, freeing {}, and {} orphaned records",
            report.deleted,
            human_bytes(report.bytes_deleted),
            report.records_deleted
        );
    }
    Ok(())
}

/// Re-seal existing encrypted files to the peers enrolled now.
pub async fn run_share(config_path: PathBuf, apply: bool) -> Result<()> {
    let (core, identity) = open_repo(&config_path)?;
    let recipients = core.sealing_recipient_keys()?;
    let report = core.reseal_to_recipients(&identity, nexusfs_core::now_ms(), !apply)?;

    println!("recipients:     {} (this node included)", recipients.len());
    println!("files scanned:  {}", report.files_scanned);
    println!("  plaintext:    {}", report.plaintext);
    println!("  up to date:   {}", report.already_current);
    println!("  to re-seal:   {}", report.resealed);
    if report.unreadable > 0 {
        // Not an error: a node that is not itself a recipient cannot recover the file
        // key, so it has nothing to re-seal with. Saying how many is more useful than
        // stopping partway.
        println!(
            "  unreadable:   {} (this node is not a recipient)",
            report.unreadable
        );
    }

    if report.dry_run {
        if report.resealed > 0 {
            println!("\nnothing was written. Re-run with --apply to re-seal.");
        } else {
            println!("\nnothing to re-seal.");
        }
    } else {
        println!(
            "\nre-sealed {} file(s), one operation each",
            report.resealed
        );
    }

    // Said on every run, including the survey. An operator who reaches for this after
    // *removing* a peer, believing it revokes, has done nothing at all.
    println!(
        "\nnote: this grants access and never withdraws it. The ciphertext is unchanged, \n\
         so anyone who already held a key for these files still holds one. Withdrawing \n\
         access means re-encrypting under fresh keys, which this does not do."
    );
    Ok(())
}

/// Re-encrypt content under fresh keys, sealed to the peers enrolled now.
pub async fn run_rotate(config_path: PathBuf, path: Option<String>, apply: bool) -> Result<()> {
    let (core, identity) = open_repo(&config_path)?;
    let recipients = core.sealing_recipient_keys()?;
    let report = core.rotate_content(&identity, path.as_deref(), nexusfs_core::now_ms(), !apply)?;

    println!("recipients:     {} (this node included)", recipients.len());
    println!("files scanned:  {}", report.files_scanned);
    println!("  to rotate:    {}", report.rotated);
    if report.plaintext > 0 {
        println!("  plaintext:    {} (no key to rotate)", report.plaintext);
    }
    if report.unreadable > 0 {
        println!(
            "  unreadable:   {} (this node is not a recipient)",
            report.unreadable
        );
    }
    println!("content:        {}", human_bytes(report.bytes));

    if report.dry_run {
        if report.rotated > 0 {
            println!(
                "\nnothing was written. Re-run with --apply to re-encrypt {}.",
                human_bytes(report.bytes)
            );
        } else {
            println!("\nnothing to rotate.");
        }
    } else {
        println!(
            "\nrotated {} file(s), re-encrypting {}",
            report.rotated,
            human_bytes(report.bytes)
        );
        println!("run `nexusfs gc --apply` to reclaim the superseded ciphertext");
    }

    // The limit of what rotation can do, stated on every run. An operator who believes
    // this reaches a peer's existing copy will make a decision they would not otherwise
    // make.
    println!(
        "\nnote: this withdraws access to the content from here on. A device that already \n\
         copied the old ciphertext and held a key for it can still read that version."
    );
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
            println!("seal_key:  {}", hex::encode(identity.sealing_pubkey()));
            println!("\nEnrol this node on a peer with:");
            println!(
                "  nexusfs peer add --config <peer-config> {:x} {} {}",
                core.device_id.0,
                hex::encode(identity.pubkey_bytes()),
                hex::encode(identity.sealing_pubkey())
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
                // The sealing key's absence is worth showing: it is the difference
                // between a peer that can read new encrypted content and one that
                // cannot, and nothing else on this line reveals it.
                println!(
                    "{:032x}  {}  {}",
                    peer.device_id.0,
                    hex::encode(peer.pubkey),
                    match peer.seal_key {
                        Some(k) => hex::encode(k),
                        None => "(no sealing key)".into(),
                    }
                );
            }
            Ok(())
        }

        PeerAction::Add {
            config,
            device,
            pubkey,
            seal_key,
            rotate,
        } => {
            let (core, _identity) = open_repo(&config)?;
            let device = parse_device_id(&device)?;
            let key = parse_pubkey(&pubkey)?;
            let seal = seal_key.as_deref().map(parse_pubkey).transpose()?;

            match core.enrol_peer(device, &key, seal.as_ref(), rotate)? {
                nexusfs_core::Enrolment::Added => println!("enrolled {:x}", device.0),
                nexusfs_core::Enrolment::Unchanged => {
                    println!("{:x} was already enrolled with this key", device.0)
                }
                nexusfs_core::Enrolment::Rotated => {
                    println!("rotated the key for {:x}", device.0)
                }
            }
            if seal.is_none() {
                println!(
                    "note: no sealing key given, so this peer cannot be a recipient of \
                     newly written encrypted content. Re-run with the seal_key from \
                     `nexusfs peer identity` on that node to add one."
                );
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
    /// Human orientation only; the proof is about an inode, not a path. Absent when the
    /// subject was named by inode, which is the only way to ask about a missing entry.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    path: Option<String>,
    /// Hex state root the proof is against.
    state_root: String,
    /// Hex device id of the node that produced it, for provenance.
    issuer: String,
    inode: String,
    /// Not `#[serde(flatten)]`: flattening routes the value through serde_json's
    /// internal representation, which cannot hold the `u128` inode inside the proof.
    claim: Claim,
}

/// Which way round the proof runs.
#[derive(serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
enum Claim {
    /// The entry is in the committed state, with this content.
    Present {
        proof: nexusfs_zk::merkle::InclusionProof,
    },
    /// The entry is not in the committed state at all.
    Absent {
        proof: nexusfs_zk::merkle::AbsenceProof,
    },
}

pub async fn run_prove(
    config_path: PathBuf,
    path: Option<String>,
    inode_arg: Option<String>,
    out: Option<PathBuf>,
) -> Result<()> {
    let (core, _identity) = open_repo(&config_path)?;

    let (inode, path) = match (&path, &inode_arg) {
        (Some(_), Some(_)) => bail!("give a path or --inode, not both"),
        (Some(p), None) => {
            let Some((inode, _)) = core.resolve_path(p)? else {
                bail!(
                    "no such path: {p}. To prove something is *absent*, name it with \
                     --inode: a missing entry has no path to resolve."
                );
            };
            (inode, Some(p.clone()))
        }
        (None, Some(hex_inode)) => {
            let trimmed = hex_inode.trim().trim_start_matches("0x");
            let inode = u128::from_str_radix(trimmed, 16)
                .with_context(|| format!("inode {hex_inode:?} is not hexadecimal"))?;
            (inode, None)
        }
        (None, None) => bail!("give a path, or --inode"),
    };

    let Some(state_root) = core.get_state_root()? else {
        bail!("this repository has no state root yet");
    };

    let claim = match core.inclusion_proof(inode)? {
        Some(proof) => Claim::Present { proof },
        None => match core.absence_proof(inode)? {
            Some(proof) => Claim::Absent { proof },
            // Neither present nor absent means the map disagrees with itself.
            None => bail!("inode {inode:x} is neither present nor provably absent"),
        },
    };

    let portable = PortableProof {
        path,
        state_root: hex::encode(state_root),
        issuer: format!("{:x}", core.device_id.0),
        inode: format!("{inode:x}"),
        claim,
    };
    let json = serde_json::to_string_pretty(&portable).context("serialize proof")?;

    let subject = portable
        .path
        .clone()
        .unwrap_or_else(|| format!("inode {}", portable.inode));

    match out {
        Some(file) => {
            std::fs::write(&file, json).with_context(|| format!("write {}", file.display()))?;
            println!("wrote proof for {subject} to {}", file.display());
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

    match &portable.claim {
        Claim::Present { proof } => nexusfs_zk::merkle::check(proof, &expected)?,
        Claim::Absent { proof } => nexusfs_zk::merkle::check_absent(proof, &expected)?,
    }

    // Print the inode the *proof* commits to, not the one the file's header claims.
    // Those fields are decoration: nothing signs or binds them, so a file whose header
    // was edited would otherwise have a genuine proof reported against the wrong
    // subject — by the one command whose job is to not be fooled.
    let proven_inode = match &portable.claim {
        Claim::Present { proof } => proof.inode,
        Claim::Absent { proof } => proof.inode,
    };
    println!("inode:      {proven_inode:x}   (from the proof)");
    println!("state root: {}", hex::encode(expected));
    match &portable.claim {
        Claim::Present { proof } => {
            println!("claim:      present");
            println!("object:     {}", hex::encode(proof.value));
            println!("path steps: {}", proof.steps.len());
        }
        Claim::Absent { proof } => {
            println!("claim:      absent");
            println!("map size:   {} entries", proof.len);
        }
    }

    // Everything below is unauthenticated context carried alongside the proof.
    println!("\nunverified labels from the file (not covered by the proof):");
    println!(
        "  path:     {}",
        portable.path.as_deref().unwrap_or("(none)")
    );
    println!("  inode:    {}", portable.inode);
    println!("  issuer:   {}", portable.issuer);
    if u128::from_str_radix(portable.inode.trim_start_matches("0x"), 16) != Ok(proven_inode) {
        println!("  WARNING:  the file's inode label does not match the proof's subject");
    }

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
