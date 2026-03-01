use serde::{Deserialize, Serialize};

use crate::types::{DeviceId, Hash, OpId};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ProofMode {
    None,
    Transparent,
    ZkCommit,
    ZkFull,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProofBundle {
    pub mode: ProofMode,
    /// Mode-specific bytes (e.g., transparent evidence or SNARK proof bytes).
    pub bytes: Vec<u8>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CausalCtx {
    /// A minimal dependency set. Later can be replaced by vector clocks or dotted version vectors.
    pub deps: Vec<OpId>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum FsOpKind {
    CreateFile {
        parent: u128,
        name: String,
        mode: u32,
    },
    Mkdir {
        parent: u128,
        name: String,
        mode: u32,
    },
    Write {
        inode: u128,
        offset: u64,
        data_hashes: Vec<Hash>,
        new_size: u64,
    },
    Rename {
        old_parent: u128,
        old_name: String,
        new_parent: u128,
        new_name: String,
    },
    Unlink {
        parent: u128,
        name: String,
    },
    SetAttr {
        inode: u128,
        mode: Option<u32>,
        uid: Option<u32>,
        gid: Option<u32>,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FsOp {
    pub id: OpId,
    pub time_unix_ms: u64,
    pub ctx: CausalCtx,
    pub kind: FsOpKind,
    pub author_pubkey: [u8; 32],
    pub sig: Vec<u8>,
    pub proof: Option<ProofBundle>,
}

impl FsOp {
    /// Returns the OpId in (device, counter) form for indexing.
    pub fn id_tuple(&self) -> (DeviceId, u64) {
        (self.id.device_id, self.id.counter)
    }
}
