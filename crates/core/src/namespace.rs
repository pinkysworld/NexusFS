//! Persistent namespace state: directory OR-Maps and inode records.
//!
//! This is the mutable half of the filesystem. Content-addressed objects in the CAS
//! are immutable; everything that changes lives here, in CRDT form, so that two
//! replicas applying the same operations in different orders end up byte-identical.

use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};

use nexusfs_crdt::conflicts::conflict_name;
use nexusfs_crdt::lww::LwwReg;
use nexusfs_crdt::or_map::OrMap;
use nexusfs_storage::Hash;

use crate::codec::{decode, encode};
use crate::inode::ROOT_INODE;
use crate::object::{DirEntry, EntryType, FileNode, Object};
use crate::state::CoreState;

pub const CF_STATE: &str = "state";

const DIR_PREFIX: &[u8] = b"dir\0";
const INODE_PREFIX: &[u8] = b"inode\0";
/// The maintained inode map: what the state root commits to.
pub(crate) const IMAP_PREFIX: &[u8] = b"imap\0";
/// Where an inode was created, so a file's reachability can be answered without a walk.
pub(crate) const PARENT_PREFIX: &[u8] = b"parent\0";

/// Directory contents: name -> entry, as an observed-remove map.
///
/// A map rather than a plain list because concurrent adds of the same name must both
/// survive until a reader resolves them deterministically (see [`materialize_dir`]).
pub type DirMap = OrMap<String, DirEntryValue>;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct DirEntryValue {
    pub inode_id: u128,
    pub entry_type: EntryType,
    /// Timestamp of the operation that created this link. Carried here so conflict
    /// naming needs no oplog lookup and stays identical on every replica.
    pub created_unix_ms: u64,
}

