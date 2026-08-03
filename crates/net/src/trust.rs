//! Which peers we are willing to accept operations from.
//!
//! Transport encryption does not answer this question. TLS proves you are talking to
//! whoever holds the certificate; it says nothing about whether that party is the peer
//! you synced with yesterday. Identity here is the ed25519 key that signs the Hello,
//! pinned per device on first contact.

use anyhow::{bail, Result};

use nexusfs_core::CoreState;
use nexusfs_proto::DeviceId;

const CF_META: &str = "meta";
const PEER_PREFIX: &[u8] = b"peer/key/";

fn peer_key(device: DeviceId) -> Vec<u8> {
    let mut k = PEER_PREFIX.to_vec();
    k.extend_from_slice(&device.0.to_be_bytes());
    k
}

/// Trust-on-first-use policy for peer public keys.
#[derive(Clone, Copy, Debug)]
pub struct TrustPolicy {
    /// Accept and pin an unknown device the first time it is seen.
    pub tofu: bool,
}

/// Check a peer's key against what we have pinned, pinning it if this is first contact.
///
/// A device presenting a *different* key than the one pinned is rejected regardless of
/// policy: that is either key rotation, which needs an explicit re-enrolment, or
/// someone impersonating a device we trust.
pub fn authorize(
    core: &CoreState,
    policy: TrustPolicy,
    device: DeviceId,
    pubkey: &[u8; 32],
) -> Result<()> {
    let stored = core.stores.kv.get_kv(CF_META, &peer_key(device))?;

    match stored {
        Some(known) if known == pubkey => Ok(()),
        Some(_) => bail!(
            "device {:x} presented a different key than the one pinned; \
             refusing to sync (re-enrol the peer if this was a deliberate rotation)",
            device.0
        ),
        None if policy.tofu => {
            core.stores.kv.put_kv(CF_META, &peer_key(device), pubkey)?;
            Ok(())
        }
        None => bail!(
            "device {:x} is not enrolled and trust-on-first-use is disabled",
            device.0
        ),
    }
}

/// Every peer key pinned so far.
pub fn known_peers(core: &CoreState) -> Result<Vec<(DeviceId, [u8; 32])>> {
    let mut out = Vec::new();
    for (k, v) in core.stores.kv.scan_prefix(CF_META, PEER_PREFIX)? {
        if k.len() != PEER_PREFIX.len() + 16 || v.len() != 32 {
            continue;
        }
        let mut id = [0u8; 16];
        id.copy_from_slice(&k[PEER_PREFIX.len()..]);
        let mut key = [0u8; 32];
        key.copy_from_slice(&v);
        out.push((DeviceId(u128::from_be_bytes(id)), key));
    }
    Ok(out)
}
