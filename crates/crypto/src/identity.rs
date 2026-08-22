use anyhow::{Context, Result};
use base64::{engine::general_purpose::STANDARD, Engine as _};
use ed25519_dalek::{SigningKey, VerifyingKey};
use rand::rngs::OsRng;
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::Path;
use x25519_dalek::{PublicKey, StaticSecret};

/// Domain separator for deriving a sealing key from a supplied seed.
const SEAL_DERIVE_DOMAIN: &[u8] = b"nexusfs/seal-key/v1";

/// This device's long-term key material.
///
/// Two keys, for two jobs that must not share one:
///
/// - an **ed25519 signing key**, which authorises operations and the session handshake;
/// - an **X25519 sealing key**, which receives file keys sealed to this device.
///
/// They are independent secrets rather than one mapped into the other's curve. The
/// birational Ed25519-to-Curve25519 map is standard and tempting — it would mean one key
/// to enrol instead of two — but it puts a signing oracle and a Diffie-Hellman oracle
/// over the same secret scalar, and the interactions there are subtle enough that this
/// codebase should not be the place they are first got right.
#[derive(Clone)]
pub struct Identity {
    signing: SigningKey,
    sealing: StaticSecret,
}

impl std::fmt::Debug for Identity {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // Public keys only. `SigningKey`'s own Debug prints the secret, and this type
        // ends up inside structs that get logged.
        f.debug_struct("Identity")
            .field("pubkey", &hex::encode(self.pubkey_bytes()))
            .field("sealing_pubkey", &hex::encode(self.sealing_pubkey()))
            .finish()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct IdentityFile {
    // base64 of 32-byte ed25519 secret key (seed)
    ed25519_seed_b64: String,
    /// base64 of the 32-byte X25519 secret.
    ///
    /// Optional so an identity written before sealing existed still loads. One is
    /// derived and written back on first load, which keeps the file the single thing
    /// worth backing up.
    #[serde(default)]
    x25519_secret_b64: Option<String>,
}

impl Identity {
    pub fn generate() -> Self {
        Self {
            signing: SigningKey::generate(&mut OsRng),
            sealing: StaticSecret::random_from_rng(OsRng),
        }
    }

    /// Build an identity from caller-supplied key material.
    ///
    /// Lets an embedder provide entropy from its own source rather than the OS —
    /// notably the WebAssembly build, which takes a seed from `crypto.getRandomValues`
    /// and so needs no `getrandom` backend compiled in.
    ///
    /// The sealing key is derived from the same seed through a domain-separated KDF.
    /// That is expansion of one secret into two independent keys, not reuse of one key
    /// for two algorithms: neither derived key reveals the other, and a caller with one
    /// seed to offer should not be forced to invent a second.
    pub fn from_seed(seed: [u8; 32]) -> Self {
        Self {
            signing: SigningKey::from_bytes(&seed),
            sealing: StaticSecret::from(Self::derive_sealing_seed(&seed)),
        }
    }

    fn derive_sealing_seed(seed: &[u8; 32]) -> [u8; 32] {
        let mut hasher = blake3::Hasher::new_keyed(seed);
        hasher.update(SEAL_DERIVE_DOMAIN);
        hasher.finalize().into()
    }

    pub fn load_or_create(path: impl AsRef<Path>) -> Result<Self> {
        let path = path.as_ref();
        if !path.exists() {
            if let Some(parent) = path.parent() {
                fs::create_dir_all(parent).ok();
            }
            let id = Self::generate();
            id.save(path)?;
            return Ok(id);
        }

        let txt = fs::read_to_string(path).context("read identity file")?;
        let f: IdentityFile = toml::from_str(&txt).context("parse identity file toml")?;
        let seed = decode_key(&f.ed25519_seed_b64).context("decode ed25519 seed")?;
        let signing = SigningKey::from_bytes(&seed);

        let (sealing, needs_write) = match &f.x25519_secret_b64 {
            Some(encoded) => (
                StaticSecret::from(decode_key(encoded).context("decode x25519 secret")?),
                false,
            ),
            // Written before sealing existed. Derive one from the seed already there
            // rather than minting an unrelated secret, so the file remains recoverable
            // from a backup taken before this and the device keeps one identity.
            None => (StaticSecret::from(Self::derive_sealing_seed(&seed)), true),
        };

        let id = Self { signing, sealing };
        if needs_write {
            id.save(path).context("add a sealing key to the identity")?;
        }
        Ok(id)
    }

