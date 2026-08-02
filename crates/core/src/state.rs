use std::collections::BTreeMap;
use std::sync::Arc;

use anyhow::Result;
use tracing::info;

use nexusfs_crdt::conflicts;
use nexusfs_proto::{ClockSummary, DeviceId, FsOp, Hash, OpId};
use nexusfs_storage::{BlobStore, KvStore};

use crate::chunker::store_chunks_with_refs;
use crate::codec::{decode, encode, encode_object};
use crate::hash::hash_bytes;
use crate::inode::ROOT_INODE;
use crate::namespace::InodeRecord;
use crate::object::{ChunkRef, DirEntry, DirNode, EntryType, FileNode, Object, ObjectHeader};

pub(crate) const CF_META: &str = "meta";
pub(crate) const CF_OPLOG: &str = "oplog";

const KEY_HEAD_CURRENT: &[u8] = b"head/current";
const KEY_STATE_ROOT: &[u8] = b"state/root";
const KEY_STATE_TIME: &[u8] = b"state/time";

/// Column-family prefix keys for oplog entries.
const OP_PREFIX: &[u8] = b"op\0";
const APPLIED_PREFIX: &[u8] = b"applied\0";

fn op_key(device_id: DeviceId, counter: u64) -> Vec<u8> {
    let mut k = Vec::with_capacity(OP_PREFIX.len() + 16 + 8);
    k.extend_from_slice(OP_PREFIX);
    k.extend_from_slice(&device_id.0.to_be_bytes());
    k.extend_from_slice(&counter.to_be_bytes());
    k
}

fn parse_op_key(key: &[u8]) -> Option<(DeviceId, u64)> {
    if !key.starts_with(OP_PREFIX) || key.len() != OP_PREFIX.len() + 16 + 8 {
        return None;
    }
    let mut did = [0u8; 16];
    did.copy_from_slice(&key[OP_PREFIX.len()..OP_PREFIX.len() + 16]);
    let device_id = DeviceId(u128::from_be_bytes(did));

    let mut ctr = [0u8; 8];
    ctr.copy_from_slice(&key[OP_PREFIX.len() + 16..OP_PREFIX.len() + 16 + 8]);
    let counter = u64::from_be_bytes(ctr);

    Some((device_id, counter))
}

fn applied_key(device_id: DeviceId, counter: u64) -> Vec<u8> {
    let mut k = Vec::with_capacity(APPLIED_PREFIX.len() + 16 + 8);
    k.extend_from_slice(APPLIED_PREFIX);
    k.extend_from_slice(&device_id.0.to_be_bytes());
    k.extend_from_slice(&counter.to_be_bytes());
    k
}

/// Stores needed by the core. Usually backed by the same DB, but abstracted.
#[derive(Clone)]
pub struct Stores {
    pub blobs: Arc<dyn BlobStore>,
    pub kv: Arc<dyn KvStore>,
}

#[derive(Clone)]
pub struct CoreState {
    pub stores: Stores,
    pub device_id: DeviceId,
    pub chunk_size: usize,
}

impl CoreState {
    pub fn new(stores: Stores, device_id: DeviceId) -> Self {
        Self {
            stores,
            device_id,
            chunk_size: 1024 * 1024,
        }
    }

    /// Store an encoded object in the CAS keyed by its BLAKE3 hash.
    pub fn put_object(&self, obj: &Object) -> Result<Hash> {
        let bytes = encode_object(obj)?;
        let h = hash_bytes(&bytes);
        self.stores.blobs.put(h, &bytes)?;
        Ok(h)
    }

    pub fn get_object(&self, h: &Hash) -> Result<Option<Object>> {
        let Some(bytes) = self.stores.blobs.get(h)? else {
            return Ok(None);
        };
        let obj: Object = decode(&bytes)?;
        Ok(Some(obj))
    }

    // ---- head and state commitment ------------------------------------------

    pub fn get_head(&self) -> Result<Option<Hash>> {
        self.read_hash(KEY_HEAD_CURRENT)
    }

    pub fn set_head(&self, head: Hash) -> Result<()> {
        self.stores.kv.put_kv(CF_META, KEY_HEAD_CURRENT, &head)?;
        Ok(())
    }

    /// Last computed state commitment. See `compute_state_root`.
    pub fn get_state_root(&self) -> Result<Option<Hash>> {
        self.read_hash(KEY_STATE_ROOT)
    }

    pub(crate) fn set_state_root(&self, root: Hash) -> Result<()> {
        self.stores.kv.put_kv(CF_META, KEY_STATE_ROOT, &root)?;
        Ok(())
    }

    fn read_hash(&self, key: &[u8]) -> Result<Option<Hash>> {
        if let Some(v) = self.stores.kv.get_kv(CF_META, key)? {
            if v.len() == 32 {
                let mut h = [0u8; 32];
                h.copy_from_slice(&v);
                return Ok(Some(h));
            }
        }
        Ok(None)
    }

    /// Highest operation timestamp applied so far.
    ///
    /// Used as the snapshot timestamp. `max` is commutative, so replaying the same
    /// operations in a different order yields the same value — which is what lets a
    /// replica's head be stable rather than dependent on arrival order.
    pub(crate) fn state_time(&self) -> Result<u64> {
        Ok(self
            .stores
            .kv
            .get_kv(CF_META, KEY_STATE_TIME)?
            .and_then(|v| v.try_into().ok())
            .map(u64::from_be_bytes)
            .unwrap_or(0))
    }

    pub(crate) fn observe_time(&self, time_unix_ms: u64) -> Result<()> {
        let current = self.state_time()?;
        if time_unix_ms > current {
            self.stores
                .kv
                .put_kv(CF_META, KEY_STATE_TIME, &time_unix_ms.to_be_bytes())?;
        }
        Ok(())
    }

