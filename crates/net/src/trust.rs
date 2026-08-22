//! Which peers we are willing to accept operations from.
//!
//! Transport encryption does not answer this question. TLS proves you are talking to
//! whoever holds the certificate; it says nothing about whether that party is the peer
//! you synced with yesterday. Identity here is the ed25519 key that signs the Hello,
//! pinned per device on first contact.

//! The pinned keys themselves live in `core`, so an operator can enrol and revoke on a
//! build without networking, and before any connection is attempted. This module owns
//! only the decision.

use anyhow::{bail, Result};

use nexusfs_core::CoreState;
use nexusfs_proto::DeviceId;

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
    match core.peer_key(device)? {
        Some(known) if known == *pubkey => Ok(()),
        Some(_) => bail!(
            "device {:x} presented a different key than the one pinned; \
             refusing to sync (re-enrol the peer if this was a deliberate rotation)",
            device.0
        ),
        None if policy.tofu => {
            core.enrol_peer(device, pubkey, None, false)?;
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
    Ok(core
        .enrolled_peers()?
        .into_iter()
        .map(|p| (p.device_id, p.pubkey))
        .collect())
}
