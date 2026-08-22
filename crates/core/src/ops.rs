//! High-level filesystem operations.
//!
//! `apply.rs` deals in single operations; this is the layer above, where a caller
//! says "write these bytes to this path" and the necessary sequence of signed
//! operations is derived. Every facade — the CLI, the S3 API, the browser build —
//! goes through here, so none of them can drift into their own interpretation of
//! what a write means.

use anyhow::{bail, Context, Result};

use nexusfs_crypto::Identity;
use nexusfs_proto::{FsOp, FsOpKind};

use crate::apply::ApplyOutcome;
use crate::object::EntryType;
use crate::state::CoreState;

/// Guard against a pathological tree walking forever.
const MAX_WALK_ENTRIES: usize = 100_000;

/// Object hashes an operation introduces, for the record in its proof.
fn changed_hashes(kind: &FsOpKind) -> Vec<nexusfs_proto::Hash> {
    match kind {
        FsOpKind::Write { chunks, .. } => chunks.iter().map(|c| c.hash).collect(),
        _ => Vec::new(),
    }
}

/// A single entry produced by [`CoreState::walk`].
#[derive(Debug, Clone)]
pub struct WalkEntry {
    /// Absolute path, e.g. `/docs/notes/a.txt`.
    pub path: String,
    pub name: String,
    pub kind: EntryType,
    pub depth: usize,
    /// Byte length for files; zero for directories.
    pub size: u64,
    pub inode: u128,
    /// Last write time recorded on the inode.
    pub mtime_unix_ms: u64,
}

impl CoreState {
    /// The operations that created the entries currently visible at `name`.
    ///
    /// A dot is exactly the id of the operation that added the entry, so this is both
    /// the observed-remove set and, for a rename, the identity of the thing being
    /// moved. Sorted, so two nodes building the same removal from the same view produce
    /// byte-identical operations.
    fn observed_entry_ops(&self, parent: u128, name: &str) -> Result<Vec<nexusfs_proto::OpId>> {
        let Some(map) = self.load_dir(parent)? else {
            return Ok(Vec::new());
        };
        let mut ops: Vec<nexusfs_proto::OpId> = map
            .get_all(&name.to_string())
            .into_iter()
            .map(|(dot, _)| nexusfs_proto::OpId {
                device_id: nexusfs_proto::DeviceId(dot.device_id),
                counter: dot.counter,
            })
            .collect();
        ops.sort_by_key(|id| (id.device_id.0, id.counter));
        Ok(ops)
    }

    /// Sign an operation and apply it, treating "not yet applicable" as an error.
    ///
    /// Parking is right for replicated operations, whose dependencies may genuinely
    /// arrive later. A local caller has no such excuse: if its preconditions are
    /// unmet it asked for something impossible and should be told so.
    pub fn apply_local(&self, identity: &Identity, kind: FsOpKind, now: u64) -> Result<FsOp> {
        if !self.proofs.generates() {
            let op = self.make_op(identity, kind, now)?;
            return match self.apply_op(&op)? {
                ApplyOutcome::Applied | ApplyOutcome::AlreadyApplied => {
                    self.announce_local_op();
                    Ok(op)
                }
                ApplyOutcome::Pending(reason) => bail!("operation cannot be applied: {reason}"),
            };
        }

        // Producing evidence needs the resulting state root, which only exists after
        // the operation is applied — but the signature has to cover the evidence. So
        // apply once to learn the outcome, then re-sign the operation with its proof
        // attached and record that version. The second apply is a no-op against state
        // because the operation id is already marked applied; it exists to replace the
        // stored copy with the one whose signature covers the proof.
        let old_root = self.begin_proof()?;
        let provisional = self.make_op(identity, kind.clone(), now)?;

        match self.apply_op_unproven(&provisional)? {
            ApplyOutcome::Applied | ApplyOutcome::AlreadyApplied => {}
            ApplyOutcome::Pending(reason) => bail!("operation cannot be applied: {reason}"),
        }

        let changed = changed_hashes(&kind);
        let bundle = if self.proofs.commits() {
            self.finish_commit_proof(old_root, subject_inode(&provisional.id, &kind), changed)?
        } else {
            self.finish_proof(old_root, changed)?
        };

        let mut op = provisional;
        op.proof = Some(bundle);
        op.sig = nexusfs_crypto::sign(identity.signing_key(), &op.signing_bytes()?);
        self.append_op(&op)?;

        self.announce_local_op();
        Ok(op)
    }

