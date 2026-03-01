use anyhow::{Context, Result};
use ed25519_dalek::{Signature, SigningKey, VerifyingKey, SIGNATURE_LENGTH};
use ed25519_dalek::{Signer, Verifier};

pub fn sign(sk: &SigningKey, msg: &[u8]) -> Vec<u8> {
    let sig: Signature = sk.sign(msg);
    sig.to_bytes().to_vec()
}

pub fn verify(vk_bytes: &[u8; 32], msg: &[u8], sig_bytes: &[u8]) -> Result<()> {
    if sig_bytes.len() != SIGNATURE_LENGTH {
        anyhow::bail!("invalid signature length");
    }
    let vk = VerifyingKey::from_bytes(vk_bytes).context("invalid verifying key")?;
    let mut sig_arr = [0u8; SIGNATURE_LENGTH];
    sig_arr.copy_from_slice(sig_bytes);
    let sig = Signature::from_bytes(&sig_arr);
    vk.verify(msg, &sig).context("signature verify failed")?;
    Ok(())
}
