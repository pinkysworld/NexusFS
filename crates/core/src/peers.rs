//! The pinned peer keys.
//!
//! Kept in `core` rather than in the networking crate because enrolment is an operator
//! task, not a transport one. An operator must be able to enrol a peer, inspect what is
//! trusted, and revoke a key on a node built without QUIC — and, more importantly,
//! *before* the first connection, which is the whole point of not relying on
//! trust-on-first-use.
//!
//! What is stored is a device id to ed25519 public key mapping. `net` reads it to
//! decide whether to accept a session; nothing here makes that decision.

use anyhow::{bail, Result};

use nexusfs_proto::DeviceId;

use crate::state::{CoreState, CF_META};

const PEER_PREFIX: &[u8] = b"peer/key/";

fn peer_key(device: DeviceId) -> Vec<u8> {
    let mut k = PEER_PREFIX.to_vec();
    k.extend_from_slice(&device.0.to_be_bytes());
    k
}

/// One enrolled peer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EnrolledPeer {
    pub device_id: DeviceId,
    pub pubkey: [u8; 32],
}

/// What `enrol` did, so the caller can tell the operator whether anything changed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Enrolment {
    /// The device was not known before.
    Added,
    /// Already enrolled with exactly this key.
    Unchanged,
    /// Already enrolled with a *different* key, and the caller allowed replacement.
    Rotated,
}

impl CoreState {
    /// The key pinned for `device`, if any.
    pub fn peer_key(&self, device: DeviceId) -> Result<Option<[u8; 32]>> {
        let Some(raw) = self.stores.kv.get_kv(CF_META, &peer_key(device))? else {
            return Ok(None);
        };
        if raw.len() != 32 {
            bail!("pinned key for device {:x} is not 32 bytes", device.0);
        }
        let mut key = [0u8; 32];
        key.copy_from_slice(&raw);
        Ok(Some(key))
    }

    /// Every peer enrolled so far, ordered by device id.
    pub fn enrolled_peers(&self) -> Result<Vec<EnrolledPeer>> {
        let mut out = Vec::new();
        for (k, v) in self.stores.kv.scan_prefix(CF_META, PEER_PREFIX)? {
            if k.len() != PEER_PREFIX.len() + 16 || v.len() != 32 {
                continue;
            }
            let mut id = [0u8; 16];
            id.copy_from_slice(&k[PEER_PREFIX.len()..]);
            let mut pubkey = [0u8; 32];
            pubkey.copy_from_slice(&v);
            out.push(EnrolledPeer {
                device_id: DeviceId(u128::from_be_bytes(id)),
                pubkey,
            });
        }
        out.sort_by_key(|p| p.device_id.0);
        Ok(out)
    }

    /// Pin `pubkey` for `device`.
    ///
    /// Replacing an existing, different key requires `allow_rotation`. Silently
    /// overwriting would erase the one signal that distinguishes a planned key rotation
    /// from an impersonation attempt, which is exactly what pinning exists to catch.
    pub fn enrol_peer(
        &self,
        device: DeviceId,
        pubkey: &[u8; 32],
        allow_rotation: bool,
    ) -> Result<Enrolment> {
        match self.peer_key(device)? {
            Some(existing) if existing == *pubkey => Ok(Enrolment::Unchanged),
            Some(_) if !allow_rotation => bail!(
                "device {:x} is already enrolled with a different key. If this is a \
                 deliberate rotation, re-run with --rotate; if it is not, the peer is \
                 not who it claims to be.",
                device.0
            ),
            Some(_) => {
                self.stores.kv.put_kv(CF_META, &peer_key(device), pubkey)?;
                Ok(Enrolment::Rotated)
            }
            None => {
                self.stores.kv.put_kv(CF_META, &peer_key(device), pubkey)?;
                Ok(Enrolment::Added)
            }
        }
    }

    /// Forget a peer's key. Returns whether one was there to forget.
    ///
    /// With trust-on-first-use enabled the device can simply be pinned again on its
    /// next connection, so revocation is only durable when TOFU is off.
    pub fn revoke_peer(&self, device: DeviceId) -> Result<bool> {
        if self.peer_key(device)?.is_none() {
            return Ok(false);
        }
        self.stores.kv.delete_kv(CF_META, &peer_key(device))?;
        Ok(true)
    }
}
