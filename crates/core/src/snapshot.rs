use std::collections::{BTreeMap, BTreeSet};

use anyhow::{bail, Result};

use nexusfs_proto::types::DeviceId;
use nexusfs_storage::Hash;

use crate::inode::ROOT_INODE;
use crate::namespace::AttrState;
use crate::object::{DirNode, EntryType, Object, ObjectHeader, SnapshotRoot};
use crate::state::CoreState;

/// Domain separator for the inode-map commitment.
const INODE_MAP_DOMAIN: &[u8] = b"nexusfs/inode-map/v1";

/// Backstop against a malformed tree walking forever.
const MAX_TREE_NODES: usize = 1_000_000;

pub fn new_snapshot_root(
    root_dir_inode: u128,
    inode_map_root: Option<Hash>,
    author: DeviceId,
    now_ms: u64,
) -> SnapshotRoot {
    SnapshotRoot {
        header: ObjectHeader {
            type_tag: 3,
            version: 1,
        },
        root_dir_inode,
        inode_map_root,
        timestamp_unix_ms: now_ms,
        author,
    }
}

impl CoreState {
    /// Materialize the whole namespace into CAS objects and commit to it.
    ///
    /// Two commitments are produced, and both are needed:
    /// - each directory becomes a canonical `DirNode`, committing to *structure*
    ///   (which names map to which inodes);
    /// - `inode_map_root` commits to *content* (which object each inode currently
    ///   resolves to).
    ///
    /// A `DirNode` alone would not change when a file's bytes change, since it stores
    /// inode ids rather than content hashes. The inode map closes that gap, so the
    /// state commitment moves whenever anything in the tree moves.
    pub fn build_snapshot(&self) -> Result<Hash> {
        let state_root = self.compute_state_root()?;
        self.set_state_root(state_root)?;

        let snapshot = new_snapshot_root(
            ROOT_INODE,
            Some(state_root),
            self.device_id,
            self.state_time()?,
        );
        let head = self.put_object(&Object::SnapshotRoot(snapshot))?;
        self.set_head(head)?;
        Ok(head)
    }

    /// The pure state commitment: a function of applied operations only.
    ///
    /// This — not the head hash — is what two replicas compare to decide whether they
    /// have converged. The head additionally carries provenance (`author`), which is
    /// necessarily per-device.
    pub fn compute_state_root(&self) -> Result<Hash> {
        let mut inode_map: BTreeMap<u128, Hash> = BTreeMap::new();
        let mut visited: BTreeSet<u128> = BTreeSet::new();
        self.materialize_tree(ROOT_INODE, &mut inode_map, &mut visited, true)?;
        Ok(commit_inode_map(&inode_map))
    }

    /// Walk the live tree, recording each inode's current object hash.
    ///
    /// `store` controls whether the `DirNode`s it builds are written to the CAS.
    /// Snapshotting needs them persisted; garbage collection only needs their hashes,
    /// and writing there would be worse than pointless — the sled backend flushes on
    /// every put, so a survey would fsync once per directory while claiming to be
    /// read-only.
    pub(crate) fn materialize_tree(
        &self,
        inode: u128,
        inode_map: &mut BTreeMap<u128, Hash>,
        visited: &mut BTreeSet<u128>,
        store: bool,
    ) -> Result<Hash> {
        if !visited.insert(inode) {
            bail!("cycle detected in directory tree at inode {inode:x}");
        }
        if visited.len() > MAX_TREE_NODES {
            bail!("directory tree exceeded {MAX_TREE_NODES} nodes");
        }

        let entries = self.materialize_dir(inode)?;

        for entry in &entries {
            match entry.entry_type {
                EntryType::Dir => {
                    self.materialize_tree(entry.inode_id, inode_map, visited, store)?;
                }
                EntryType::File => {
                    if let Some(record) = self.load_inode(entry.inode_id)? {
                        if let Some(hash) = record.content.value.node_hash {
                            inode_map.insert(entry.inode_id, hash);
                        }
                    }
                }
            }
        }

        // Metadata comes from the inode record, never from wall-clock: the snapshot
        // has to be reproducible from state alone.
        let record = self.load_inode(inode)?;
        let attrs = record.as_ref().map(|r| r.attrs.value).unwrap_or(AttrState {
            mode: 0o40755,
            uid: 0,
            gid: 0,
        });
        let (mtime, ctime) = record
            .as_ref()
            .map(|r| (r.content.value.mtime_unix_ms, r.ctime_unix_ms))
            .unwrap_or((0, 0));

        let mut dir = DirNode {
            header: ObjectHeader {
                type_tag: 2,
                version: 1,
            },
            entries,
            mode: attrs.mode,
            uid: attrs.uid,
            gid: attrs.gid,
            mtime_unix_ms: mtime,
            ctime_unix_ms: ctime,
        };
        dir.canonicalize();

        let object = Object::DirNode(dir);
        let hash = if store {
            self.put_object(&object)?
        } else {
            self.object_hash(&object)?
        };
        inode_map.insert(inode, hash);
        Ok(hash)
    }
}

/// BLAKE3 over the sorted `(inode, hash)` pairs.
///
/// A flat commitment is enough to make the head meaningful today. Replacing it with a
/// SNARK-friendly Merkle map later changes only this function and the ZkCommit path.
fn commit_inode_map(map: &BTreeMap<u128, Hash>) -> Hash {
    let mut hasher = blake3::Hasher::new();
    hasher.update(INODE_MAP_DOMAIN);
    hasher.update(&(map.len() as u64).to_be_bytes());
    for (inode, hash) in map {
        hasher.update(&inode.to_be_bytes());
        hasher.update(hash);
    }
    *hasher.finalize().as_bytes()
}
