use serde::{Deserialize, Serialize};

use crate::op::FsOp;
use crate::types::{DeviceId, Feature, Hash};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BuildInfo {
    pub version: String,
    pub git_hash: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SignedHead {
    pub head_hash: Hash,
    pub device_id: DeviceId,
    pub time_unix_ms: u64,
    pub sig: Vec<u8>,
}

/// What a node already holds, per device.
///
/// A contiguous watermark alone is not enough. A node that refuses one operation — a
/// bad signature is permanent, not transient — would pin its watermark below that
/// counter forever, and a peer answering only from the watermark would resend the same
/// window every round while everything above it stayed unreachable.
///
/// So the summary also carries the counters held *above* the watermark. The gap stays
/// visible, which is what allows the refused operation to be offered again if it is ever
/// replaced, while everything past it still transfers.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ClockSummary {
    /// Highest contiguous counter held for each device.
    pub entries: Vec<(DeviceId, u64)>,
    /// Counters held above that watermark, sorted, per device.
    ///
    /// Bounded by the sender: an unbounded list would let a fragmented history grow the
    /// frame without limit. Truncation is safe — it only costs a resend.
    #[serde(default)]
    pub above: Vec<(DeviceId, Vec<u64>)>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OpRange {
    pub device_id: DeviceId,
    pub start: u64,
    pub end: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TelemetryMsg {
    pub battery_pct: Option<u8>,
    pub charging: bool,
    pub temp_c: Option<i16>,
    pub cpu_load: f32,
    pub link_cost: f32,
    pub storage_free_bytes: u64,
    pub time_unix_ms: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum Msg {
    Hello {
        device_id: DeviceId,
        pubkey_ed25519: [u8; 32],
        features: Vec<Feature>,
        build: BuildInfo,
        time_unix_ms: u64,
        nonce: [u8; 32],
        sig: Vec<u8>,
    },
    /// Response to Hello.
    ///
    /// Carries the responder's own identity, because the initiator has to decide
    /// whether to trust whoever answered — and echoes the initiator's nonce under the
    /// responder's signature, so the reply cannot be replayed from an earlier session
    /// or forged by a party that does not hold the key.
    HelloAck {
        accepted: bool,
        reason: Option<String>,
        features: Vec<Feature>,
        peer_device: DeviceId,
        peer_pubkey: [u8; 32],
        /// The nonce from the Hello being answered.
        nonce: [u8; 32],
        sig: Vec<u8>,
    },

    Have {
        summary: ClockSummary,
        head: Option<SignedHead>,
    },
    WantOps {
        ranges: Vec<OpRange>,
        limit_ops: u32,
    },
    OpsBatch {
        ops: Vec<FsOp>,
        more: bool,
    },

    WantBlobs {
        hashes: Vec<Hash>,
        max_bytes: u64,
    },
    BlobsBatch {
        blobs: Vec<(Hash, Vec<u8>)>,
        more: bool,
    },

    SnapshotAnnounce {
        signed_head: SignedHead,
    },
    Telemetry {
        telemetry: TelemetryMsg,
    },

    Ping,
    Pong,

    /// Initiator has everything it asked for; the responder may close.
    Bye,

    Error {
        code: u16,
        message: String,
        retry_after_ms: Option<u64>,
    },
}
