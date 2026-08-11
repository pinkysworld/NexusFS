//! Reclaiming storage that nothing refers to any more.
//!
//! # Where the garbage comes from
//!
//! Content addressing means nothing is ever overwritten in place. Rewriting a file
//! stores new chunks and a new `FileNode`; the old ones simply stop being referenced.
//!
//! The larger source is quieter. Every applied operation rebuilds the snapshot, which
//! stores a fresh `SnapshotRoot` and re-materializes a `DirNode` for every directory on
//! the path to the change. The previous ones become unreachable immediately. So a
//! repository accrues garbage per *operation*, not per overwrite — even a history with
//! nothing superseded and nothing deleted has objects to reclaim, and none of it shows
//! up as deleted data. The store just grows.
//!
//! # What "reachable" means here
//!
//! Mark-and-sweep from two roots:
//!
//! 1. **Live state.** Walking the namespace from the root inode yields every current
//!    `DirNode` and `FileNode`, and each `FileNode` names its chunks.
//! 2. **Pending operations.** An operation parked waiting for content already holds
//!    references to chunks that have not arrived, and may hold ones that have. Those
//!    must survive, or the operation could never be applied.
//!
//! Deliberately *not* roots:
//!
//! - **Superseded file versions.** Once a write loses the content register, no reader
//!   can reach its bytes. This is only safe because `apply` treats a superseded write
//!   as satisfied without demanding its chunks — otherwise a peer replaying history
//!   would ask for content that had been collected here and wait for it forever.
//! - **Historical snapshot roots.** Only the current head is reachable. Proofs record
//!   state roots, which are commitments computed from state rather than stored objects,
//!   so an audit does not need old snapshots to survive.
//!
//! # Why the sweep checks before it marks
//!
//! A repository that reaches nothing is indistinguishable from one whose every object
//! is garbage, and acting on that mistake deletes the lot. The obvious guard — refuse
//! when the reachable set comes back empty — does not work: marking walks the tree by
//! rebuilding it, so a repository whose state is gone still yields one hash, that of a
//! freshly materialized empty root. The set is non-empty and meaningless.
//!
//! So the preconditions are checked first, against the two things every live repository
//! has: a head, and a root inode record. Missing either means corruption or a
//! half-finished restore, which is exactly when deleting is worst.
//!
//! Marking writes nothing. It rebuilds each directory object in memory and takes its
//! hash, which is what keeps a survey genuinely read-only.
//!
//! The same caution is why the admin endpoint only ever surveys: the daemon could write
//! a blob between the mark and the sweep, and that blob would look like garbage.

use std::collections::BTreeSet;

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use tracing::{debug, info};

use nexusfs_proto::FsOpKind;
use nexusfs_storage::Hash;

use crate::inode::ROOT_INODE;
use crate::object::Object;
use crate::state::CoreState;

/// What one collection pass found, and what it did about it.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct GcReport {
    pub blobs_scanned: usize,
    pub bytes_scanned: u64,
    pub reachable: usize,
    pub unreachable: usize,
    /// Bytes held by unreachable blobs, whether or not they were deleted.
    pub bytes_reclaimable: u64,
    pub deleted: usize,
    pub bytes_deleted: u64,
    /// True when this was a survey and nothing was removed.
    pub dry_run: bool,
    /// Set when the sweep declined to delete despite not being a dry run.
    pub refused: Option<String>,
}

impl GcReport {
    /// Whether anything would be, or was, reclaimed.
    pub fn has_garbage(&self) -> bool {
        self.unreachable > 0
    }
}

impl CoreState {
    /// Every object hash the repository can still reach.
    pub fn reachable_hashes(&self) -> Result<BTreeSet<Hash>> {
        let mut reachable = BTreeSet::new();

        // The head names the current snapshot object.
        if let Some(head) = self.get_head()? {
            reachable.insert(head);
        }

        // Walking the live tree yields a hash per inode: a `DirNode` for directories,
        // a `FileNode` for files. Hashes only — a survey must not write.
        let mut inode_map = std::collections::BTreeMap::new();
        let mut visited = BTreeSet::new();
        self.materialize_tree(ROOT_INODE, &mut inode_map, &mut visited, false)
            .context("walk live namespace state")?;

        for hash in inode_map.values() {
            reachable.insert(*hash);
            // A `FileNode` names chunks; a `DirNode` names no blobs of its own.
            if let Some(Object::FileNode(node)) = self.get_object(hash)? {
                for chunk in &node.chunks {
                    reachable.insert(chunk.hash);
                }
            }
        }

        // Content an unapplied operation is still waiting on, plus content it has
        // already received. Collecting either would strand the operation.
        for op in self.pending_ops()? {
            if let FsOpKind::Write { chunks, .. } = &op.kind {
                for chunk in chunks {
                    reachable.insert(chunk.hash);
                }
            }
        }

        Ok(reachable)
    }

    /// Why this repository is not safe to sweep, if it is not.
    ///
    /// Checked *before* marking rather than after. Marking rebuilds the tree it walks,
    /// so even a repository with no state left produces the hash of an empty root — the
    /// reachable set comes back non-empty and useless, and "nothing was reachable" can
    /// never be observed.
    fn sweep_blocker(&self) -> Result<Option<String>> {
        if self.get_head()?.is_none() {
            return Ok(Some(
                "the repository has no head, so live state cannot be established; \
                 bootstrap or restore it before collecting"
                    .into(),
            ));
        }
        if self.load_inode(ROOT_INODE)?.is_none() {
            return Ok(Some(
                "the root inode record is missing, so the namespace walk would find \
                 nothing and every object would look like garbage"
                    .into(),
            ));
        }
        Ok(None)
    }

    /// Survey unreachable storage, and delete it unless `dry_run`.
    pub fn collect_garbage(&self, dry_run: bool) -> Result<GcReport> {
        let mut report = GcReport {
            dry_run,
            ..Default::default()
        };

        let blocker = self.sweep_blocker()?;
        if !dry_run {
            if let Some(reason) = blocker {
                report.refused = Some(reason);
                let stored = self.stores.blobs.list()?;
                report.blobs_scanned = stored.len();
                report.bytes_scanned = stored.iter().map(|(_, len)| len).sum();
                return Ok(report);
            }
        }

        let reachable = self.reachable_hashes()?;
        let stored = self.stores.blobs.list()?;

        report.blobs_scanned = stored.len();
        report.bytes_scanned = stored.iter().map(|(_, len)| len).sum();

        let mut garbage = Vec::new();
        for (hash, len) in stored {
            if reachable.contains(&hash) {
                report.reachable += 1;
            } else {
                report.unreachable += 1;
                report.bytes_reclaimable += len;
                garbage.push((hash, len));
            }
        }

        debug!(
            scanned = report.blobs_scanned,
            reachable = report.reachable,
            unreachable = report.unreachable,
            "garbage collection survey complete"
        );

        if dry_run {
            return Ok(report);
        }

        for (hash, len) in garbage {
            self.stores.blobs.delete(&hash)?;
            report.deleted += 1;
            report.bytes_deleted += len;
        }

        self.flush()?;
        info!(
            deleted = report.deleted,
            bytes = report.bytes_deleted,
            "reclaimed unreachable storage"
        );
        Ok(report)
    }
}
