use anyhow::{Context, Result};
use base64::{engine::general_purpose::STANDARD, Engine as _};
use ed25519_dalek::{SigningKey, VerifyingKey};
use rand::rngs::OsRng;
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::Path;

#[derive(Debug, Clone)]
pub struct Identity {
    signing: SigningKey,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct IdentityFile {
    // base64 of 32-byte ed25519 secret key (seed)
    ed25519_seed_b64: String,
}

impl Identity {
    pub fn generate() -> Self {
        let signing = SigningKey::generate(&mut OsRng);
        Self { signing }
    }

    pub fn load_or_create(path: impl AsRef<Path>) -> Result<Self> {
        let path = path.as_ref();
        if path.exists() {
            let txt = fs::read_to_string(path).context("read identity file")?;
            let f: IdentityFile = toml::from_str(&txt).context("parse identity file toml")?;
            let seed = STANDARD
                .decode(f.ed25519_seed_b64.as_bytes())
                .context("decode base64 seed")?;
            let seed: [u8; 32] = seed
                .try_into()
                .map_err(|_| anyhow::anyhow!("invalid ed25519 seed length"))?;
            let signing = SigningKey::from_bytes(&seed);
            Ok(Self { signing })
        } else {
            if let Some(parent) = path.parent() {
                fs::create_dir_all(parent).ok();
            }
            let id = Self::generate();
            id.save(path)?;
            Ok(id)
        }
    }

    pub fn save(&self, path: impl AsRef<Path>) -> Result<()> {
        let seed: [u8; 32] = self.signing.to_bytes();
        let f = IdentityFile {
            ed25519_seed_b64: STANDARD.encode(seed),
        };
        let txt = toml::to_string_pretty(&f).context("serialize identity file")?;
        fs::write(path, txt).context("write identity file")?;
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
}
