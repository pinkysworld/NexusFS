use anyhow::{Context, Result};
use chacha20poly1305::aead::{Aead, AeadCore, KeyInit};
use chacha20poly1305::XChaCha20Poly1305;
use rand::rngs::OsRng;
use x25519_dalek::{EphemeralSecret, PublicKey};

/// A simple key envelope using X25519 to derive a shared secret, then XChaCha20-Poly1305.
/// This is a placeholder for a proper HPKE-like construction.
#[derive(Debug, Clone)]
pub struct Envelope {
    pub sender_ephemeral_pub: [u8; 32],
    pub nonce: [u8; 24],
    pub ciphertext: Vec<u8>,
}

fn kdf(shared: [u8; 32]) -> [u8; 32] {
    // Placeholder KDF: BLAKE3(shared)
    blake3::hash(&shared).into()
}

/// Encrypt a `file_key` to `recipient_pub` using ephemeral X25519.
pub fn seal(recipient_pub: [u8; 32], file_key: &[u8], aad: &[u8]) -> Result<Envelope> {
    let recipient = PublicKey::from(recipient_pub);
    let eph = EphemeralSecret::random_from_rng(OsRng);
    let eph_pub = PublicKey::from(&eph);

    let shared = eph.diffie_hellman(&recipient);
    let key = kdf(shared.to_bytes());

    let cipher = XChaCha20Poly1305::new((&key).into());
    let nonce = XChaCha20Poly1305::generate_nonce(&mut OsRng);
    let ct = cipher
        .encrypt(
            &nonce,
            chacha20poly1305::aead::Payload { msg: file_key, aad },
        )
        .context("envelope encrypt")?;

    Ok(Envelope {
        sender_ephemeral_pub: eph_pub.to_bytes(),
        nonce: nonce.into(),
        ciphertext: ct,
    })
}

/// Decrypt an envelope with recipient private key.
/// NOTE: This skeleton does not define how recipient private keys are stored; integrate later.
pub fn open(_recipient_secret: [u8; 32], _env: &Envelope, _aad: &[u8]) -> Result<Vec<u8>> {
    anyhow::bail!("Envelope open is not implemented in skeleton (needs recipient secret handling).")
}