    /// Tell whoever is listening that this node just applied an operation of its own.
    ///
    /// After the operation is durable, never before: a listener that reacts by telling a
    /// peer "come and get it" must not be able to arrive before the thing it is about.
    /// Errors are not possible to report from here and would not be actionable anyway —
    /// the worst case is a peer waiting out its poll interval, which is what happened
    /// before any of this existed.
    fn announce_local_op(&self) {
        if let Some(sink) = &self.local_ops {
            sink.applied();
        }
    }

    /// Create `path` and any missing parents, returning the directory's inode.
    ///
    /// Existing directories are left alone; an existing *file* on the path is an
    /// error, since silently replacing it would lose data.
    pub fn mkdir_p(&self, identity: &Identity, path: &str, now: u64) -> Result<u128> {
        let mut current = self.root_inode();
        let mut walked = String::new();

        for part in Self::split_path(path)? {
            walked.push('/');
            walked.push_str(&part);

            match self.lookup(current, &part)? {
                Some(entry) if entry.entry_type == EntryType::Dir => {
                    current = entry.inode_id;
                }
                Some(_) => bail!("{walked} exists and is not a directory"),
                None => {
                    let op = self.apply_local(
                        identity,
                        FsOpKind::Mkdir {
                            parent: current,
                            name: part.clone(),
                            mode: 0o40755,
                        },
                        now,
                    )?;
                    current = crate::inode::inode_for_op(op.id);
                }
            }
        }

        Ok(current)
    }

    /// Write `data` to `path`, creating parent directories and the file as needed.
    ///
    /// An existing file is overwritten in place — the inode is reused, so history and
    /// any other links to it survive.
    pub fn write_file(
        &self,
        identity: &Identity,
        path: &str,
        data: &[u8],
        now: u64,
    ) -> Result<u128> {
        let parts = Self::split_path(path)?;
        let Some((name, dirs)) = parts.split_last() else {
            bail!("cannot write to {path:?}: no file name");
        };

        let parent = self.mkdir_p(identity, &dirs.join("/"), now)?;

        let inode = match self.lookup(parent, name)? {
            Some(entry) if entry.entry_type == EntryType::File => entry.inode_id,
            Some(_) => bail!("{path} exists and is not a file"),
            None => {
                let op = self.apply_local(
                    identity,
                    FsOpKind::CreateFile {
                        parent,
                        name: name.clone(),
                        mode: 0o100644,
                    },
                    now,
                )?;
                crate::inode::inode_for_op(op.id)
            }
        };

        // Chunks must be in the store before the write is applied, or it parks. The
        // key material returned here rides along in the operation so that whoever
        // applies it records the same encryption state.
        let (chunks, encryption) = self.store_content(data)?;
        self.apply_local(
            identity,
            FsOpKind::Write {
                inode,
                offset: 0,
                chunks,
                new_size: data.len() as u64,
                encryption,
            },
            now,
        )?;

        Ok(inode)
    }

    /// Unlink `path` from its parent directory.
    pub fn remove_path(&self, identity: &Identity, path: &str, now: u64) -> Result<()> {
        let Some((parent, name)) = self.resolve_parent(path)? else {
            bail!("no such path: {path}");
        };
        if self.lookup(parent, &name)?.is_none() {
            bail!("no such path: {path}");
        }
        // Record what this removal actually saw. Deriving it later, on whichever
        // replica happens to apply the op, would make the result depend on arrival
        // order.
        let observed = self.observed_entry_ops(parent, &name)?;
        self.apply_local(
            identity,
            FsOpKind::Unlink {
                parent,
                name,
                observed,
            },
            now,
        )?;
        Ok(())
    }

