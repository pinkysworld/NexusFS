#![forbid(unsafe_code)]

use anyhow::Result;

pub type Hash = [u8; 32];

pub trait BlobStore: Send + Sync {
    fn put(&self, key: Hash, data: &[u8]) -> Result<()>;
    fn get(&self, key: &Hash) -> Result<Option<Vec<u8>>>;
    fn has(&self, key: &Hash) -> Result<bool>;
    fn delete(&self, key: &Hash) -> Result<()>;

    /// `(blob count, total bytes)` for capacity reporting.
    fn stats(&self) -> Result<(usize, u64)>;

    /// Every stored blob as `(hash, stored length)`.
    ///
    /// Hashes and sizes only, never bodies: garbage collection has to walk the whole
    /// store, and materializing every blob to decide what to delete would need as much
    /// memory as the repository occupies on disk.
    fn list(&self) -> Result<Vec<(Hash, u64)>>;
}

pub trait KvStore: Send + Sync {
    fn put_kv(&self, cf: &str, key: &[u8], val: &[u8]) -> Result<()>;
    fn get_kv(&self, cf: &str, key: &[u8]) -> Result<Option<Vec<u8>>>;
    fn delete_kv(&self, cf: &str, key: &[u8]) -> Result<()>;
    fn scan_prefix(&self, cf: &str, prefix: &[u8]) -> Result<Vec<(Vec<u8>, Vec<u8>)>>;

    /// Keys under `prefix`, without their values.
    ///
    /// Separate from `scan_prefix` because several hot callers want only the keys, and
    /// materializing every value to discard it makes those calls cost the size of the
    /// data rather than the size of the answer.
    fn scan_prefix_keys(&self, cf: &str, prefix: &[u8]) -> Result<Vec<Vec<u8>>>;

    /// How many keys sit under `prefix`.
    fn count_prefix(&self, cf: &str, prefix: &[u8]) -> Result<usize>;
}

/// Always available: it has no dependencies and no filesystem, so it is the one
/// backend every target can build, including `wasm32-unknown-unknown`.
pub mod mem_store;

#[cfg(feature = "sled")]
pub mod sled_store;

#[cfg(feature = "rocksdb")]
pub mod rocks_store;
