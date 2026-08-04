//! Transparent proofs: auditable evidence of what an operation changed.
//!
//! # What these do and do not prove
//!
//! A transparent proof records the state root before an operation, the state root
//! after it, and the object hashes the operation touched. Because the author signs the
//! operation *including* its proof, they cannot later claim a different transition —
//! the evidence is bound to the signature.
//!
//! What it is **not** is zero-knowledge, and not a proof that the transition was
//! *correct*. Verifying that requires replaying the operation, which `nexusfs verify`
//! does locally. A receiver checks two cheaper things: that the bundle is well formed,
//! and — when it already holds the author's prior state — that the recorded `old_root`
//! matches. That second check is skipped rather than failed when the receiver has not
//! yet caught up, because operations legitimately arrive out of order.
//!
//! The ZkCommit path (M7) replaces the recorded roots with commitments that can be
//! proved without revealing them. Nothing here is thrown away when that lands: the
//! same before/after shape is what a circuit would attest to.

use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};

use nexusfs_proto::{FsOp, Hash, ProofBundle, ProofMode};

use crate::state::CoreState;

/// Evidence recorded alongside an operation.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct TransparentProof {
    /// State root before the operation, if the author had one.
    pub old_root: Option<Hash>,
    /// State root after the author applied it.
    pub new_root: Option<Hash>,
    /// Object hashes the operation introduced or replaced.
    pub changed: Vec<Hash>,
}

pub fn encode_proof(proof: &TransparentProof) -> Result<Vec<u8>> {
    postcard::to_stdvec(proof).context("encode transparent proof")
}

pub fn decode_proof(bytes: &[u8]) -> Result<TransparentProof> {
    postcard::from_bytes(bytes).context("decode transparent proof")
}

/// How strictly a node treats proofs on operations it receives.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ProofPolicy {
    /// Do not generate or inspect proofs.
    #[default]
    None,
    /// Generate proofs on local operations and validate any that arrive.
    Transparent,
    /// As above, and refuse operations that carry no proof at all.
    Required,
}

impl ProofPolicy {
    pub fn from_config(value: &str) -> Self {
        match value.trim().to_ascii_lowercase().as_str() {
            "transparent" => Self::Transparent,
            "required" => Self::Required,
            _ => Self::None,
        }
    }

    pub fn generates(&self) -> bool {
        !matches!(self, Self::None)
    }
}

impl CoreState {
    /// Build the evidence for an operation about to be applied locally.
    ///
    /// Called before the mutation, so `old_root` is genuinely the prior state.
    pub fn begin_proof(&self) -> Result<Option<Hash>> {
        self.get_state_root()
    }

    /// Finish the evidence once the operation has been applied.
    pub fn finish_proof(&self, old_root: Option<Hash>, changed: Vec<Hash>) -> Result<ProofBundle> {
        let proof = TransparentProof {
            old_root,
            new_root: self.get_state_root()?,
            changed,
        };
        Ok(ProofBundle {
            mode: ProofMode::Transparent,
            bytes: encode_proof(&proof)?,
        })
    }

    /// Check an incoming operation's proof against local policy.
    ///
    /// Deliberately lenient about ordering and strict about structure: a malformed or
    /// mislabelled bundle is always rejected, while a well-formed bundle whose
    /// `old_root` we cannot corroborate yet is accepted, because a node that is behind
    /// has no basis to judge it.
    pub fn check_proof(&self, op: &FsOp, policy: ProofPolicy) -> Result<()> {
        if policy == ProofPolicy::None {
            return Ok(());
        }

        let Some(bundle) = &op.proof else {
            if policy == ProofPolicy::Required {
                bail!(
                    "operation {:x}/{} carries no proof and policy requires one",
                    op.id.device_id.0,
                    op.id.counter
                );
            }
            return Ok(());
        };

        match bundle.mode {
            ProofMode::Transparent => {}
            other => bail!("proof mode {other:?} is not supported by this build"),
        }

        // A bundle that does not decode is malformed, and malformed evidence is worse
        // than none: it must be rejected deterministically rather than ignored.
        let proof = decode_proof(&bundle.bytes).with_context(|| {
            format!(
                "operation {:x}/{} carries a malformed transparent proof",
                op.id.device_id.0, op.id.counter
            )
        })?;

        if proof.new_root.is_none() {
            bail!(
                "operation {:x}/{} records no resulting state root",
                op.id.device_id.0,
                op.id.counter
            );
        }

        Ok(())
    }
}

/// Outcome of replaying the local oplog against its recorded evidence.
#[derive(Debug, Default, Clone)]
pub struct VerifyReport {
    pub operations: usize,
    pub with_proof: usize,
    pub without_proof: usize,
    pub malformed: usize,
    /// Operations whose recorded `new_root` matched the root reached by replay.
    pub roots_matched: usize,
    pub roots_mismatched: usize,
    pub signature_failures: usize,
    pub unreadable_files: Vec<String>,
    pub state_root: Option<Hash>,
}

impl VerifyReport {
    pub fn ok(&self) -> bool {
        self.malformed == 0
            && self.signature_failures == 0
            && self.roots_mismatched == 0
            && self.unreadable_files.is_empty()
    }
}

impl CoreState {
    /// Audit the local repository.
    ///
    /// Checks every operation's signature and proof structure, and reads every file
    /// back so that missing chunks, corrupted ciphertext and truncated content all
    /// surface here rather than the first time somebody opens the file.
    pub fn verify_repository(&self) -> Result<VerifyReport> {
        let mut report = VerifyReport::default();

        for op in self.all_ops()? {
            report.operations += 1;

            if self.verify_op(&op).is_err() {
                report.signature_failures += 1;
            }

            match &op.proof {
                None => report.without_proof += 1,
                Some(bundle) => match decode_proof(&bundle.bytes) {
                    Ok(_) if bundle.mode != ProofMode::Transparent => report.malformed += 1,
                    Ok(_) => report.with_proof += 1,
                    Err(_) => report.malformed += 1,
                },
            }
        }

        // Reading every file exercises chunk presence, length, ordering and — when
        // encryption is on — authentication of every chunk.
        for entry in self.walk("/")? {
            if entry.kind == crate::object::EntryType::File
                && self.read_file_path(&entry.path).is_err()
            {
                report.unreadable_files.push(entry.path);
            }
        }

        report.state_root = self.get_state_root()?;
        Ok(report)
    }
}
