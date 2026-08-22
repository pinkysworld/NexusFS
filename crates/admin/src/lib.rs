#![forbid(unsafe_code)]

pub mod assets;
pub mod routes;

use std::net::SocketAddr;
use std::sync::Arc;

use anyhow::Result;
use axum::Router;
use tracing::info;

use nexusfs_core::CoreState;

/// Supplies peer sync status to the admin API.
///
/// A trait object rather than a direct dependency on `nexusfs-net`: the admin crate
/// must keep building when the `quic` feature is off, and the daemon is the only place
/// that knows whether replication is running.
pub trait PeerSource: Send + Sync {
    fn peers(&self) -> Vec<PeerView>;
}

/// One peer as the console renders it.
#[derive(Clone, Debug, Default, serde::Serialize)]
pub struct PeerView {
    pub address: String,
    pub device_id: Option<String>,
    pub last_attempt_ms: u64,
    pub last_success_ms: Option<u64>,
    pub last_error: Option<String>,
    pub ops_received: usize,
    pub blobs_received: usize,
    pub content_bytes: u64,
    /// The last pass stopped short of fetching everything because of the sync budget.
    pub content_deferred: bool,
    pub syncs: u64,
}

/// Supplies the energy reading and the replication budget derived from it.
///
/// Same trait-object arrangement as `PeerSource`, and for the same reason: the console
/// should be able to show why replication is holding back without the admin crate
/// knowing how that decision is made.
pub trait EnergySource: Send + Sync {
    fn energy(&self) -> EnergyView;
}

/// The current power situation and what replication is allowed to do about it.
#[derive(Clone, Debug, Default, serde::Serialize)]
pub struct EnergyView {
    /// False when energy-aware scheduling is switched off in the config.
    pub enabled: bool,
    /// "mains" | "battery" | "unknown".
    pub power: String,
    pub battery_pct: Option<u8>,
    pub temp_c: Option<i16>,
    pub cpu_load: Option<f32>,
    /// "unmetered" | "metered" | "unknown".
    pub link: String,
    /// Free space where the store lives. `None` when it could not be read — which is
    /// not the same as zero, and the console must not render it as such.
    pub storage_free_bytes: Option<u64>,
    pub sampled_unix_ms: u64,
    /// Contact peers at all.
    pub sync: bool,
    /// Transfer content, not just operations.
    pub content: bool,
    /// `None` means uncapped.
    pub max_content_bytes: Option<u64>,
    pub interval_scale: f32,
    pub reason: String,
}

#[derive(Clone)]
pub struct AdminState {
    pub core: Arc<CoreState>,
    pub token: String,
    /// `None` when replication is not compiled in or not running.
    pub peers: Option<Arc<dyn PeerSource>>,
    /// `None` when the daemon is not sampling energy.
    pub energy: Option<Arc<dyn EnergySource>>,
    /// Where to get content this node deferred. `None` when replication is not running,
    /// in which case a missing chunk is simply missing.
    pub content: Option<Arc<dyn nexusfs_core::ContentFetcher>>,
    /// This node's ed25519 public key, so the console can show what to enrol elsewhere.
    ///
    /// Passed in rather than read from an `Identity`, which would make this crate depend
    /// on `nexusfs-crypto` for one field. It is public key material by definition.
    pub node_pubkey: Option<[u8; 32]>,
    /// This node's X25519 sealing key, which a peer needs in order to seal content to
    /// it. Public by construction, same as the signing key beside it.
    pub node_seal_key: Option<[u8; 32]>,
}

pub async fn serve(bind: SocketAddr, state: AdminState) -> Result<()> {
    let app: Router = routes::router(state);

    info!("admin listening on http://{}", bind);
    let listener = tokio::net::TcpListener::bind(bind).await?;
    axum::serve(listener, app).await?;
    Ok(())
}
