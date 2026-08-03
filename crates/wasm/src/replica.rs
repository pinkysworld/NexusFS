//! One in-browser NexusFS node.

use std::sync::Arc;

use anyhow::{bail, Result};

use nexusfs_core::{CoreState, Stores};
use nexusfs_crypto::Identity;
use nexusfs_proto::FsOp;
use nexusfs_storage::mem_store::MemStore;

/// A content-addressed blob as it moves between nodes: `(hash, bytes)`.
pub type Blob = (nexusfs_proto::Hash, Vec<u8>);

pub struct Replica {
    pub name: String,
    pub core: CoreState,
    pub identity: Identity,
    pub store: MemStore,
    /// Set while the node is "offline" in the demo, so the UI can queue rather than sync.
    pub online: bool,
}

impl Replica {
    pub fn new(name: String, device_id: u128, seed: [u8; 32]) -> Result<Self> {
        let store = MemStore::new();
        let stores = Stores {
            blobs: Arc::new(store.clone()),
            kv: Arc::new(store.clone()),
        };
        let mut core = CoreState::new(stores, nexusfs_proto::DeviceId(device_id));
        // Small chunks so the playground shows multi-chunk files without needing
        // megabyte pastes.
        core.chunk_size = 64;
        core.bootstrap_if_needed()?;

        Ok(Self {
            name,
            core,
            identity: Identity::from_seed(seed),
            store,
            online: true,
        })
    }

    pub fn mkdir(&self, path: &str, now: u64) -> Result<()> {
        let Some((parent, name)) = self.core.resolve_parent(path)? else {
            bail!("parent directory of {path} does not exist");
        };
        if self.core.lookup(parent, &name)?.is_some() {
            bail!("{path} already exists");
        }
        self.core.mkdir_p(&self.identity, path, now)?;
        Ok(())
    }

    pub fn put(&self, path: &str, content: &[u8], now: u64) -> Result<()> {
        self.core.write_file(&self.identity, path, content, now)?;
        Ok(())
    }

    pub fn rm(&self, path: &str, now: u64) -> Result<()> {
        self.core.remove_path(&self.identity, path, now)
    }

    pub fn mv(&self, from: &str, to: &str, now: u64) -> Result<()> {
        self.core.rename_path(&self.identity, from, to, now)
    }

    /// Everything this node holds: its oplog and its blobs.
    ///
    /// This is deliberately the same pair the replication protocol moves — ops first,
    /// then the content they reference.
    pub fn export(&self) -> Result<(Vec<FsOp>, Vec<Blob>)> {
        Ok((self.core.all_ops()?, self.store.all_blobs()))
    }

    /// Accept another node's operations and content.
    ///
    /// Blobs land first so that writes are applicable on arrival; anything still
    /// missing a dependency parks and is retried. Returns how many ops newly applied.
    pub fn import(&self, ops: Vec<FsOp>, blobs: Vec<Blob>) -> Result<usize> {
        for (hash, bytes) in blobs {
            // Content-addressed: verify before trusting a peer's label for it.
            let actual = nexusfs_core::hash_bytes(&bytes);
            if actual != hash {
                bail!("blob hash mismatch on import");
            }
            self.core.stores.blobs.put(hash, &bytes)?;
        }

        // Count via the applied-set rather than per-call outcomes: applying one op can
        // unblock several parked ones inside the same call, so summing return values
        // undercounts badly.
        let before = self.core.applied_count()?;
        for op in ops {
            self.core.apply_op(&op)?;
        }
        self.core.retry_pending()?;
        Ok(self.core.applied_count()?.saturating_sub(before))
    }
}
