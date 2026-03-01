use anyhow::{Context, Result};
use nexusfs_crypto::{sign as sign_bytes, verify as verify_sig, Identity};
use nexusfs_proto::{BuildInfo, DeviceId, MessageEnvelope, Msg};
use rand::RngCore;

/// Protocol version (v0).
pub const PROTOCOL_VERSION: u16 = 1;

/// Max message size accepted (bytes).
pub const MAX_FRAME_LEN: usize = 8 * 1024 * 1024;

#[derive(Debug, Clone)]
pub struct PeerHello {
    pub device_id: DeviceId,
    pub pubkey: [u8; 32],
    pub features: Vec<String>,
}

/// Create a signed Hello message.
pub fn make_hello(
    identity: &Identity,
    device_id: DeviceId,
    features: Vec<String>,
    build: BuildInfo,
    now_ms: u64,
) -> Result<Msg> {
    let mut nonce = [0u8; 32];
    rand::thread_rng().fill_bytes(&mut nonce);

    // Sign canonical fields (excluding sig)
    #[derive(serde::Serialize)]
    struct HelloToSign<'a> {
        device_id: DeviceId,
        pubkey_ed25519: [u8; 32],
        features: &'a [String],
        build: &'a BuildInfo,
        time_unix_ms: u64,
        nonce: [u8; 32],
    }

    let to_sign = HelloToSign {
        device_id,
        pubkey_ed25519: identity.pubkey_bytes(),
        features: &features,
        build: &build,
        time_unix_ms: now_ms,
        nonce,
    };

    let bytes = postcard::to_stdvec(&to_sign).context("encode hello to sign")?;
    let sig = sign_bytes(identity.signing_key(), &bytes);

    Ok(Msg::Hello {
        device_id,
        pubkey_ed25519: identity.pubkey_bytes(),
        features,
        build,
        time_unix_ms: now_ms,
        nonce,
        sig,
    })
}

/// Verify a received Hello message and return PeerHello info.
pub fn verify_hello(msg: &Msg) -> Result<PeerHello> {
    let Msg::Hello {
        device_id,
        pubkey_ed25519,
        features,
        build,
        time_unix_ms,
        nonce,
        sig,
    } = msg
    else {
        anyhow::bail!("not a Hello message");
    };

    #[derive(serde::Serialize)]
    struct HelloToSign<'a> {
        device_id: DeviceId,
        pubkey_ed25519: [u8; 32],
        features: &'a [String],
        build: &'a BuildInfo,
        time_unix_ms: u64,
        nonce: [u8; 32],
    }

    let to_sign = HelloToSign {
        device_id: *device_id,
        pubkey_ed25519: *pubkey_ed25519,
        features,
        build,
        time_unix_ms: *time_unix_ms,
        nonce: *nonce,
    };

    let bytes = postcard::to_stdvec(&to_sign).context("encode hello to sign")?;
    verify_sig(pubkey_ed25519, &bytes, sig)?;

    Ok(PeerHello {
        device_id: *device_id,
        pubkey: *pubkey_ed25519,
        features: features.clone(),
    })
}

/// Helper to build a MessageEnvelope.
pub fn env(msg_id: u64, reply_to: Option<u64>, payload: Msg) -> MessageEnvelope {
    MessageEnvelope {
        protocol_version: PROTOCOL_VERSION,
        msg_id,
        reply_to,
        payload,
    }
}
