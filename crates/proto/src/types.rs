use serde::{Deserialize, Serialize};

use crate::msg::Msg;

/// BLAKE3 digest (32 bytes).
pub type Hash = [u8; 32];

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct DeviceId(pub u128);

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct PeerId(pub u128);

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct OpId {
    pub device_id: DeviceId,
    pub counter: u64,
}

/// A reference to a chunk stored as a blob in the CAS.
///
/// Lives in `proto` rather than `core` because `FsOpKind::Write` carries chunk
/// references over the wire: a peer must be able to reconstruct file layout from
/// the oplog alone, before it has fetched any of the referenced blobs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChunkRef {
    pub hash: Hash,
    pub len: u32,
    pub offset: u64,
}

/// A feature flag string negotiated in Hello/HelloAck.
pub type Feature = String;

/// Versioned envelope for all network messages.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MessageEnvelope {
    pub protocol_version: u16,
    pub msg_id: u64,
    pub reply_to: Option<u64>,
    pub payload: Msg,
}
