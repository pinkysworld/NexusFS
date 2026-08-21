use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::fs;
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
    /// Trust an unknown device's key the first time it connects.
    pub tofu: bool,
    /// Seconds between pulls from each configured peer.
    #[serde(default = "default_sync_interval")]
    pub sync_interval_secs: u64,
}

fn default_sync_interval() -> u64 {
    15
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
    /// Shared secret required in `x-nexusfs-token`. Empty disables the check.
    ///
    /// Defaulted so existing configs keep parsing; the daemon warns when it is unset.
    #[serde(default)]
    pub token: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Posix {
    pub enabled: bool,
    pub mountpoint: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Security {
    /// Encrypt chunk content before it is written to disk.
    pub encrypt_at_rest: bool,
    /// `none`, `transparent`, or `required` (transparent, and reject unproven ops).
    /// `zk_commit` and `zk_full` are accepted but not implemented, and behave as
    /// `none` rather than silently pretending to prove anything.
    pub proof_mode: String,
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

    #[cfg(feature = "admin")]
    pub fn admin_addr(&self) -> Result<std::net::SocketAddr> {
        self.admin
            .bind
            .parse()
            .context("parse admin.bind socket addr")
    }

    #[cfg(feature = "s3")]
    pub fn s3_addr(&self) -> Result<std::net::SocketAddr> {
        self.s3.bind.parse().context("parse s3.bind socket addr")
    }

    /// Listen address for the replication transport.
    #[cfg(feature = "quic")]
    #[cfg(feature = "quic")]
    pub fn net_addr(&self) -> Result<std::net::SocketAddr> {
        self.net
            .listen
            .parse()
            .context("parse net.listen socket addr")
    }
}
