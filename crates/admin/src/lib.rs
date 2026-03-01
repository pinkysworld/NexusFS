#![forbid(unsafe_code)]

pub mod assets;
pub mod routes;

use std::net::SocketAddr;
use std::sync::Arc;

use anyhow::Result;
use axum::Router;
use tracing::info;

use nexusfs_core::CoreState;

#[derive(Clone)]
pub struct AdminState {
    pub core: Arc<CoreState>,
    pub token: String,
}

pub async fn serve(bind: SocketAddr, state: AdminState) -> Result<()> {
    let app: Router = routes::router(state);

    info!("admin listening on http://{}", bind);
    let listener = tokio::net::TcpListener::bind(bind).await?;
    axum::serve(listener, app).await?;
    Ok(())
}
