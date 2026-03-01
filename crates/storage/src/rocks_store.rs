#![forbid(unsafe_code)]

/// RocksDB backend stub.
///
/// Note: rocksdb adds native build complexity. Keep it behind the `rocksdb` feature.
#[allow(dead_code)]
pub struct RocksStore;

#[cfg(feature = "rocksdb")]
impl RocksStore {
    pub fn open(_path: impl AsRef<std::path::Path>) -> anyhow::Result<Self> {
        anyhow::bail!("RocksDB backend is not implemented in the skeleton; use `sled` for now.");
    }
}
