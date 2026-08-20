#![forbid(unsafe_code)]

//! An S3-compatible subset over NexusFS.
//!
//! Scope is deliberately small: object PUT/GET/HEAD/DELETE, bucket create and list,
//! and ListObjectsV2 with prefix, delimiter and pagination. Not implemented, and not
//! planned for v0: SigV4 request signing, multipart upload, versioning, ACLs, CORS,
//! lifecycle rules, and server-side encryption.
//!
//! Every write goes through the same signed-operation pipeline the CLI uses, so
//! objects written here are ordinary files that replication and verification treat
//! no differently.

pub mod routes;
pub mod xml;

use std::net::SocketAddr;
use std::sync::Arc;

use anyhow::Result;
use axum::Router;
use tracing::info;

use nexusfs_core::CoreState;
use nexusfs_crypto::Identity;

#[derive(Clone)]
pub struct S3State {
    pub core: Arc<CoreState>,
    /// Signs the operations that object writes produce.
    pub identity: Arc<Identity>,
    /// Shared secret checked via `x-nexusfs-token`; empty disables the check.
    pub token: String,
    /// Where to get content this node deferred; `None` when replication is not running.
    pub content: Option<std::sync::Arc<dyn nexusfs_core::ContentFetcher>>,
}

pub async fn serve(bind: SocketAddr, st: S3State) -> Result<()> {
    let app: Router = routes::router(st);
    info!("s3 listening on http://{}", bind);
    let listener = tokio::net::TcpListener::bind(bind).await?;
    axum::serve(listener, app).await?;
    Ok(())
}
