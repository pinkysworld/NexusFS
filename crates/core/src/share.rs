//! Re-sealing existing files to the current recipient set.
//!
//! Enrolling a peer makes it a recipient of everything written *after* that point.
//! Files already on disk carry envelopes for whoever was enrolled when they were
//! written, so a new peer replicates them, verifies them, and cannot read a byte. That
//! is correct — nothing should silently gain access — but it is rarely what an operator
//! wanted, and this is how they say so.
//!
//! # What this does not do
//!
//! **It does not revoke.** Re-sealing adds envelopes; it cannot take back what someone
//! already has. A device that once held an envelope for a file, or the repository key it
//! was sealed with, can still decrypt the ciphertext it kept — and the ciphertext has
//! not changed. Genuinely withdrawing access means re-encrypting the content under a
//! fresh file key, which rewrites every chunk and every hash that names it. That is key
//! rotation, and it is deliberately a separate job.
//!
//! Saying so matters more than it might seem: an operator who runs this after removing a
//! peer, believing it revokes, has done nothing at all. The command says it out loud
//! rather than leaving it in a manual.
//!
//! # How it works
//!
//! A file's encryption record lives inside its `FileNode`, and a `FileNode` is only
//! reachable through the operation log — so re-sealing is a `Write` like any other: the
//! same chunks, the same size, a new set of envelopes. It is signed, it replicates, and
//! it converges. Nothing here reaches around the state machine.
//!
//! Files this node cannot itself read are skipped rather than failing the run. A node
//! that is not a recipient cannot recover the file key, so it has nothing to re-seal
//! with; reporting that as a count is more useful than stopping halfway through.
//!
//! # Rotation, which is the other half
//!
//! [`CoreState::rotate_content`] is what re-sealing is not: it mints a *fresh* file key,
//! re-encrypts the content under it, and seals that to the current recipients. Every
//! chunk changes, so every hash naming it changes, and the old ciphertext becomes
//! garbage this node will collect.
//!
//! That is what makes removing a peer mean something. It is still not time travel: a
//! device that copied the old ciphertext and held an envelope for it can read that
//! version for as long as it keeps the bytes. What rotation withdraws is access to the
//! content *from here on* — this node's copy, and every update to it. Nothing can
//! withdraw what somebody already took.
//!
//! It is expensive in a way re-sealing is not — every byte is read, encrypted again and
//! written again — so it is a separate command with its own survey, rather than a flag
//! on one that costs almost nothing.

use anyhow::{Context, Result};

use nexusfs_crypto::Identity;
use nexusfs_proto::{FileEncryption, FsOpKind};
use nexusfs_storage::Hash;

use crate::inode::ROOT_INODE;
use crate::object::{EntryType, Object};
use crate::state::CoreState;

/// What one re-sealing pass did.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ShareReport {
    /// Files examined.
    pub files_scanned: usize,
    /// Files that were already sealed to exactly the current recipients.
    pub already_current: usize,
    /// Files re-sealed, each of which emitted one operation.
    pub resealed: usize,
    /// Encrypted files this node could not open, and so could not re-seal.
    pub unreadable: usize,
    /// Files stored as plaintext, which have no envelopes to bring up to date.
    pub plaintext: usize,
    /// True when this was a survey and nothing was written.
    pub dry_run: bool,
}

/// What one rotation pass did.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RotateReport {
    pub files_scanned: usize,
    /// Files re-encrypted, each of which emitted one operation.
    pub rotated: usize,
    /// Encrypted files this node could not open, and so could not rotate.
    pub unreadable: usize,
    /// Files stored as plaintext, which have no key to rotate.
    pub plaintext: usize,
    /// Content bytes that were, or would be, re-encrypted.
    pub bytes: u64,
    /// True when this was a survey and nothing was written.
    pub dry_run: bool,
}