    // ---- oplog ----------------------------------------------------------------

    /// Append an operation to the oplog. This does NOT automatically apply it.
    pub fn append_op(&self, op: &FsOp) -> Result<()> {
        let key = op_key(op.id.device_id, op.id.counter);
        let val = encode(op)?;
        self.stores.kv.put_kv(CF_OPLOG, &key, &val)?;
        Ok(())
    }

    pub fn get_op(&self, op_id: OpId) -> Result<Option<FsOp>> {
        let key = op_key(op_id.device_id, op_id.counter);
        let Some(bytes) = self.stores.kv.get_kv(CF_OPLOG, &key)? else {
            return Ok(None);
        };
        Ok(Some(decode(&bytes)?))
    }

    /// Most recent operations by device/counter order, newest first.
    pub fn recent_ops(&self, limit: usize) -> Result<Vec<FsOp>> {
        let mut rows = self.stores.kv.scan_prefix(CF_OPLOG, OP_PREFIX)?;
        rows.reverse();
        rows.into_iter()
            .take(limit)
            .map(|(_k, v)| decode::<FsOp>(&v))
            .collect()
    }

    pub fn op_count(&self) -> Result<usize> {
        Ok(self.stores.kv.scan_prefix(CF_OPLOG, OP_PREFIX)?.len())
    }

    pub fn applied_count(&self) -> Result<usize> {
        Ok(self.stores.kv.scan_prefix(CF_OPLOG, APPLIED_PREFIX)?.len())
    }

    pub fn is_op_applied(&self, op_id: OpId) -> Result<bool> {
        let key = applied_key(op_id.device_id, op_id.counter);
        Ok(self.stores.kv.get_kv(CF_OPLOG, &key)?.is_some())
    }

    pub fn mark_op_applied(&self, op_id: OpId) -> Result<()> {
        let key = applied_key(op_id.device_id, op_id.counter);
        self.stores.kv.put_kv(CF_OPLOG, &key, &[1])?;
        Ok(())
    }

    /// Compute a clock summary by scanning oplog keys.
    /// This is v0 and may be optimized later.
    pub fn clock_summary(&self) -> Result<ClockSummary> {
        let rows = self.stores.kv.scan_prefix(CF_OPLOG, OP_PREFIX)?;
        let mut max: BTreeMap<DeviceId, u64> = BTreeMap::new();
        for (k, _v) in rows {
            if let Some((did, ctr)) = parse_op_key(&k) {
                let e = max.entry(did).or_insert(0);
                *e = (*e).max(ctr);
            }
        }
        Ok(ClockSummary {
            entries: max.into_iter().collect(),
        })
    }

    // ---- bootstrap --------------------------------------------------------------

    /// Create the root directory and an initial snapshot if this repository is empty.
    pub fn bootstrap_if_needed(&self) -> Result<()> {
        if self.get_head()?.is_some() {
            return Ok(());
        }

        // The root is the one inode not created by an operation, so it cannot derive
        // its identity from an OpId. It must nonetheless be byte-identical on every
        // replica — stamping it with wall-clock time or the local device id would give
        // two nodes different root DirNodes and therefore different state roots, even
        // with identical contents. Hence a fixed genesis record that any later SetAttr
        // (ts > 0) cleanly outranks.
        let root = InodeRecord::new(EntryType::Dir, 0o40755, 0, 0, 0);
        self.store_inode(ROOT_INODE, &root)?;
        self.store_dir(ROOT_INODE, &Default::default())?;

        let head = self.build_snapshot()?;
        info!(head = %hex::encode(head), "bootstrapped new repository");
        Ok(())
    }

    // ---- chunk and object helpers -----------------------------------------------

    /// Deterministic conflict name helper (used by CRDT merges).
    pub fn conflict_name(base: &str, device_id: DeviceId, time_ms: u64) -> String {
        conflicts::conflict_name(base, device_id.0, time_ms)
    }

    pub fn store_chunks(&self, data: &[u8]) -> Result<Vec<ChunkRef>> {
        store_chunks_with_refs(data, self.chunk_size, |hash, chunk| {
            self.stores.blobs.put(hash, chunk)
        })
    }

    pub fn make_filenode_with_refs(
        &self,
        chunks: Vec<ChunkRef>,
        size: u64,
        now_ms: u64,
    ) -> Result<Hash> {
        let file = FileNode {
            header: ObjectHeader {
                type_tag: 1,
                version: 1,
            },
            size,
            chunks,
            mode: 0o100644,
            uid: 0,
            gid: 0,
            mtime_unix_ms: now_ms,
            ctime_unix_ms: now_ms,
        };
        self.put_object(&Object::FileNode(file))
    }

    pub fn store_filenode_from_bytes(&self, data: &[u8], now_ms: u64) -> Result<Hash> {
        let chunk_refs = self.store_chunks(data)?;
        self.make_filenode_with_refs(chunk_refs, data.len() as u64, now_ms)
    }

    /// Build a canonical DirNode and store it.
    pub fn make_dirnode(&self, entries: Vec<DirEntry>, now_ms: u64) -> Result<Hash> {
        let dir = DirNode::new_canonical(entries, 0o40755, 0, 0, now_ms);
        self.put_object(&Object::DirNode(dir))
    }

    // ---- storage accounting -------------------------------------------------------

    pub fn blob_stats(&self) -> Result<(usize, u64)> {
        self.stores.blobs.stats()
    }
}

pub fn now_ms() -> u64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    let dur = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default();
    (dur.as_secs() * 1000) + (dur.subsec_millis() as u64)
}
