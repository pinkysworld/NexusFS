use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::fs;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};

/// Expand a leading `~` or `~/` against `$HOME`. Other paths pass through untouched.
fn expand_home(raw: &str) -> PathBuf {
    let Some(rest) = raw.strip_prefix('~') else {
        return PathBuf::from(raw);
    };
    let Ok(home) = std::env::var("HOME") else {
        return PathBuf::from(raw);
    };
    PathBuf::from(home).join(rest.trim_start_matches('/'))
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    pub node: Node,
    pub net: Net,
    pub admin: Admin,
    pub s3: S3,
    pub posix: Posix,
    pub security: Security,
    pub energy: Energy,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Node {
    pub data_dir: String,
    pub device_name: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Net {
    pub listen: String,
    pub peers: Vec<String>,
    pub tofu: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Admin {
    pub bind: String,
    pub token: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct S3 {
    pub enabled: bool,
    pub bind: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Posix {
    pub enabled: bool,
    pub mountpoint: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Security {
    pub encrypt_at_rest: bool,
    pub proof_mode: String, // none|transparent|zk_commit|zk_full
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Energy {
    pub enabled: bool,
    pub battery_low_pct: u8,
    pub temp_high_c: i16,
}

impl Config {
    pub fn load(path: impl AsRef<Path>) -> Result<Self> {
        let txt = fs::read_to_string(path).context("read config")?;
        let cfg: Config = toml::from_str(&txt).context("parse toml config")?;
        Ok(cfg)
    }

    /// Resolved data directory, expanding a leading `~`.
    ///
    /// Expansion matters because the store must be able to live outside a
    /// cloud-synced folder: a sync daemon rewriting files under a live embedded
    /// database is a good way to corrupt it.
    pub fn data_dir(&self) -> PathBuf {
        expand_home(&self.node.data_dir)
    }

    pub fn admin_addr(&self) -> Result<SocketAddr> {
        self.admin
            .bind
            .parse()
            .context("parse admin.bind socket addr")
    }

    #[cfg(feature = "s3")]
    pub fn s3_addr(&self) -> Result<SocketAddr> {
        self.s3.bind.parse().context("parse s3.bind socket addr")
    }

    /// Listen address for the replication transport.
    ///
    /// Unused until the peer manager lands (see the `quic` TODO in `daemon.rs`); kept
    /// so the config surface and the transport arrive together rather than the address
    /// parsing being written twice.
    #[cfg(feature = "quic")]
    #[allow(dead_code)]
    pub fn net_addr(&self) -> Result<SocketAddr> {
        self.net
            .listen
            .parse()
            .context("parse net.listen socket addr")
    }
}
