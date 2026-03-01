#![forbid(unsafe_code)]

pub mod transparent;

use anyhow::Result;
use nexusfs_proto::{FsOp, ProofBundle, ProofMode};

/// Proof modes supported by this build.
#[derive(Debug, Clone, Copy)]
pub enum LocalProofMode {
    None,
    Transparent,
    ZkCommit,
    ZkFull,
}

impl From<LocalProofMode> for ProofMode {
    fn from(m: LocalProofMode) -> ProofMode {
        match m {
            LocalProofMode::None => ProofMode::None,
            LocalProofMode::Transparent => ProofMode::Transparent,
            LocalProofMode::ZkCommit => ProofMode::ZkCommit,
            LocalProofMode::ZkFull => ProofMode::ZkFull,
        }
    }
}

/// Minimal view of state needed for proof generation.
/// Later: include Poseidon roots, policy commitments, etc.
#[derive(Debug, Clone, Default)]
pub struct ProofStateView {
    pub head: Option<[u8; 32]>,
}

/// Prover interface (pluggable).
pub trait Prover: Send + Sync {
    fn prove(&self, op: &FsOp, st: &ProofStateView) -> Result<ProofBundle>;
}

/// Verifier interface (pluggable).
pub trait Verifier: Send + Sync {
    fn verify(&self, op: &FsOp, proof: &ProofBundle, st: &ProofStateView) -> Result<()>;
}