/// Content of an inode, moved as one unit.
///
/// Hash, size and mtime change together on every write, so they share a single
/// register — otherwise a losing write could leave its size attached to a winning
/// write's content.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ContentState {
    /// CAS hash of the current `FileNode`.
    ///
    /// Only meaningful for files. Directory content is derived from the OR-Map, and
    /// its `DirNode` hash is computed during snapshot building.
    pub node_hash: Option<Hash>,
    pub size: u64,
    pub mtime_unix_ms: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct AttrState {
    pub mode: u32,
    pub uid: u32,
    pub gid: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InodeRecord {
    pub kind: EntryType,
    pub content: LwwReg<ContentState>,
    pub attrs: LwwReg<AttrState>,
    pub ctime_unix_ms: u64,
}

impl InodeRecord {
    pub fn new(kind: EntryType, mode: u32, now_ms: u64, writer_id: u128, seq: u64) -> Self {
        Self {
            kind,
            content: LwwReg::new(
                ContentState {
                    node_hash: None,
                    size: 0,
                    mtime_unix_ms: now_ms,
                },
                now_ms,
                writer_id,
                seq,
            ),
            attrs: LwwReg::new(
                AttrState {
                    mode,
                    uid: 0,
                    gid: 0,
                },
                now_ms,
                writer_id,
                seq,
            ),
            ctime_unix_ms: now_ms,
        }
    }
}

fn prefixed_key(prefix: &[u8], inode: u128) -> Vec<u8> {
    let mut k = Vec::with_capacity(prefix.len() + 16);
    k.extend_from_slice(prefix);
    k.extend_from_slice(&inode.to_be_bytes());
    k
}

pub fn dir_key(inode: u128) -> Vec<u8> {
    prefixed_key(DIR_PREFIX, inode)
}

pub(crate) fn imap_key(inode: u128) -> Vec<u8> {
    let mut k = IMAP_PREFIX.to_vec();
    k.extend_from_slice(&inode.to_be_bytes());
    k
}

/// Inodes are fixed-width big-endian, so key order is inode order — which is what lets
/// the map be read back already sorted, as the commitment requires.
pub(crate) fn parse_imap_key(key: &[u8]) -> Option<u128> {
    if !key.starts_with(IMAP_PREFIX) || key.len() != IMAP_PREFIX.len() + 16 {
        return None;
    }
    let mut b = [0u8; 16];
    b.copy_from_slice(&key[IMAP_PREFIX.len()..]);
    Some(u128::from_be_bytes(b))
}

pub(crate) fn parent_key(inode: u128) -> Vec<u8> {
    let mut k = PARENT_PREFIX.to_vec();
    k.extend_from_slice(&inode.to_be_bytes());
    k
}

pub fn inode_key(inode: u128) -> Vec<u8> {
    prefixed_key(INODE_PREFIX, inode)
}

/// Reject names that would make path resolution ambiguous or escape the tree.
pub fn validate_name(name: &str) -> Result<()> {
    if name.is_empty() {
        bail!("entry name must not be empty");
    }
    if name.contains('/') {
        bail!("entry name must not contain a path separator: {name:?}");
    }
    if name == "." || name == ".." {
        bail!("entry name must not be {name:?}");
    }
    Ok(())
}

impl CoreState {
    // ---- raw state accessors -------------------------------------------------

    pub fn load_dir(&self, inode: u128) -> Result<Option<DirMap>> {
        let Some(bytes) = self.stores.kv.get_kv(CF_STATE, &dir_key(inode))? else {
            return Ok(None);
        };
        Ok(Some(decode(&bytes).context("decode dir map")?))
    }

    pub fn store_dir(&self, inode: u128, map: &DirMap) -> Result<()> {
        self.stores
            .kv
            .put_kv(CF_STATE, &dir_key(inode), &encode(map)?)?;
        Ok(())
    }

    pub fn load_inode(&self, inode: u128) -> Result<Option<InodeRecord>> {
        let Some(bytes) = self.stores.kv.get_kv(CF_STATE, &inode_key(inode))? else {
            return Ok(None);
        };
        Ok(Some(decode(&bytes).context("decode inode record")?))
    }

    /// Record where an inode lives, for reachability questions the map cannot answer.
    pub(crate) fn set_parent(&self, inode: u128, parent: u128) -> Result<()> {
        self.stores
            .kv
            .put_kv(CF_STATE, &parent_key(inode), &parent.to_be_bytes())
    }

    pub(crate) fn parent_of(&self, inode: u128) -> Result<Option<u128>> {
        let Some(raw) = self.stores.kv.get_kv(CF_STATE, &parent_key(inode))? else {
            return Ok(None);
        };
        let Ok(bytes) = <[u8; 16]>::try_from(&raw[..]) else {
            return Ok(None);
        };
        Ok(Some(u128::from_be_bytes(bytes)))
    }

    pub fn store_inode(&self, inode: u128, rec: &InodeRecord) -> Result<()> {
        self.stores
            .kv
            .put_kv(CF_STATE, &inode_key(inode), &encode(rec)?)?;
        Ok(())
    }

    pub fn inode_exists(&self, inode: u128) -> Result<bool> {
        Ok(self
            .stores
            .kv
            .get_kv(CF_STATE, &inode_key(inode))?
            .is_some())
    }

    pub fn is_dir(&self, inode: u128) -> Result<bool> {
        Ok(self
            .load_inode(inode)?
            .map(|r| r.kind == EntryType::Dir)
            .unwrap_or(false))
    }

    // ---- conflict-aware views -------------------------------------------------

    /// Resolve a directory's OR-Map into the entries a user actually sees.
    ///
    /// When several replicas concurrently bound the same name, every add survives in
    /// the map. Exactly one of them keeps the plain name — the one with the lowest
    /// dot, which every replica agrees on — and the rest are renamed via
    /// [`conflict_name`]. No data is hidden and no replica sees a different listing.
    pub fn materialize_dir(&self, inode: u128) -> Result<Vec<DirEntry>> {
        let Some(map) = self.load_dir(inode)? else {
            return Ok(Vec::new());
        };

        let mut entries = Vec::new();
        for name in map.live_keys() {
            let survivors = map.get_all(&name);
            for (idx, (dot, value)) in survivors.into_iter().enumerate() {
                let entry_name = if idx == 0 {
                    name.clone()
                } else {
                    conflict_name(&name, dot.device_id, value.created_unix_ms)
                };
                entries.push(DirEntry {
                    name: entry_name,
                    inode_id: value.inode_id,
                    entry_type: value.entry_type,
                });
            }
        }

        entries.sort_by(|a, b| {
            a.name
                .cmp(&b.name)
                .then_with(|| a.entry_type.cmp(&b.entry_type))
                .then_with(|| a.inode_id.cmp(&b.inode_id))
        });
        Ok(entries)
    }

    /// Look up one name in a directory, using the same view `materialize_dir` shows,
    /// so `ls` and `cat` can never disagree about what a path means.
    pub fn lookup(&self, parent: u128, name: &str) -> Result<Option<DirEntry>> {
        Ok(self
            .materialize_dir(parent)?
            .into_iter()
            .find(|e| e.name == name))
    }

    // ---- read path -------------------------------------------------------------

    /// Split a path into components, rejecting traversal.
    pub fn split_path(path: &str) -> Result<Vec<String>> {
        let mut out = Vec::new();
        for part in path.split('/') {
            if part.is_empty() || part == "." {
                continue;
            }
            if part == ".." {
                bail!("path traversal with '..' is not supported: {path:?}");
            }
            out.push(part.to_string());
        }
        Ok(out)
    }

    /// Resolve an absolute path to `(inode, kind)`, or `None` if any component is
    /// missing.
    pub fn resolve_path(&self, path: &str) -> Result<Option<(u128, EntryType)>> {
        let parts = Self::split_path(path)?;
        let mut current = ROOT_INODE;
        let mut kind = EntryType::Dir;

        for (idx, part) in parts.iter().enumerate() {
            if kind != EntryType::Dir {
                bail!("{:?} is not a directory", parts[..idx].join("/"));
            }
            let Some(entry) = self.lookup(current, part)? else {
                return Ok(None);
            };
            current = entry.inode_id;
            kind = entry.entry_type;
        }

        Ok(Some((current, kind)))
    }

    /// Resolve the parent directory of a path plus the final component.
    pub fn resolve_parent(&self, path: &str) -> Result<Option<(u128, String)>> {
        let parts = Self::split_path(path)?;
        let Some((name, dirs)) = parts.split_last() else {
            bail!("path {path:?} has no final component");
        };
        let parent_path = dirs.join("/");
        let Some((parent, kind)) = self.resolve_path(&parent_path)? else {
            return Ok(None);
        };
        if kind != EntryType::Dir {
            bail!("{parent_path:?} is not a directory");
        }
        Ok(Some((parent, name.clone())))
    }

    pub fn read_dir_path(&self, path: &str) -> Result<Vec<DirEntry>> {
        let Some((inode, kind)) = self.resolve_path(path)? else {
            bail!("no such directory: {path}");
        };
        if kind != EntryType::Dir {
            bail!("not a directory: {path}");
        }
        self.materialize_dir(inode)
    }

    /// Reassemble a file's bytes from its chunk references.
    pub fn read_file(&self, inode: u128) -> Result<Vec<u8>> {
        let Some(rec) = self.load_inode(inode)? else {
            bail!("no such inode: {inode:x}");
        };
        if rec.kind != EntryType::File {
            bail!("inode {inode:x} is not a file");
        }
        let Some(node_hash) = rec.content.value.node_hash else {
            // A file that was created but never written is legitimately empty.
            return Ok(Vec::new());
        };

        let file = match self.get_object(&node_hash)? {
            Some(Object::FileNode(f)) => f,
            Some(_) => bail!("inode {inode:x} points at a non-file object"),
            None => bail!(
                "missing file object {} for inode {inode:x}",
                hex(&node_hash)
            ),
        };

        self.materialize_file(&file)
    }

    /// Fetch and concatenate a `FileNode`'s chunks, verifying layout as it goes.
    pub fn materialize_file(&self, file: &FileNode) -> Result<Vec<u8>> {
        // Whether a file is encrypted is recorded on the file, not on the node, so a
        // node reads its own older plaintext files unchanged after enabling encryption.
        let file_key = match &file.encryption {
            Some(info) => {
                let cipher = self.cipher.as_ref().ok_or_else(|| {
                    anyhow::anyhow!(
                        "file is encrypted but this node has no repository key configured"
                    )
                })?;
                Some(cipher.open_file_key(&info.sealed_key)?)
            }
            None => None,
        };

        let mut out = Vec::with_capacity(file.size as usize);
        for (index, chunk) in file.chunks.iter().enumerate() {
            let Some(stored) = self.stores.blobs.get(&chunk.hash)? else {
                bail!(
                    "missing chunk {} (offset {})",
                    hex(&chunk.hash),
                    chunk.offset
                );
            };
            // `len` is the stored length, which for ciphertext includes the AEAD tag.
            if stored.len() != chunk.len as usize {
                bail!(
                    "chunk {} length mismatch: stored {}, expected {}",
                    hex(&chunk.hash),
                    stored.len(),
                    chunk.len
                );
            }
            // `offset` is into the plaintext, so it tracks the assembled output.
            if chunk.offset as usize != out.len() {
                bail!(
                    "chunk {} offset mismatch: expected {}, got {}",
                    hex(&chunk.hash),
                    out.len(),
                    chunk.offset
                );
            }

            match &file_key {
                // Authentication failure means altered ciphertext or the wrong
                // repository key. Either way the read must fail rather than return
                // whatever the bytes happen to decode to.
                Some(key) => out.extend_from_slice(&nexusfs_crypto::RepoCipher::open_chunk(
                    key,
                    index as u64,
                    &stored,
                )?),
                None => out.extend_from_slice(&stored),
            }
        }

        if out.len() as u64 != file.size {
            bail!(
                "file size mismatch: reassembled {}, header {}",
                out.len(),
                file.size
            );
        }
        Ok(out)
    }

    pub fn read_file_path(&self, path: &str) -> Result<Vec<u8>> {
        let Some((inode, kind)) = self.resolve_path(path)? else {
            bail!("no such file: {path}");
        };
        if kind != EntryType::File {
            bail!("not a file: {path}");
        }
        self.read_file(inode)
    }
}

fn hex(h: &Hash) -> String {
    h.iter().map(|b| format!("{b:02x}")).collect()
}
