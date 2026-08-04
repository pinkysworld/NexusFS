use std::cmp::Ordering;

use serde::{Deserialize, Serialize};

use nexusfs_proto::types::DeviceId;
use nexusfs_storage::Hash;

/// Chunk references are defined in `proto` because operations carry them over the
/// wire; re-exported here so object-model code has one obvious import path.
pub use nexusfs_proto::types::ChunkRef;

/// Versioned object header to allow safe upgrades.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ObjectHeader {
    pub type_tag: u16,
    pub version: u16,
}

/// Key material for an encrypted file, stored with the file itself.
///
/// Travelling inside the `FileNode` means the key replicates with the content and
/// needs no side channel — a node holding the repository key can read a file the
/// moment it arrives.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileEncryption {
    /// The file key, sealed with the repository key.
    pub sealed_key: Vec<u8>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileNode {
    pub header: ObjectHeader,
    pub size: u64,
    pub chunks: Vec<ChunkRef>,

    // POSIX-ish metadata (subset)
    pub mode: u32,
    pub uid: u32,
    pub gid: u32,
    pub mtime_unix_ms: u64,
    pub ctime_unix_ms: u64,

    /// Present when the chunks hold ciphertext.
    ///
    /// `ChunkRef::hash` always names the bytes as stored, so a peer can verify a
    /// transfer whether or not it can decrypt. `len` is likewise the stored length,
    /// which for an encrypted chunk includes the AEAD tag; `size` above is the
    /// plaintext length.
    #[serde(default)]
    pub encryption: Option<FileEncryption>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub enum EntryType {
    File,
    Dir,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DirEntry {
    pub name: String,
    pub inode_id: u128,
    pub entry_type: EntryType,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DirNode {
    pub header: ObjectHeader,
    /// Entries MUST be sorted by name for canonical hashing.
    pub entries: Vec<DirEntry>,

    pub mode: u32,
    pub uid: u32,
    pub gid: u32,
    pub mtime_unix_ms: u64,
    pub ctime_unix_ms: u64,
}

impl DirNode {
    pub fn new_canonical(
        mut entries: Vec<DirEntry>,
        mode: u32,
        uid: u32,
        gid: u32,
        now_ms: u64,
    ) -> Self {
        sort_dir_entries(&mut entries);
        Self {
            header: ObjectHeader {
                type_tag: 2,
                version: 1,
            },
            entries,
            mode,
            uid,
            gid,
            mtime_unix_ms: now_ms,
            ctime_unix_ms: now_ms,
        }
    }

    pub fn canonicalize(&mut self) {
        sort_dir_entries(&mut self.entries);
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SnapshotRoot {
    pub header: ObjectHeader,
    pub root_dir_inode: u128,
    /// Future: SNARK-friendly Merkle map root for inode -> current hash.
    pub inode_map_root: Option<Hash>,
    pub timestamp_unix_ms: u64,
    pub author: DeviceId,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum Object {
    FileNode(FileNode),
    DirNode(DirNode),
    SnapshotRoot(SnapshotRoot),
}

impl Object {
    pub fn canonicalized(&self) -> Self {
        match self {
            Self::DirNode(dir) => {
                let mut dir = dir.clone();
                dir.canonicalize();
                Self::DirNode(dir)
            }
            _ => self.clone(),
        }
    }
}

fn sort_dir_entries(entries: &mut [DirEntry]) {
    entries.sort_by(compare_dir_entries);
}

fn compare_dir_entries(left: &DirEntry, right: &DirEntry) -> Ordering {
    left.name
        .cmp(&right.name)
        .then_with(|| left.entry_type.cmp(&right.entry_type))
        .then_with(|| left.inode_id.cmp(&right.inode_id))
}
