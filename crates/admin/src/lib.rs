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
    pub syncs: u64,
}

#[derive(Clone)]
pub struct AdminState {
    pub core: Arc<CoreState>,
    pub token: String,
    /// `None` when replication is not compiled in or not running.
    pub peers: Option<Arc<dyn PeerSource>>,
}

pub async fn serve(bind: SocketAddr, state: AdminState) -> Result<()> {
    let app: Router = routes::router(state);

    info!("admin listening on http://{}", bind);
    let listener = tokio::net::TcpListener::bind(bind).await?;
    axum::serve(listener, app).await?;
    Ok(())
}
