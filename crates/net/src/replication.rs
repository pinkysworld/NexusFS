use anyhow::{Context, Result};
use nexusfs_crypto::{sign as sign_bytes, verify as verify_sig, Identity};
use nexusfs_proto::{BuildInfo, DeviceId, MessageEnvelope, Msg};
use rand::RngCore;

/// Protocol version (v0).
/// Bumped for v2's Merkle state commitment, and again for v3's per-recipient sealing.
///
/// v2's wire format did not change, but the meaning of a state root did. Two peers on
/// different versions would sync operations happily and then report different roots
/// forever, which looks like corruption. Refusing the handshake says what is actually
/// wrong.
///
/// v3 does change the wire format: a `Write` carrying encrypted content now describes
/// how its file key is protected in a different shape. Unencrypted operations are
/// byte-identical across the two, so the failure a mismatched pair would hit is narrow
/// and would look like sporadic decode errors on some files and not others — worse to
/// diagnose than a refused handshake, not better.
///
/// v4 added `Notify`/`Noted` for push notification. A v3 peer answers a notification
/// with an `Error` and carries on polling, so the failure there is mild — but the
/// message enum is postcard-encoded by variant *index*, and inserting variants shifts
/// every one after them. That is the kind of change that decodes into something else
/// rather than failing, which is exactly what a version number is for.
pub const PROTOCOL_VERSION: u16 = 4;

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

/// Canonical bytes covered by a HelloAck signature.
///
/// Includes the initiator's nonce, which binds the reply to one specific Hello. Without
/// it a recorded HelloAck could be replayed at a later session by someone who never
/// held the key.
#[derive(serde::Serialize)]
struct AckToSign<'a> {
    accepted: bool,
    reason: &'a Option<String>,
    features: &'a [String],
    peer_device: DeviceId,
    peer_pubkey: [u8; 32],
    nonce: [u8; 32],
}

/// Build a signed HelloAck answering the Hello that carried `nonce`.
pub fn make_hello_ack(
    identity: &Identity,
    device_id: DeviceId,
    accepted: bool,
    reason: Option<String>,
    features: Vec<String>,
    nonce: [u8; 32],
) -> Result<Msg> {
    let to_sign = AckToSign {
        accepted,
        reason: &reason,
        features: &features,
        peer_device: device_id,
        peer_pubkey: identity.pubkey_bytes(),
        nonce,
    };
    let bytes = postcard::to_stdvec(&to_sign).context("encode hello ack to sign")?;
    let sig = sign_bytes(identity.signing_key(), &bytes);

    Ok(Msg::HelloAck {
        accepted,
        reason,
        features,
        peer_device: device_id,
        peer_pubkey: identity.pubkey_bytes(),
        nonce,
        sig,
    })
}

/// Verify a HelloAck and confirm it answers the nonce we sent.
pub fn verify_hello_ack(msg: &Msg, expected_nonce: &[u8; 32]) -> Result<PeerHello> {
    let Msg::HelloAck {
        accepted,
        reason,
        features,
        peer_device,
        peer_pubkey,
        nonce,
        sig,
    } = msg
    else {
        anyhow::bail!("not a HelloAck message");
    };

    if nonce != expected_nonce {
        anyhow::bail!("HelloAck answered a different handshake");
    }

    let to_sign = AckToSign {
        accepted: *accepted,
        reason,
        features,
        peer_device: *peer_device,
        peer_pubkey: *peer_pubkey,
        nonce: *nonce,
    };
    let bytes = postcard::to_stdvec(&to_sign).context("encode hello ack to sign")?;
    verify_sig(peer_pubkey, &bytes, sig).context("HelloAck signature")?;

    if !accepted {
        anyhow::bail!(
            "peer refused the handshake: {}",
            reason.clone().unwrap_or_default()
        );
    }

    Ok(PeerHello {
        device_id: *peer_device,
        pubkey: *peer_pubkey,
        features: features.clone(),
    })
}

/// Read the nonce out of a Hello, so a responder can echo it under its own signature.
pub fn hello_nonce(msg: &Msg) -> Result<[u8; 32]> {
    match msg {
        Msg::Hello { nonce, .. } => Ok(*nonce),
        _ => anyhow::bail!("not a Hello message"),
    }
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
