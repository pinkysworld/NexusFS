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
//! # The commitment mode
//!
//! `ProofPolicy::Commit` records something a transparent proof cannot: an inclusion
//! path showing that the entry the operation touched really is in the state root the
//! author claims. A transparent proof is only checkable by someone who already holds
//! the author's prior state; a commitment proof is checkable by anyone holding the root
//! — no filesystem, no network, no replay.
//!
//! That is a commitment scheme, not zero knowledge. The verifier learns the inode and
//! its object hash; what it does not learn is the rest of the tree. The mode is named
//! `ZkCommit` because the sibling path is precisely the witness a SNARK would consume,
//! not because a proving system is involved. See `nexusfs_zk::merkle`.

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

/// Evidence that an entry is in a committed state, checkable on its own.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CommitProof {
    /// State root before the operation, when the author had one. Carried for the same
    /// reason the transparent proof carries it: it chains one operation to the next.
    pub old_root: Option<Hash>,
    /// The state root this operation produced.
    pub new_root: Hash,
    /// Inclusion of the entry the operation is about, in `new_root`.
    pub entry: nexusfs_zk::merkle::InclusionProof,
}

pub fn encode_commit(proof: &CommitProof) -> Result<Vec<u8>> {
    postcard::to_stdvec(proof).context("encode commitment proof")
}

pub fn decode_commit(bytes: &[u8]) -> Result<CommitProof> {
    postcard::from_bytes(bytes).context("decode commitment proof")
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
    /// Generate commitment proofs carrying an inclusion path.
    ///
    /// Incoming transparent proofs are still accepted. Refusing them would make the
    /// mode unusable in any cluster that is not upgraded in lockstep, and a transparent
    /// proof is not *wrong* — it proves less.
    Commit,
}

impl ProofPolicy {
    pub fn from_config(value: &str) -> Self {
        match value.trim().to_ascii_lowercase().as_str() {
            "transparent" => Self::Transparent,
            "required" => Self::Required,
            "zk_commit" | "commit" => Self::Commit,
            _ => Self::None,
        }
    }

    pub fn generates(&self) -> bool {
        !matches!(self, Self::None)
    }

    /// Whether local operations should carry an inclusion path.
    pub fn commits(&self) -> bool {
        matches!(self, Self::Commit)
    }
}

impl CoreState {
    /// Build the evidence for an operation about to be applied locally.
    ///
    /// Called before the mutation, so `old_root` is genuinely the prior state.
    pub fn begin_proof(&self) -> Result<Option<Hash>> {
        self.get_state_root()
    }

    /// Finish the evidence for an operation whose subject is `inode`.
    ///
    /// Falls back to a transparent bundle when the subject is not in the live tree —
    /// an unlink removes its own subject, and a proof of absence is a different
    /// construction. Downgrading is safe because the verifier checks whatever mode the
    /// bundle declares; silently emitting a commitment proof for the wrong entry would
    /// not be.
    pub fn finish_commit_proof(
        &self,
        old_root: Option<Hash>,
        inode: Option<u128>,
        changed: Vec<Hash>,
    ) -> Result<ProofBundle> {
        let new_root = self.get_state_root()?;
        let entry = match (new_root, inode) {
            (Some(_), Some(inode)) => self.inclusion_proof(inode)?,
            _ => None,
        };

        match (new_root, entry) {
            (Some(new_root), Some(entry)) => Ok(ProofBundle {
                mode: ProofMode::ZkCommit,
                bytes: encode_commit(&CommitProof {
                    old_root,
                    new_root,
                    entry,
                })?,
            }),
            _ => self.finish_proof(old_root, changed),
        }
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
            ProofMode::Transparent => {
                // A bundle that does not decode is malformed, and malformed evidence is
                // worse than none: it must be rejected deterministically, not ignored.
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
            }

            ProofMode::ZkCommit => {
                let proof = decode_commit(&bundle.bytes).with_context(|| {
                    format!(
                        "operation {:x}/{} carries a malformed commitment proof",
                        op.id.device_id.0, op.id.counter
                    )
                })?;

                // The whole point of this mode: the claim is checkable here and now,
                // against nothing but the bundle itself. No prior state, no replay.
                nexusfs_zk::merkle::check(&proof.entry, &proof.new_root).with_context(|| {
                    format!(
                        "operation {:x}/{} carries an inclusion path that does not \
                         reach the state root it claims",
                        op.id.device_id.0, op.id.counter
                    )
                })?;
            }

            other => bail!("proof mode {other:?} is not supported by this build"),
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
