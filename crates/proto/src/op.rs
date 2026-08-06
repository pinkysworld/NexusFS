use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

use crate::types::{ChunkRef, DeviceId, OpId};

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
        /// Full chunk references, not bare hashes: a receiver must be able to lay
        /// out the file from the oplog before fetching any blob.
        chunks: Vec<ChunkRef>,
        new_size: u64,
        /// The file key, sealed with the repository key, when the chunks hold
        /// ciphertext. Travels with the operation so a replica records the same
        /// encryption state without a side channel.
        #[serde(default)]
        encryption: Option<Vec<u8>>,
    },
    Rename {
        old_parent: u128,
        old_name: String,
        new_parent: u128,
        new_name: String,
        /// The entries the author saw at `old_name`, identified by the operation that
        /// created each. See `Unlink::observed`.
        #[serde(default)]
        observed: Vec<OpId>,
    },
    Unlink {
        parent: u128,
        name: String,
        /// The entries this removal saw, identified by the operation that created each.
        ///
        /// Observed-remove semantics only converge if a removal names what it removed.
        /// Deriving that from local state at apply time makes the result depend on
        /// arrival order: a removal applied before the matching creation would observe
        /// nothing, and the creation would then survive on that replica and not on
        /// another. Carrying the set makes the removal mean the same thing everywhere,
        /// and lets it be recorded even before the creation it refers to arrives.
        #[serde(default)]
        observed: Vec<OpId>,
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

/// Canonical view of an `FsOp` with `sig` omitted.
///
/// Mirrors the `HelloToSign` pattern in `nexusfs-net` so both signing schemes stay
/// consistent. Everything except the signature is covered, including the proof
/// bundle, so a peer cannot strip or swap a proof without invalidating the op.
#[derive(Serialize)]
struct FsOpToSign<'a> {
    id: OpId,
    time_unix_ms: u64,
    ctx: &'a CausalCtx,
    kind: &'a FsOpKind,
    author_pubkey: [u8; 32],
    proof: &'a Option<ProofBundle>,
}

impl FsOp {
    /// Returns the OpId in (device, counter) form for indexing.
    pub fn id_tuple(&self) -> (DeviceId, u64) {
        (self.id.device_id, self.id.counter)
    }

    /// Deterministic bytes covered by `sig`.
    ///
    /// Both the signer and every verifier must derive these identically, so this is
    /// the single definition of what an operation's signature commits to.
    pub fn signing_bytes(&self) -> Result<Vec<u8>> {
        let to_sign = FsOpToSign {
            id: self.id,
            time_unix_ms: self.time_unix_ms,
            ctx: &self.ctx,
            kind: &self.kind,
            author_pubkey: self.author_pubkey,
            proof: &self.proof,
        };
        postcard::to_stdvec(&to_sign).context("encode op to sign")
    }
}
