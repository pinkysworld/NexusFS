//! The pinned peer keys.
//!
//! Kept in `core` rather than in the networking crate because enrolment is an operator
//! task, not a transport one. An operator must be able to enrol a peer, inspect what is
//! trusted, and revoke a key on a node built without QUIC — and, more importantly,
//! *before* the first connection, which is the whole point of not relying on
//! trust-on-first-use.
//!
//! Two things are stored per device, in two key families rather than one record:
//!
//! - the **ed25519 signing key**, which `net` reads to decide whether to accept a
//!   session. Nothing here makes that decision.
//! - the **X25519 sealing key**, which the write path reads to seal file keys to that
//!   peer. A device may be enrolled without one — every enrolment made before sealing
//!   existed is — and that is a meaningful state rather than an error: such a peer can
//!   replicate and verify, and simply cannot be made a recipient of new encrypted
//!   content.
//!
//! Two families rather than one wider value, because a value is a fixed-width byte
//! string here: widening it would make every existing enrolment undecodable, and the
//! whole point of pinning is that the operator does not have to redo it.

use anyhow::{bail, Result};

use nexusfs_proto::DeviceId;

use crate::state::{CoreState, CF_META};

const PEER_PREFIX: &[u8] = b"peer/key/";
const SEAL_PREFIX: &[u8] = b"peer/seal/";

fn peer_key(device: DeviceId) -> Vec<u8> {
    prefixed(PEER_PREFIX, device)
}

fn seal_key_for(device: DeviceId) -> Vec<u8> {
    prefixed(SEAL_PREFIX, device)
}

fn prefixed(prefix: &[u8], device: DeviceId) -> Vec<u8> {
    let mut k = prefix.to_vec();
    k.extend_from_slice(&device.0.to_be_bytes());
    k
}

/// One enrolled peer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EnrolledPeer {
    pub device_id: DeviceId,
    pub pubkey: [u8; 32],
    /// The X25519 key to seal file keys to, when one has been enrolled.
    ///
    /// `None` for a peer enrolled before sealing existed. Such a peer still replicates
    /// and still verifies; it just cannot receive new encrypted content, which is a
    /// state worth showing an operator rather than papering over.
    pub seal_key: Option<[u8; 32]>,
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

    /// The sealing key pinned for `device`, if one has been enrolled.
    pub fn peer_seal_key(&self, device: DeviceId) -> Result<Option<[u8; 32]>> {
        let Some(raw) = self.stores.kv.get_kv(CF_META, &seal_key_for(device))? else {
            return Ok(None);
        };
        let key: [u8; 32] = raw.as_slice().try_into().map_err(|_| {
            anyhow::anyhow!("sealing key for device {:x} is not 32 bytes", device.0)
        })?;
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
            let device_id = DeviceId(u128::from_be_bytes(id));
            out.push(EnrolledPeer {
                device_id,
                pubkey,
                seal_key: self.peer_seal_key(device_id)?,
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
        seal_key: Option<&[u8; 32]>,
        allow_rotation: bool,
    ) -> Result<Enrolment> {
        // The sealing key is written first and on every path, including `Unchanged`:
        // re-running enrolment with a sealing key added is how an operator upgrades a
        // peer pinned before sealing existed, and refusing to record it because the
        // signing key had not moved would make that impossible.
        if let Some(seal) = seal_key {
            let existing = self.peer_seal_key(device)?;
            if existing.as_ref() != Some(seal) {
                if existing.is_some() && !allow_rotation {
                    bail!(
                        "device {:x} is already enrolled with a different sealing key. \
                         If this is a deliberate rotation, re-run with --rotate.",
                        device.0
                    );
                }
                self.stores
                    .kv
                    .put_kv(CF_META, &seal_key_for(device), seal)?;
                self.flush()?;
            }
        }

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
                // Trust changes do not ride an operation, so they carry their own
                // durability point rather than relying on the database being dropped
                // cleanly — which a killed process never does.
                self.flush()?;
                Ok(Enrolment::Rotated)
            }
            None => {
                self.stores.kv.put_kv(CF_META, &peer_key(device), pubkey)?;
                self.flush()?;
                Ok(Enrolment::Added)
            }
        }
    }

    /// Forget a peer's key. Returns whether one was there to forget.
    ///
    /// With trust-on-first-use enabled the device can simply be pinned again on its
    /// next connection, so revocation is only durable when TOFU is off.
    pub fn revoke_peer(&self, device: DeviceId) -> Result<bool> {
        if self.peer_key(device)?.is_none() && self.peer_seal_key(device)?.is_none() {
            return Ok(false);
        }
        self.stores.kv.delete_kv(CF_META, &peer_key(device))?;
        // Both, always. Leaving the sealing key behind would keep the device a
        // recipient of newly written content after being revoked as a peer, which is
        // the opposite of what revoking means.
        self.stores.kv.delete_kv(CF_META, &seal_key_for(device))?;
        self.flush()?;
        Ok(true)
    }

    /// Every device that can receive sealed content: enrolled peers with a sealing key.
    ///
    /// Deliberately not "all enrolled peers": a peer without a sealing key would be
    /// silently skipped, and a file that a peer cannot read is better noticed at the
    /// point of enrolment than at the point of reading.
    pub fn sealing_recipients(&self) -> Result<Vec<(DeviceId, [u8; 32])>> {
        Ok(self
            .enrolled_peers()?
            .into_iter()
            .filter_map(|p| p.seal_key.map(|k| (p.device_id, k)))
            .collect())
    }
}
