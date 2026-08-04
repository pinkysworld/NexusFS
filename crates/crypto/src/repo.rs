//! At-rest encryption for chunk content.
//!
//! # Why content stays addressed by ciphertext
//!
//! Chunks are named by the hash of the bytes actually stored, which are the encrypted
//! bytes. That is the only choice compatible with the replication path: a peer
//! verifies `hash(received) == requested` before storing anything, and a peer that
//! cannot decrypt must still be able to perform that check. Addressing by plaintext
//! hash would force every verifier to hold the key, and would additionally let anyone
//! holding a candidate file confirm whether you store it.
//!
//! The cost is that identical plaintext under different file keys deduplicates to
//! nothing. Convergent encryption would recover that, at the price of exactly the
//! confirmation-of-file attack just described, so it is not used.
//!
//! # Key hierarchy
//!
//! - A **repository key** lives beside the device identity and never moves.
//! - Each write mints a fresh random **file key**, which encrypts that file's chunks.
//! - The file key is sealed with the repository key and stored inside the `FileNode`,
//!   so it replicates with the file and needs no side channel.
//!
//! A fresh file key per write is what makes deriving chunk nonces from
//! `(file key, chunk index)` safe: a nonce is never reused under the same key, because
//! rewriting a file changes the key.
//!
//! Replicas therefore share one repository key. Distributing distinct keys per peer is
//! what `envelope.rs` is for, and is not wired into the write path yet.

use anyhow::{bail, Context, Result};
use chacha20poly1305::aead::{Aead, AeadCore, KeyInit, OsRng};
use chacha20poly1305::{XChaCha20Poly1305, XNonce};

/// Domain separator so chunk nonces cannot collide with any other BLAKE3 use.
const NONCE_DOMAIN: &[u8] = b"nexusfs/chunk-nonce/v1";

/// Encrypts chunk content under a repository key.
#[derive(Clone)]
pub struct RepoCipher {
    key: [u8; 32],
}

impl std::fmt::Debug for RepoCipher {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // Never let the key reach a log line.
        f.write_str("RepoCipher(<redacted>)")
    }
}

impl RepoCipher {
    pub fn new(key: [u8; 32]) -> Self {
        Self { key }
    }

    /// Mint a fresh key for one file's content.
    pub fn new_file_key() -> [u8; 32] {
        use rand::RngCore;
        let mut key = [0u8; 32];
        rand::rngs::OsRng.fill_bytes(&mut key);
        key
    }

    /// Deterministic per-chunk nonce.
    ///
    /// Safe only because the file key is fresh for every write; the same index under a
    /// new key produces a different nonce.
    fn chunk_nonce(file_key: &[u8; 32], index: u64) -> [u8; 24] {
        let mut hasher = blake3::Hasher::new_keyed(file_key);
        hasher.update(NONCE_DOMAIN);
        hasher.update(&index.to_le_bytes());

        let mut nonce = [0u8; 24];
        nonce.copy_from_slice(&hasher.finalize().as_bytes()[..24]);
        nonce
    }

    /// Encrypt one chunk of a file.
    pub fn seal_chunk(file_key: &[u8; 32], index: u64, plaintext: &[u8]) -> Result<Vec<u8>> {
        let cipher = XChaCha20Poly1305::new(file_key.into());
        let nonce = Self::chunk_nonce(file_key, index);
        cipher
            .encrypt(XNonce::from_slice(&nonce), plaintext)
            .map_err(|_| anyhow::anyhow!("chunk encryption failed"))
    }

    /// Decrypt one chunk of a file.
    pub fn open_chunk(file_key: &[u8; 32], index: u64, ciphertext: &[u8]) -> Result<Vec<u8>> {
        let cipher = XChaCha20Poly1305::new(file_key.into());
        let nonce = Self::chunk_nonce(file_key, index);
        cipher
            .decrypt(XNonce::from_slice(&nonce), ciphertext)
            .map_err(|_| anyhow::anyhow!("chunk failed authentication; wrong key or tampered data"))
    }

    /// Seal a file key so it can be stored inside the file's own metadata.
    ///
    /// Layout: 24-byte nonce followed by the AEAD ciphertext.
    pub fn seal_file_key(&self, file_key: &[u8; 32]) -> Result<Vec<u8>> {
        let cipher = XChaCha20Poly1305::new((&self.key).into());
        let nonce = XChaCha20Poly1305::generate_nonce(&mut OsRng);
        let mut sealed = cipher
            .encrypt(&nonce, file_key.as_slice())
            .map_err(|_| anyhow::anyhow!("sealing the file key failed"))?;

        let mut out = nonce.to_vec();
        out.append(&mut sealed);
        Ok(out)
    }

    /// Recover a file key sealed by [`seal_file_key`].
    pub fn open_file_key(&self, sealed: &[u8]) -> Result<[u8; 32]> {
        if sealed.len() < 24 {
            bail!("sealed file key is truncated");
        }
        let (nonce, ciphertext) = sealed.split_at(24);

        let cipher = XChaCha20Poly1305::new((&self.key).into());
        let plain = cipher
            .decrypt(XNonce::from_slice(nonce), ciphertext)
            .map_err(|_| anyhow::anyhow!("could not unseal the file key; wrong repository key?"))?;

        plain
            .try_into()
            .map_err(|_| anyhow::anyhow!("unsealed file key has the wrong length"))
    }

    /// Load the repository key from `path`, creating it on first use.
    ///
    /// Written with owner-only permissions: anyone who can read this file can read
    /// every chunk the node stores.
    #[cfg(feature = "std-fs")]
    pub fn load_or_create(path: impl AsRef<std::path::Path>) -> Result<Self> {
        use std::io::Write;

        let path = path.as_ref();
        if path.exists() {
            let encoded = std::fs::read_to_string(path).context("read repository key")?;
            let raw = hex::decode(encoded.trim()).context("decode repository key")?;
            let key: [u8; 32] = raw
                .try_into()
                .map_err(|_| anyhow::anyhow!("repository key must be 32 bytes"))?;
            return Ok(Self::new(key));
        }

        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).ok();
        }

        let key = Self::new_file_key();
        let mut file = std::fs::File::create(path).context("create repository key")?;
        file.write_all(hex::encode(key).as_bytes())
            .context("write repository key")?;

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))
                .context("restrict repository key permissions")?;
        }

        Ok(Self::new(key))
    }
}
