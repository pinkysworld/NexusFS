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

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClockSummary {
    /// Highest contiguous counter applied for each device.
    pub entries: Vec<(DeviceId, u64)>,
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
    HelloAck {
        accepted: bool,
        reason: Option<String>,
        features: Vec<Feature>,
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

    Error {
        code: u16,
        message: String,
        retry_after_ms: Option<u64>,
    },
}
