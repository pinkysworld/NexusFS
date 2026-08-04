use anyhow::{Context, Result};
use chacha20poly1305::aead::{Aead, AeadCore, KeyInit};
use chacha20poly1305::{XChaCha20Poly1305, XNonce};
use rand::rngs::OsRng;
use x25519_dalek::{EphemeralSecret, PublicKey, StaticSecret};

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

/// Decrypt an envelope with the recipient's X25519 secret.
///
/// The sender's ephemeral public key travels with the envelope, so the recipient can
/// re-derive the same shared secret without any prior exchange. `aad` must match what
/// was passed to [`seal`], which is how the envelope is bound to its context.
pub fn open(recipient_secret: [u8; 32], env: &Envelope, aad: &[u8]) -> Result<Vec<u8>> {
    let secret = StaticSecret::from(recipient_secret);
    let sender_ephemeral = PublicKey::from(env.sender_ephemeral_pub);

    let shared = secret.diffie_hellman(&sender_ephemeral);
    let key = kdf(shared.to_bytes());

    let cipher = XChaCha20Poly1305::new((&key).into());
    cipher
        .decrypt(
            XNonce::from_slice(&env.nonce),
            chacha20poly1305::aead::Payload {
                msg: &env.ciphertext,
                aad,
            },
        )
        .map_err(|_| {
            anyhow::anyhow!("envelope failed authentication; wrong recipient key or tampered data")
        })
}
