//! Filesystem commands.
//!
//! Every verb here goes through the same signed-operation pipeline the daemon and
//! (later) replication use. Nothing writes namespace state directly — that is the
//! point of the exercise: if `put` and `cat` work, the state machine is real.

use std::io::Write as _;
use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};

use nexusfs_core::{now_ms, ApplyOutcome, CoreState, EntryType};
use nexusfs_crypto::Identity;
use nexusfs_proto::FsOpKind;

use crate::config::Config;
use crate::daemon::open_core;

/// Open the repository ready for a filesystem command.
fn open_repo(config_path: &Path) -> Result<(CoreState, Identity)> {
    let cfg = Config::load(config_path)?;
    let (core, identity, _token) = open_core(&cfg)?;
    core.bootstrap_if_needed()?;
    Ok((core, identity))
}

/// Apply an operation, turning a parked op into a clear error for CLI users.
///
/// Parking is the right behaviour for replicated ops, whose dependencies genuinely
/// may arrive later. A local command has no such excuse: if its preconditions are
/// unmet, the user asked for something impossible and should be told so.
fn apply(core: &CoreState, identity: &Identity, kind: FsOpKind) -> Result<()> {
    let op = core.make_op(identity, kind, now_ms())?;
    match core.apply_op(&op)? {
        ApplyOutcome::Applied | ApplyOutcome::AlreadyApplied => Ok(()),
        ApplyOutcome::Pending(reason) => bail!("operation cannot be applied: {reason}"),
    }
}

pub async fn run_mkdir(config_path: PathBuf, path: String, parents: bool) -> Result<()> {
    let (core, identity) = open_repo(&config_path)?;

    if parents {
        let mut prefix = String::new();
        for part in CoreState::split_path(&path)? {
            prefix.push('/');
            prefix.push_str(&part);
            if core.resolve_path(&prefix)?.is_none() {
                mkdir_one(&core, &identity, &prefix)?;
            }
        }
    } else {
        mkdir_one(&core, &identity, &path)?;
    }

    println!("created {path}");
    Ok(())
}

fn mkdir_one(core: &CoreState, identity: &Identity, path: &str) -> Result<()> {
    let Some((parent, name)) = core.resolve_parent(path)? else {
        bail!("parent directory of {path} does not exist (use -p to create it)");
    };
    if core.lookup(parent, &name)?.is_some() {
        bail!("{path} already exists");
    }
    apply(
        core,
        identity,
        FsOpKind::Mkdir {
            parent,
            name,
            mode: 0o40755,
        },
    )
}

pub async fn run_put(config_path: PathBuf, source: PathBuf, dest: String) -> Result<()> {
    let (core, identity) = open_repo(&config_path)?;

    let data = std::fs::read(&source).with_context(|| format!("read {}", source.display()))?;

    let Some((parent, name)) = core.resolve_parent(&dest)? else {
        bail!("parent directory of {dest} does not exist");
    };

    // Create the file if it is new; otherwise overwrite the existing inode so history
    // and any other links to it are preserved.
    let inode = match core.lookup(parent, &name)? {
        Some(entry) if entry.entry_type == EntryType::File => entry.inode_id,
        Some(_) => bail!("{dest} exists and is not a file"),
        None => {
            apply(
                &core,
                &identity,
                FsOpKind::CreateFile {
                    parent,
                    name: name.clone(),
                    mode: 0o100644,
                },
            )?;
            core.lookup(parent, &name)?
                .context("file vanished immediately after creation")?
                .inode_id
        }
    };

    let chunks = core.store_chunks(&data)?;
    apply(
        &core,
        &identity,
        FsOpKind::Write {
            inode,
            offset: 0,
            chunks,
            new_size: data.len() as u64,
        },
    )?;

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

    let Some((parent, name)) = core.resolve_parent(&path)? else {
        bail!("no such path: {path}");
    };
    if core.lookup(parent, &name)?.is_none() {
        bail!("no such path: {path}");
    }

    apply(&core, &identity, FsOpKind::Unlink { parent, name })?;
    println!("removed {path}");
    Ok(())
}

pub async fn run_mv(config_path: PathBuf, from: String, to: String) -> Result<()> {
    let (core, identity) = open_repo(&config_path)?;

    let Some((old_parent, old_name)) = core.resolve_parent(&from)? else {
        bail!("no such path: {from}");
    };
    if core.lookup(old_parent, &old_name)?.is_none() {
        bail!("no such path: {from}");
    }
    let Some((new_parent, new_name)) = core.resolve_parent(&to)? else {
        bail!("destination directory of {to} does not exist");
    };

    apply(
        &core,
        &identity,
        FsOpKind::Rename {
            old_parent,
            old_name,
            new_parent,
            new_name,
        },
    )?;
    println!("moved {from} -> {to}");
    Ok(())
}
