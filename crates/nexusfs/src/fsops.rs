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
