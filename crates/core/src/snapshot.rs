use std::collections::{BTreeMap, BTreeSet};

use anyhow::{bail, Result};

use nexusfs_proto::types::DeviceId;
use nexusfs_storage::Hash;

use crate::inode::ROOT_INODE;
use crate::namespace::{imap_key, parse_imap_key, AttrState, CF_STATE, IMAP_PREFIX};
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
    /// Read from the maintained map rather than walked. The walk is what an apply used
    /// to spend most of its time on — 2.8ms of a 3.7ms state root at a thousand
    /// entries — and it grows with the tree while the change that triggered it does not.
    ///
    /// Sorted and unique because the keys are fixed-width big-endian inodes, so the
    /// store's byte order is inode order — which is what the commitment requires.
    pub fn inode_map(&self) -> Result<Vec<(u128, Hash)>> {
        let rows = self.stores.kv.scan_prefix(CF_STATE, IMAP_PREFIX)?;
        let mut out = Vec::with_capacity(rows.len());
        for (key, value) in rows {
            let (Some(inode), Ok(hash)) = (parse_imap_key(&key), <[u8; 32]>::try_from(&value[..]))
            else {
                // Name the row: this aborts every read of the state root, so the one
                // thing an operator needs is which entry to look at.
                bail!(
                    "inode map holds a malformed entry: key {} ({} bytes), value {} bytes",
                    key.iter().map(|b| format!("{b:02x}")).collect::<String>(),
                    key.len(),
                    value.len()
                );
            };
            out.push((inode, hash));
        }
        Ok(out)
    }

    /// Rebuild the map from a full walk, replacing whatever was there.
    ///
    /// The authority the incremental path is checked against, and the fallback whenever
    /// an operation could change *which* inodes are reachable rather than merely what
    /// they hold.
    pub fn rebuild_inode_map(&self) -> Result<Vec<(u128, Hash)>> {
        let mut fresh: BTreeMap<u128, Hash> = BTreeMap::new();
        let mut visited: BTreeSet<u128> = BTreeSet::new();
        self.materialize_tree(ROOT_INODE, &mut fresh, &mut visited, true)?;

        for (key, _) in self.stores.kv.scan_prefix(CF_STATE, IMAP_PREFIX)? {
            if let Some(inode) = parse_imap_key(&key) {
                if !fresh.contains_key(&inode) {
                    self.stores.kv.delete_kv(CF_STATE, &key)?;
                }
            }
        }
        for (inode, hash) in &fresh {
            self.stores.kv.put_kv(CF_STATE, &imap_key(*inode), hash)?;
        }
        Ok(fresh.into_iter().collect())
    }

    /// Recompute one inode's entry, or drop it when the inode is not reachable.
    ///
    /// Reachability is checked here rather than by the caller so that patching is
    /// always safe: an operation applied inside a directory that has since been
    /// unlinked must add nothing, and this is the single place that decides.
    pub(crate) fn refresh_map_entry(&self, inode: u128) -> Result<()> {
        let key = imap_key(inode);

        if !self.is_reachable(inode)? {
            self.stores.kv.delete_kv(CF_STATE, &key)?;
            return Ok(());
        }

        let Some(record) = self.load_inode(inode)? else {
            self.stores.kv.delete_kv(CF_STATE, &key)?;
            return Ok(());
        };

        match record.kind {
            EntryType::Dir => {
                let hash = self.store_dir_object(inode)?;
                self.stores.kv.put_kv(CF_STATE, &key, &hash)?;
            }
            // A file with no content yet is absent from the map, exactly as the walk
            // leaves it — the commitment is over content, and there is none.
            EntryType::File => match record.content.value.node_hash {
                Some(hash) => self.stores.kv.put_kv(CF_STATE, &key, &hash)?,
                None => self.stores.kv.delete_kv(CF_STATE, &key)?,
            },
        }
        Ok(())
    }

    /// Whether `inode` is reachable from the root.
    ///
    /// Directories always carry a map entry when reachable, so membership answers this
    /// outright for them. A file may legitimately have no entry — it has no content yet
    /// — so it is answered through the parent recorded when it was created, confirming
    /// the parent still lists it.
    pub(crate) fn is_reachable(&self, inode: u128) -> Result<bool> {
        let mut current = inode;
        let mut hops = 0usize;

        loop {
            if current == ROOT_INODE {
                return Ok(true);
            }
            if self
                .stores
                .kv
                .get_kv(CF_STATE, &imap_key(current))?
                .is_some()
            {
                return Ok(true);
            }

            let Some(parent) = self.parent_of(current)? else {
                return Ok(false);
            };
            if !self.dir_lists(parent, current)? {
                return Ok(false);
            }

            current = parent;
            hops += 1;
            if hops > MAX_TREE_NODES {
                bail!("parent chain for inode {inode:x} does not terminate");
            }
        }
    }

    /// Build one directory's canonical object from state.
    ///
    /// The single construction site, called by both the patch path
    /// ([`store_dir_object`]) and the full walk ([`materialize_tree`]). Two copies
    /// would be a state root that depends on which path produced it: a field added to
    /// `DirNode` in one and not the other splits the commitment silently, and the
    /// split only shows up as two converged replicas disagreeing.
    ///
    /// Metadata comes from the inode record, never from wall-clock: the snapshot has
    /// to be reproducible from state alone.
    fn dir_object(&self, inode: u128, entries: Vec<crate::object::DirEntry>) -> Result<Object> {
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
        Ok(Object::DirNode(dir))
    }

    /// Materialize and store one directory's object, returning its hash.
    fn store_dir_object(&self, inode: u128) -> Result<Hash> {
        let entries = self.materialize_dir(inode)?;
        let object = self.dir_object(inode, entries)?;
        self.put_object(&object)
    }

    /// Whether `parent` currently lists `child` among its live entries.
    fn dir_lists(&self, parent: u128, child: u128) -> Result<bool> {
        Ok(self
            .materialize_dir(parent)?
            .iter()
            .any(|e| e.inode_id == child))
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

    /// Prove that `inode` is *not* in the current state.
    ///
    /// The pairing that makes this worth having: an inclusion proof against an earlier
    /// root and an absence proof against a later one together demonstrate a deletion,
    /// to someone who holds neither state.
    ///
    /// `None` when the inode is present — the caller wants the other kind of proof.
    pub fn absence_proof(&self, inode: u128) -> Result<Option<nexusfs_zk::merkle::AbsenceProof>> {
        Ok(nexusfs_zk::merkle::prove_absent(&self.inode_map()?, inode))
    }

    /// Inclusion proofs for many entries, sharing one traversal of the tree.
    pub fn inclusion_proofs(
        &self,
        inodes: &[u128],
    ) -> Result<Vec<nexusfs_zk::merkle::InclusionProof>> {
        Ok(nexusfs_zk::merkle::prove_many(&self.inode_map()?, inodes))
    }

    /// The pure state commitment: a function of applied operations only.
    ///
    /// This — not the head hash — is what two replicas compare to decide whether they
    /// have converged. The head additionally carries provenance (`author`), which is
    /// necessarily per-device.
    pub fn compute_state_root(&self) -> Result<Hash> {
        Ok(nexusfs_zk::merkle::commit(&self.inode_map()?))
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

        let object = self.dir_object(inode, entries)?;
        let hash = if store {
            self.put_object(&object)?
        } else {
            self.object_hash(&object)?
        };
        inode_map.insert(inode, hash);
        Ok(hash)
    }
}