    pub fn save(&self, path: impl AsRef<Path>) -> Result<()> {
        let f = IdentityFile {
            ed25519_seed_b64: STANDARD.encode(self.signing.to_bytes()),
            x25519_secret_b64: Some(STANDARD.encode(self.sealing.to_bytes())),
        };
        let txt = toml::to_string_pretty(&f).context("serialize identity file")?;
        fs::write(path.as_ref(), txt).context("write identity file")?;

        // Anyone who can read this file can sign as this device and open anything
        // sealed to it.
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(path.as_ref(), fs::Permissions::from_mode(0o600))
                .context("restrict identity file permissions")?;
        }
        Ok(())
    }

    pub fn signing_key(&self) -> &SigningKey {
        &self.signing
    }

    pub fn verifying_key(&self) -> VerifyingKey {
        self.signing.verifying_key()
    }

    pub fn pubkey_bytes(&self) -> [u8; 32] {
        self.verifying_key().to_bytes()
    }

    /// The X25519 secret that opens envelopes addressed to this device.
    pub fn sealing_secret(&self) -> [u8; 32] {
        self.sealing.to_bytes()
    }

    /// The X25519 public key to enrol elsewhere, so peers can seal file keys to us.
    pub fn sealing_pubkey(&self) -> [u8; 32] {
        PublicKey::from(&self.sealing).to_bytes()
    }
}

fn decode_key(encoded: &str) -> Result<[u8; 32]> {
    STANDARD
        .decode(encoded.trim().as_bytes())
        .context("decode base64")?
        .try_into()
        .map_err(|_| anyhow::anyhow!("key material must be 32 bytes"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn signing_and_sealing_keys_are_independent() {
        // The property the birational map would not give: knowing one public key says
        // nothing about the other, and neither equals the other by accident.
        let id = Identity::generate();
        assert_ne!(id.pubkey_bytes(), id.sealing_pubkey());
        assert_ne!(id.signing.to_bytes(), id.sealing_secret());
    }

    #[test]
    fn a_seed_produces_the_same_identity_every_time() {
        // The playground reloads replicas from a stored seed and expects the same
        // device back, sealing key included.
        let a = Identity::from_seed([9u8; 32]);
        let b = Identity::from_seed([9u8; 32]);
        assert_eq!(a.pubkey_bytes(), b.pubkey_bytes());
        assert_eq!(a.sealing_pubkey(), b.sealing_pubkey());

        let other = Identity::from_seed([10u8; 32]);
        assert_ne!(a.sealing_pubkey(), other.sealing_pubkey());
    }

    #[test]
    fn an_identity_round_trips_through_a_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("identity.toml");

        let created = Identity::load_or_create(&path).unwrap();
        let reloaded = Identity::load_or_create(&path).unwrap();
        assert_eq!(created.pubkey_bytes(), reloaded.pubkey_bytes());
        assert_eq!(created.sealing_pubkey(), reloaded.sealing_pubkey());
        assert_eq!(created.sealing_secret(), reloaded.sealing_secret());
    }

    #[test]
    fn an_identity_written_before_sealing_gains_a_key_without_changing_device() {
        // The upgrade path. A device that has been signing operations for months must
        // keep the same id — its inode allocation and its pinned key both depend on it —
        // and simply acquire the ability to receive sealed content.
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("identity.toml");

        let old = Identity::generate();
        let legacy = format!(
            "ed25519_seed_b64 = \"{}\"\n",
            STANDARD.encode(old.signing.to_bytes())
        );
        std::fs::write(&path, legacy).unwrap();

        let loaded = Identity::load_or_create(&path).unwrap();
        assert_eq!(
            loaded.pubkey_bytes(),
            old.pubkey_bytes(),
            "the signing identity must not change"
        );

        // Derived, persisted, and stable across a second load.
        let again = Identity::load_or_create(&path).unwrap();
        assert_eq!(loaded.sealing_pubkey(), again.sealing_pubkey());
        assert!(std::fs::read_to_string(&path)
            .unwrap()
            .contains("x25519_secret_b64"));
    }

    #[test]
    fn a_sealed_key_opens_for_its_recipient_and_nobody_else() {
        let alice = Identity::generate();
        let mallory = Identity::generate();
        let file_key = [7u8; 32];

        let env = crate::envelope::seal(alice.sealing_pubkey(), &file_key).unwrap();
        assert_eq!(
            crate::envelope::open(alice.sealing_secret(), &env).unwrap(),
            file_key
        );
        assert!(crate::envelope::open(mallory.sealing_secret(), &env).is_err());
    }
}
