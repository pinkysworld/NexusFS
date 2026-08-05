#![forbid(unsafe_code)]

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use anyhow::Result;

use crate::{BlobStore, Hash, KvStore};

type BlobMap = BTreeMap<Hash, Vec<u8>>;
/// Keyed by `(column family, key)` so one map serves every namespace.
type KvMap = BTreeMap<(String, Vec<u8>), Vec<u8>>;

/// In-memory blob and KV store.
///
/// Exists for two reasons: tests that do not want a temporary directory, and
/// targets with no filesystem at all — the WebAssembly build runs the identical
/// `CoreState` against this backend, which is only possible because storage is a
/// trait rather than a concrete database.
#[derive(Clone, Default)]
pub struct MemStore {
    blobs: Arc<Mutex<BlobMap>>,
    kv: Arc<Mutex<KvMap>>,
}

impl MemStore {
    pub fn new() -> Self {
        Self::default()
    }
}

impl BlobStore for MemStore {
    fn put(&self, key: Hash, data: &[u8]) -> Result<()> {
        self.blobs
            .lock()
            .expect("blob map poisoned")
            .insert(key, data.to_vec());
        Ok(())
    }

    fn get(&self, key: &Hash) -> Result<Option<Vec<u8>>> {
        Ok(self
            .blobs
            .lock()
            .expect("blob map poisoned")
            .get(key)
            .cloned())
    }

    fn has(&self, key: &Hash) -> Result<bool> {
        Ok(self
            .blobs
            .lock()
            .expect("blob map poisoned")
            .contains_key(key))
    }

    fn delete(&self, key: &Hash) -> Result<()> {
        self.blobs.lock().expect("blob map poisoned").remove(key);
        Ok(())
    }

    fn list(&self) -> Result<Vec<(Hash, u64)>> {
        Ok(self
            .blobs
            .lock()
            .expect("blob map poisoned")
            .iter()
            .map(|(h, v)| (*h, v.len() as u64))
            .collect())
    }

    fn stats(&self) -> Result<(usize, u64)> {
        let blobs = self.blobs.lock().expect("blob map poisoned");
        Ok((blobs.len(), blobs.values().map(|v| v.len() as u64).sum()))
    }
}

impl KvStore for MemStore {
    fn put_kv(&self, cf: &str, key: &[u8], val: &[u8]) -> Result<()> {
        self.kv
            .lock()
            .expect("kv map poisoned")
            .insert((cf.to_string(), key.to_vec()), val.to_vec());
        Ok(())
    }

    fn get_kv(&self, cf: &str, key: &[u8]) -> Result<Option<Vec<u8>>> {
        Ok(self
            .kv
            .lock()
            .expect("kv map poisoned")
            .get(&(cf.to_string(), key.to_vec()))
            .cloned())
    }

    fn delete_kv(&self, cf: &str, key: &[u8]) -> Result<()> {
        self.kv
            .lock()
            .expect("kv map poisoned")
            .remove(&(cf.to_string(), key.to_vec()));
        Ok(())
    }

    fn scan_prefix(&self, cf: &str, prefix: &[u8]) -> Result<Vec<(Vec<u8>, Vec<u8>)>> {
        // BTreeMap keeps keys ordered, so results come back in the same order the
        // sled backend produces — callers may rely on that (see `recent_ops`).
        Ok(self
            .kv
            .lock()
            .expect("kv map poisoned")
            .iter()
            .filter(|((c, k), _)| c == cf && k.starts_with(prefix))
            .map(|((_, k), v)| (k.clone(), v.clone()))
            .collect())
    }
}

/// All blobs currently held, for replicating content to another store.
impl MemStore {
    pub fn all_blobs(&self) -> Vec<(Hash, Vec<u8>)> {
        self.blobs
            .lock()
            .expect("blob map poisoned")
            .iter()
            .map(|(h, v)| (*h, v.clone()))
            .collect()
    }
}
