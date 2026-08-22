//! Sealing a file key to one recipient.
//!
//! X25519 to a fresh ephemeral key, then XChaCha20-Poly1305 under a key derived from
//! the shared secret. Not HPKE — there is no key schedule, no info string, no exporter
//! — but the shape is the same one HPKE's base mode has, and it is what lets a file key
//! reach a recipient with no prior exchange beyond that recipient's enrolled public key.
//!
//! What binds an envelope to its file is *not* the AAD. The AAD names only the
//! recipient, which stops an envelope being replayed at a different device; the binding
//! to a particular file comes from the operation signature, which covers the whole
//! `Write` — envelopes, chunk references and all. Moving an envelope between files means
//! forging that signature, and an attacker who can do that does not need to move
//! envelopes.

use anyhow::{Context, Result};
use chacha20poly1305::aead::{Aead, AeadCore, KeyInit};
use chacha20poly1305::{XChaCha20Poly1305, XNonce};
use rand::rngs::OsRng;
use x25519_dalek::{EphemeralSecret, PublicKey, StaticSecret};

pub use nexusfs_proto::SealedFileKey;

/// Domain separator, so a derived envelope key cannot collide with any other BLAKE3 use
/// in this workspace.
const KDF_DOMAIN: &[u8] = b"nexusfs/envelope-key/v1";

/// Domain separator for the additional data an envelope is bound to.
const AAD_DOMAIN: &[u8] = b"nexusfs/envelope-aad/v1";

fn kdf(shared: [u8; 32]) -> [u8; 32] {
    let mut hasher = blake3::Hasher::new_keyed(&shared);
    hasher.update(KDF_DOMAIN);
    hasher.finalize().into()
}

/// What an envelope is authenticated against: the recipient it was sealed for.
///
/// Binding to the recipient is what stops an envelope being lifted from one device's
/// slot and presented as another's. It deliberately does not bind to the file — see the
/// module docs for why the operation signature is the right place for that.
fn aad_for(recipient_pub: &[u8; 32]) -> Vec<u8> {
    let mut aad = AAD_DOMAIN.to_vec();
    aad.extend_from_slice(recipient_pub);
    aad
}

/// Domain separator for the recipient-set digest.
const RECIPIENTS_DOMAIN: &[u8] = b"nexusfs/recipient-set/v1";

/// A digest of the set of recipients a file key was sealed to.
///
/// Keyed by the file key, so only a reader can test a candidate set. Sorted first, so
/// the answer depends on the set and not on the order it was assembled in.
pub fn recipients_digest(file_key: &[u8; 32], recipients: &[[u8; 32]]) -> [u8; 32] {
    let mut sorted: Vec<[u8; 32]> = recipients.to_vec();
    sorted.sort_unstable();
    sorted.dedup();

    let mut hasher = blake3::Hasher::new_keyed(file_key);
    hasher.update(RECIPIENTS_DOMAIN);
    for key in &sorted {
        hasher.update(key);
    }
    hasher.finalize().into()
}

/// The X25519 public key matching a secret.
///
/// Exposed so a caller holding only the secret can name itself as a recipient without
/// carrying the pair around, and without this crate's dependency on `x25519_dalek`
/// leaking into every one that needs the answer.
pub fn public_from_secret(secret: [u8; 32]) -> [u8; 32] {
    PublicKey::from(&StaticSecret::from(secret)).to_bytes()
}

/// Seal `file_key` to `recipient_pub` using an ephemeral X25519 key.
pub fn seal(recipient_pub: [u8; 32], file_key: &[u8]) -> Result<SealedFileKey> {
    let aad = aad_for(&recipient_pub);
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
            chacha20poly1305::aead::Payload {
                msg: file_key,
                aad: &aad,
            },
        )
        .context("envelope encrypt")?;

    Ok(SealedFileKey {
        ephemeral_pub: eph_pub.to_bytes(),
        nonce: nonce.into(),
        ciphertext: ct,
    })
}

/// Open an envelope with the recipient's X25519 secret.
///
/// The sender's ephemeral public key travels with the envelope, so the recipient
/// re-derives the same shared secret with no prior exchange. Failure is expected and
/// cheap: a reader trials its key against every envelope on a file and most of them are
/// addressed to somebody else.
pub fn open(recipient_secret: [u8; 32], env: &SealedFileKey) -> Result<Vec<u8>> {
    let secret = StaticSecret::from(recipient_secret);
    let aad = aad_for(&PublicKey::from(&secret).to_bytes());
    let sender_ephemeral = PublicKey::from(env.ephemeral_pub);

    let shared = secret.diffie_hellman(&sender_ephemeral);
    let key = kdf(shared.to_bytes());

    let cipher = XChaCha20Poly1305::new((&key).into());
    cipher
        .decrypt(
            XNonce::from_slice(&env.nonce),
            chacha20poly1305::aead::Payload {
                msg: &env.ciphertext,
                aad: &aad,
            },
        )
        .map_err(|_| {
            anyhow::anyhow!("envelope failed authentication; wrong recipient key or tampered data")
        })
}
