use std::collections::{BTreeMap, BTreeSet};

use anyhow::{bail, Result};

use nexusfs_proto::types::DeviceId;
use nexusfs_storage::Hash;

use crate::inode::ROOT_INODE;
use crate::namespace::AttrState;
use crate::object::{DirNode, EntryType, Object, ObjectHeader, SnapshotRoot};
use crate::state::CoreState;

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

    /// The live inode map: exactly the leaves the state root commits to.
    ///
    /// Walks without storing, so building a proof does not write. Sorted and unique by
    /// construction — it comes from a `BTreeMap`, which is what the commitment requires.
    pub fn inode_map(&self) -> Result<Vec<(u128, Hash)>> {
        let mut inode_map: BTreeMap<u128, Hash> = BTreeMap::new();
        let mut visited: BTreeSet<u128> = BTreeSet::new();
        self.materialize_tree(ROOT_INODE, &mut inode_map, &mut visited, false)?;
        Ok(inode_map.into_iter().collect())
    }

    /// Prove that `inode` holds its current object hash in the current state root.
    ///
    /// `None` when the inode is not in the live tree. The proof is self-contained: a
    /// verifier needs it and the root, nothing else.
    pub fn inclusion_proof(
        &self,
        inode: u128,
    ) -> Result<Option<nexusfs_zk::merkle::InclusionProof>> {
        Ok(nexusfs_zk::merkle::prove(&self.inode_map()?, inode))
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

/// A Merkle commitment over the sorted `(inode, hash)` pairs.
///
/// This used to be one flat BLAKE3 over the whole list, which could say only "two
/// replicas agree" — convincing anyone of a single fact meant handing them the entire
/// state. A Merkle root commits to the same thing while making each entry provable on
/// its own. Changing it changes what replicas compare, which is why it arrived with an
/// on-disk format bump and a protocol version bump rather than quietly.
fn commit_inode_map(map: &BTreeMap<u128, Hash>) -> Hash {
    let entries: Vec<(u128, Hash)> = map.iter().map(|(k, v)| (*k, *v)).collect();
    nexusfs_zk::merkle::commit(&entries)
}
