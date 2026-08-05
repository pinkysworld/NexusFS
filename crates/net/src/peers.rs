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
use crate::session::{
    pull_from_peer, serve_session, SessionCtx, SyncGate, SyncLimits, SyncOutcome,
};

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
    pub content_bytes: u64,
    /// The last pass left content on the table because of the sync budget. Distinct
    /// from "up to date": the namespace is current but some bytes are still remote.
    pub content_deferred: bool,
    pub syncs: u64,
}

/// Shared, cheap to clone, readable by the admin API while syncs are in flight.
#[derive(Clone, Default)]
pub struct PeerRegistry {
    inner: Arc<Mutex<BTreeMap<String, PeerStatus>>>,
    /// Set while the gate is holding replication back, cleared on the next live pass.
    /// Kept off `PeerStatus` because a pause is a property of this node, not of any
    /// particular peer — attributing it to each peer would read as a peer failure.
    paused: Arc<Mutex<Option<String>>>,
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

    /// Why replication is currently paused, if it is.
    pub fn paused_reason(&self) -> Option<String> {
        self.paused.lock().expect("peer registry poisoned").clone()
    }

    fn record_skip(&self, reason: &str) {
        *self.paused.lock().expect("peer registry poisoned") = Some(reason.to_string());
    }

    fn clear_skip(&self) {
        *self.paused.lock().expect("peer registry poisoned") = None;
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

/// Pull once from a single peer, transferring at most what `limits` allows.
pub async fn sync_once(
    endpoint: &Endpoint,
    addr: SocketAddr,
    ctx: &SessionCtx,
    limits: SyncLimits,
) -> Result<SyncOutcome> {
    let connection = quic::connect(endpoint, addr).await?;
    let (send, recv) = connection
        .open_bi()
        .await
        .context("open replication stream")?;

    let mut stream = join(recv, send);
    let outcome = pull_from_peer(&mut stream, ctx, limits).await;

    connection.close(0u32.into(), b"done");
    outcome
}

/// Periodically pull from every configured peer.
///
/// Failures are recorded and retried on the next tick rather than aborting: a peer
/// being unreachable is the normal case for an offline-first system, not an error
/// worth tearing anything down over.
///
/// `gate` decides how much each pass may transfer. `None` means unconstrained.
pub async fn sync_loop(
    endpoint: Endpoint,
    peers: Vec<String>,
    ctx: SessionCtx,
    interval: Duration,
    registry: PeerRegistry,
    gate: Option<Arc<dyn SyncGate>>,
) {
    if peers.is_empty() {
        debug!("no peers configured; replication will only serve inbound sessions");
        return;
    }

    loop {
        // Slept rather than ticked, because the gate can stretch the period and a fixed
        // `interval` would fire immediately afterwards to catch up on the ticks it
        // thinks it missed — exactly the burst the throttle exists to prevent.
        let decision = gate.as_ref().map(|g| g.decide()).unwrap_or_default();
        let scale = decision.interval_scale.max(0.1);
        tokio::time::sleep(interval.mul_f32(scale)).await;

        if !decision.sync {
            debug!(reason = %decision.reason, "skipping sync pass");
            registry.record_skip(&decision.reason);
            continue;
        }

        registry.clear_skip();

        if !decision.limits.content {
            debug!(reason = %decision.reason, "syncing operations only");
        }

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

            match sync_once(&endpoint, addr, &ctx, decision.limits).await {
                Ok(outcome) => {
                    registry.record(peer, |s| {
                        s.last_success_ms = Some(now);
                        s.last_error = None;
                        s.syncs += 1;
                        s.ops_received += outcome.ops_received;
                        s.blobs_received += outcome.blobs_received;
                        s.content_bytes += outcome.content_bytes;
                        s.content_deferred = outcome.content_deferred;
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
