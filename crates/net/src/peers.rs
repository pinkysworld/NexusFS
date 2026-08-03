//! Peer manager: accepts inbound sessions and periodically pulls from configured peers.

use std::collections::BTreeMap;
use std::net::SocketAddr;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use anyhow::{Context, Result};
use quinn::Endpoint;
use tokio::io::join;
use tracing::{debug, info, warn};

use crate::quic;
use crate::session::{pull_from_peer, serve_session, SessionCtx, SyncOutcome};

/// How a configured peer is doing, for the admin surface.
#[derive(Debug, Clone, Default)]
pub struct PeerStatus {
    pub address: String,
    pub device_id: Option<String>,
    pub last_attempt_ms: u64,
    pub last_success_ms: Option<u64>,
    pub last_error: Option<String>,
    pub ops_received: usize,
    pub blobs_received: usize,
    pub syncs: u64,
}

/// Shared, cheap to clone, readable by the admin API while syncs are in flight.
#[derive(Clone, Default)]
pub struct PeerRegistry {
    inner: Arc<Mutex<BTreeMap<String, PeerStatus>>>,
}

impl PeerRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn snapshot(&self) -> Vec<PeerStatus> {
        self.inner
            .lock()
            .expect("peer registry poisoned")
            .values()
            .cloned()
            .collect()
    }

    fn record(&self, address: &str, update: impl FnOnce(&mut PeerStatus)) {
        let mut map = self.inner.lock().expect("peer registry poisoned");
        let entry = map
            .entry(address.to_string())
            .or_insert_with(|| PeerStatus {
                address: address.to_string(),
                ..Default::default()
            });
        update(entry);
    }
}

/// Serve inbound replication sessions until the endpoint closes.
pub async fn accept_loop(endpoint: Endpoint, ctx: SessionCtx) {
    info!(addr = ?endpoint.local_addr().ok(), "replication listening");

    while let Some(incoming) = endpoint.accept().await {
        let ctx = ctx.clone();
        tokio::spawn(async move {
            let connection = match incoming.await {
                Ok(c) => c,
                Err(e) => {
                    debug!(error = %e, "inbound connection failed to establish");
                    return;
                }
            };

            // One bidirectional stream per session keeps framing simple. The loop ends
            // when the peer closes the connection, which is the normal exit.
            while let Ok((send, recv)) = connection.accept_bi().await {
                let ctx = ctx.clone();
                tokio::spawn(async move {
                    let mut stream = join(recv, send);
                    if let Err(e) = serve_session(&mut stream, &ctx).await {
                        warn!(error = %e, "replication session ended with an error");
                    }
                });
            }
        });
    }
}

/// Pull once from a single peer.
pub async fn sync_once(
    endpoint: &Endpoint,
    addr: SocketAddr,
    ctx: &SessionCtx,
) -> Result<SyncOutcome> {
    let connection = quic::connect(endpoint, addr).await?;
    let (send, recv) = connection
        .open_bi()
        .await
        .context("open replication stream")?;

    let mut stream = join(recv, send);
    let outcome = pull_from_peer(&mut stream, ctx).await;

    connection.close(0u32.into(), b"done");
    outcome
}

/// Periodically pull from every configured peer.
///
/// Failures are recorded and retried on the next tick rather than aborting: a peer
/// being unreachable is the normal case for an offline-first system, not an error
/// worth tearing anything down over.
pub async fn sync_loop(
    endpoint: Endpoint,
    peers: Vec<String>,
    ctx: SessionCtx,
    interval: Duration,
    registry: PeerRegistry,
) {
    if peers.is_empty() {
        debug!("no peers configured; replication will only serve inbound sessions");
        return;
    }

    let mut ticker = tokio::time::interval(interval);
    loop {
        ticker.tick().await;

        for peer in &peers {
            let Ok(addr) = peer.parse::<SocketAddr>() else {
                registry.record(peer, |s| {
                    s.last_error = Some("not a valid socket address".into());
                });
                warn!(peer = %peer, "skipping peer: not a valid socket address");
                continue;
            };

            let now = nexusfs_core::now_ms();
            registry.record(peer, |s| {
                s.last_attempt_ms = now;
            });

            match sync_once(&endpoint, addr, &ctx).await {
                Ok(outcome) => {
                    registry.record(peer, |s| {
                        s.last_success_ms = Some(now);
                        s.last_error = None;
                        s.syncs += 1;
                        s.ops_received += outcome.ops_received;
                        s.blobs_received += outcome.blobs_received;
                        s.device_id = outcome.peer.map(|d| format!("{:x}", d.0));
                    });
                }
                Err(e) => {
                    registry.record(peer, |s| {
                        s.last_error = Some(format!("{e:#}"));
                    });
                    debug!(peer = %peer, error = %e, "sync attempt failed");
                }
            }
        }
    }
}
