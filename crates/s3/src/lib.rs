#![forbid(unsafe_code)]

pub mod routes;

use std::net::SocketAddr;
use std::sync::Arc;

use anyhow::Result;
use axum::Router;
use tracing::info;

use nexusfs_core::CoreState;

#[derive(Clone)]
pub struct S3State {
    pub core: Arc<CoreState>,
}

pub async fn serve(bind: SocketAddr, st: S3State) -> Result<()> {
    let app: Router = routes::router(st);
    info!("s3 listening on http://{}", bind);
    let listener = tokio::net::TcpListener::bind(bind).await?;
    axum::serve(listener, app).await?;
    Ok(())
}