    /// Move or rename an entry.
    pub fn rename_path(&self, identity: &Identity, from: &str, to: &str, now: u64) -> Result<()> {
        let Some((old_parent, old_name)) = self.resolve_parent(from)? else {
            bail!("no such path: {from}");
        };
        if self.lookup(old_parent, &old_name)?.is_none() {
            bail!("no such path: {from}");
        }
        let Some((new_parent, new_name)) = self.resolve_parent(to)? else {
            bail!("destination directory of {to} does not exist");
        };

        let observed = self.observed_entry_ops(old_parent, &old_name)?;
        self.apply_local(
            identity,
            FsOpKind::Rename {
                old_parent,
                old_name,
                new_parent,
                new_name,
                observed,
            },
            now,
        )?;
        Ok(())
    }

    /// Depth-first listing of everything beneath `root`, root itself excluded.
    ///
    /// Used by object listings, which present a flat keyspace over a real tree.
    pub fn walk(&self, root: &str) -> Result<Vec<WalkEntry>> {
        let Some((inode, kind)) = self.resolve_path(root)? else {
            bail!("no such path: {root}");
        };
        if kind != EntryType::Dir {
            bail!("not a directory: {root}");
        }

        let base = root.trim_end_matches('/').to_string();
        let mut out = Vec::new();
        self.walk_into(inode, &base, 0, &mut out)?;
        Ok(out)
    }

    fn walk_into(
        &self,
        inode: u128,
        prefix: &str,
        depth: usize,
        out: &mut Vec<WalkEntry>,
    ) -> Result<()> {
        // Rename cannot create a cycle (apply refuses to move a directory into its own
        // subtree), so depth is bounded by real nesting; the cap is belt-and-braces.
        if out.len() > MAX_WALK_ENTRIES {
            bail!("listing exceeded {MAX_WALK_ENTRIES} entries");
        }

        for entry in self.materialize_dir(inode)? {
            let path = format!("{prefix}/{}", entry.name);
            let record = self.load_inode(entry.inode_id)?;
            let (size, mtime) = record
                .map(|r| (r.content.value.size, r.content.value.mtime_unix_ms))
                .unwrap_or((0, 0));

            out.push(WalkEntry {
                path: path.clone(),
                name: entry.name.clone(),
                kind: entry.entry_type,
                depth,
                size: if entry.entry_type == EntryType::File {
                    size
                } else {
                    0
                },
                inode: entry.inode_id,
                mtime_unix_ms: mtime,
            });

            if entry.entry_type == EntryType::Dir {
                self.walk_into(entry.inode_id, &path, depth + 1, out)?;
            }
        }
        Ok(())
    }

    /// Size and modification time of a file, or `None` if it does not exist.
    pub fn stat_file(&self, path: &str) -> Result<Option<(u128, u64, u64)>> {
        let Some((inode, kind)) = self.resolve_path(path)? else {
            return Ok(None);
        };
        if kind != EntryType::File {
            return Ok(None);
        }
        let record = self
            .load_inode(inode)?
            .with_context(|| format!("inode record missing for {path}"))?;
        Ok(Some((
            inode,
            record.content.value.size,
            record.content.value.mtime_unix_ms,
        )))
    }
}

/// The inode an operation is *about*, for a commitment proof to name.
///
/// One entry per operation rather than everything it touched: a proof of "this is what
/// the operation did" needs a subject, and a creation's subject is the thing created
/// while a removal's is the directory it was removed from. Returning `None` where there
/// is no meaningful single subject makes the caller fall back rather than guess.
fn subject_inode(op_id: &nexusfs_proto::OpId, kind: &FsOpKind) -> Option<u128> {
    match kind {
        // The created inode is derived from the operation that created it.
        FsOpKind::Mkdir { .. } | FsOpKind::CreateFile { .. } => {
            Some(crate::inode::inode_for_op(*op_id))
        }
        FsOpKind::Write { inode, .. } | FsOpKind::SetAttr { inode, .. } => Some(*inode),
        // The entry is gone, so the provable fact is about the directory it left.
        FsOpKind::Unlink { parent, .. } => Some(*parent),
        FsOpKind::Rename { new_parent, .. } => Some(*new_parent),
    }
}
