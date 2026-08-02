#![forbid(unsafe_code)]

use anyhow::{Context, Result};
use sled::Db;

use crate::{BlobStore, Hash, KvStore};

#[derive(Clone)]
pub struct SledStore {
    db: Db,
}

impl SledStore {
    pub fn open(path: impl AsRef<std::path::Path>) -> Result<Self> {
        let db = sled::open(path).context("open sled db")?;
        Ok(Self { db })
    }

    fn blobs_tree(&self) -> Result<sled::Tree> {
        self.db.open_tree("blobs").context("open blobs tree")
    }

    fn kv_tree(&self, cf: &str) -> Result<sled::Tree> {
        self.db
            .open_tree(format!("kv/{}", cf))
            .with_context(|| format!("open kv tree for cf={}", cf))
    }
}

impl BlobStore for SledStore {
    fn put(&self, key: Hash, data: &[u8]) -> Result<()> {
        let t = self.blobs_tree()?;
        t.insert(key, data)?;
        t.flush()?;
        Ok(())
    }

    fn get(&self, key: &Hash) -> Result<Option<Vec<u8>>> {
        let t = self.blobs_tree()?;
        Ok(t.get(key)?.map(|ivec| ivec.to_vec()))
    }

    fn has(&self, key: &Hash) -> Result<bool> {
        let t = self.blobs_tree()?;
        Ok(t.contains_key(key)?)
    }

    fn delete(&self, key: &Hash) -> Result<()> {
        let t = self.blobs_tree()?;
        t.remove(key)?;
        t.flush()?;
        Ok(())
    }

    fn stats(&self) -> Result<(usize, u64)> {
        let t = self.blobs_tree()?;
        let mut count = 0usize;
        let mut bytes = 0u64;
        for kv in t.iter() {
            let (_k, v) = kv?;
            count += 1;
            bytes += v.len() as u64;
        }
        Ok((count, bytes))
    }
}

impl KvStore for SledStore {
    fn put_kv(&self, cf: &str, key: &[u8], val: &[u8]) -> Result<()> {
        let t = self.kv_tree(cf)?;
        t.insert(key, val)?;
        t.flush()?;
        Ok(())
    }

    fn get_kv(&self, cf: &str, key: &[u8]) -> Result<Option<Vec<u8>>> {
        let t = self.kv_tree(cf)?;
        Ok(t.get(key)?.map(|ivec| ivec.to_vec()))
    }

    fn delete_kv(&self, cf: &str, key: &[u8]) -> Result<()> {
        let t = self.kv_tree(cf)?;
        t.remove(key)?;
        t.flush()?;
        Ok(())
    }

    fn scan_prefix(&self, cf: &str, prefix: &[u8]) -> Result<Vec<(Vec<u8>, Vec<u8>)>> {
        let t = self.kv_tree(cf)?;
        let mut out = Vec::new();
        for kv in t.scan_prefix(prefix) {
            let (k, v) = kv?;
            out.push((k.to_vec(), v.to_vec()));
        }
        Ok(out)
    }
}