impl CoreState {
    /// Re-seal every encrypted file to the current recipient set.
    ///
    /// Surveys when `dry_run`. See the module docs: this grants access and never
    /// withdraws it.
    pub fn reseal_to_recipients(
        &self,
        identity: &Identity,
        now_ms: u64,
        dry_run: bool,
    ) -> Result<ShareReport> {
        let mut report = ShareReport {
            dry_run,
            ..Default::default()
        };

        let recipients = self.sealing_recipient_keys()?;
        if recipients.is_empty() {
            anyhow::bail!(
                "this node has no sealing key, so it cannot seal to anyone. Nothing to do."
            );
        }

        for (inode, node_hash) in self.encrypted_files()? {
            report.files_scanned += 1;

            let Some(Object::FileNode(file)) = self.get_object(&node_hash)? else {
                continue;
            };
            let Some(info) = &file.encryption else {
                report.plaintext += 1;
                continue;
            };

            let Ok(file_key) = self.recover_file_key(info) else {
                // Not a recipient, and no repository key that opens it. Nothing to
                // re-seal with — reported rather than fatal, because one such file
                // should not stop the rest.
                report.unreadable += 1;
                continue;
            };

            // Already addressed to exactly this set. Decided by the keyed digest rather
            // than by counting envelopes: two different sets of the same size would
            // compare equal, and this node's recipient list is not necessarily the one
            // the file was written against.
            let want = nexusfs_crypto::envelope::recipients_digest(&file_key, &recipients);
            if info.sealed_key.is_none() && info.recipients_digest == Some(want) {
                report.already_current += 1;
                continue;
            }

            if dry_run {
                report.resealed += 1;
                continue;
            }

            let sealed = recipients
                .iter()
                .map(|key| nexusfs_crypto::envelope::seal(*key, &file_key))
                .collect::<Result<Vec<_>>>()
                .context("re-seal the file key")?;

            // A Write carrying the same chunks and the same size: only the envelopes
            // differ. It takes the fast path in `apply`, so no content is read back.
            self.apply_local(
                identity,
                FsOpKind::Write {
                    inode,
                    offset: 0,
                    chunks: file.chunks.clone(),
                    new_size: file.size,
                    encryption: Some(FileEncryption {
                        sealed_key: None,
                        recipients: sealed,
                        recipients_digest: Some(want),
                    }),
                },
                now_ms,
            )?;
            report.resealed += 1;
        }

        Ok(report)
    }

    /// Re-encrypt content under fresh keys, sealed to the current recipients.
    ///
    /// Surveys when `dry_run`. `only_path` limits the run to one file; `None` rotates
    /// everything encrypted.
    ///
    /// See the module docs: this withdraws access to the content from here on, and
    /// cannot withdraw what a device already copied.
    pub fn rotate_content(
        &self,
        identity: &Identity,
        only_path: Option<&str>,
        now_ms: u64,
        dry_run: bool,
    ) -> Result<RotateReport> {
        let mut report = RotateReport {
            dry_run,
            ..Default::default()
        };

        if self.cipher.is_none() && self.sealing_secret.is_none() {
            anyhow::bail!(
                "this node has no key material configured, so there is nothing to rotate"
            );
        }

        let target = match only_path {
            Some(path) => {
                let (inode, _, _) = self
                    .stat_file(path)?
                    .ok_or_else(|| anyhow::anyhow!("no such file: {path}"))?;
                Some(inode)
            }
            None => None,
        };

        for (inode, node_hash) in self.encrypted_files()? {
            if target.is_some_and(|want| want != inode) {
                continue;
            }
            report.files_scanned += 1;

            let Some(Object::FileNode(file)) = self.get_object(&node_hash)? else {
                continue;
            };
            if file.encryption.is_none() {
                report.plaintext += 1;
                continue;
            }

            // Reading is what proves this node can rotate the file at all, and it is
            // also the content that has to be re-encrypted — so it is not a wasted
            // check even on the survey.
            let Ok(plain) = self.materialize_file(&file) else {
                report.unreadable += 1;
                continue;
            };

            report.bytes += plain.len() as u64;
            if dry_run {
                report.rotated += 1;
                continue;
            }

            // A fresh key comes from `store_content`, which mints one per write — the
            // same property that makes deriving chunk nonces from the key safe.
            let (chunks, encryption) = self.store_content(&plain)?;
            self.apply_local(
                identity,
                FsOpKind::Write {
                    inode,
                    offset: 0,
                    chunks,
                    new_size: plain.len() as u64,
                    encryption,
                },
                now_ms,
            )?;
            report.rotated += 1;
        }

        if only_path.is_some() && report.files_scanned == 0 {
            anyhow::bail!(
                "{} is not an encrypted file, so it has no key to rotate",
                only_path.unwrap_or_default()
            );
        }

        Ok(report)
    }

    /// Live files that carry an encryption record, as `(inode, file node hash)`.
    fn encrypted_files(&self) -> Result<Vec<(u128, Hash)>> {
        let mut out = Vec::new();
        let mut queue = vec![ROOT_INODE];
        let mut seen = std::collections::BTreeSet::new();

        while let Some(dir) = queue.pop() {
            for entry in self.materialize_dir(dir)? {
                if !seen.insert(entry.inode_id) {
                    continue;
                }
                match entry.entry_type {
                    EntryType::Dir => queue.push(entry.inode_id),
                    EntryType::File => {
                        if let Some(record) = self.load_inode(entry.inode_id)? {
                            if let Some(hash) = record.content.value.node_hash {
                                out.push((entry.inode_id, hash));
                            }
                        }
                    }
                }
            }
        }
        Ok(out)
    }
}
