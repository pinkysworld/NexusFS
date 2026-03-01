use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

use nexusfs_proto::{FsOp, ProofBundle, ProofMode};

/// Transparent proof bundle for immediate verifiability (non-ZK).
///
/// This is "structured evidence" that makes auditing and debugging easier.
/// It does NOT hide metadata and is NOT zero-knowledge.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct TransparentProof {
    pub old_head: Option<[u8; 32]>,
    pub new_head: Option<[u8; 32]>,
    pub changed_hashes: Vec<[u8; 32]>,
}

pub fn encode(p: &TransparentProof) -> Result<Vec<u8>> {
    postcard::to_stdvec(p).context("encode transparent proof")
}

pub fn decode(bytes: &[u8]) -> Result<TransparentProof> {
    postcard::from_bytes(bytes).context("decode transparent proof")
}

pub fn make_bundle(p: TransparentProof) -> Result<ProofBundle> {
    Ok(ProofBundle {
        mode: ProofMode::Transparent,
        bytes: encode(&p)?,
    })
}

/// Minimal verifier for transparent proofs.
pub fn verify_bundle(_op: &FsOp, bundle: &ProofBundle) -> Result<()> {
    if bundle.mode != ProofMode::Transparent {
        anyhow::bail!("expected transparent proof mode");
    }
    let _p = decode(&bundle.bytes)?;
    // TODO: validate that old/new head relationship makes sense once heads are tracked for real.
    Ok(())
}
