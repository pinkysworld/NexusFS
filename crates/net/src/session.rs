//! The replication session.
//!
//! Deliberately pull-based and one-directional: the initiator asks a peer for what it
//! is missing, and nothing is pushed. Two nodes converge by each pulling from the
//! other, which means a session has one clear owner of the loop and no negotiation
//! about who sends next. Duplex protocols are where sync implementations usually go
//! wrong.
//!
//! Written over `AsyncRead + AsyncWrite` rather than over QUIC directly, so the
//! protocol can be tested against real `CoreState`s over an in-memory pipe without
//! binding sockets or generating certificates.
//!
//! Order matters. Operations are transferred before content, because an operation is
//! small and self-describing while a chunk is only meaningful once something refers to
//! it. A write whose chunks have not arrived parks rather than applying, so the second
//! phase asks for exactly the content the first phase turned out to need.

use anyhow::{bail, Context, Result};
use tokio::io::{AsyncRead, AsyncWrite};
use tracing::{debug, info, warn};

use nexusfs_core::CoreState;
use nexusfs_crypto::Identity;
use nexusfs_proto::{BuildInfo, DeviceId, Hash, Msg};

use crate::codec::{recv_msg, send_msg};
use crate::replication::{
    env, hello_nonce, make_hello, make_hello_ack, verify_hello, verify_hello_ack, MAX_FRAME_LEN,
    PROTOCOL_VERSION,
};
use crate::trust::{authorize, TrustPolicy};

/// Operations per batch. Bounded so a large backlog streams rather than arriving as
/// one frame that would blow the frame limit.
const OPS_BATCH: usize = 256;

/// Content bytes per batch, kept well under `MAX_FRAME_LEN`.
const BLOB_BATCH_BYTES: u64 = 4 * 1024 * 1024;

/// Total ops accepted in one session, so a peer cannot stream forever.
const MAX_OPS_PER_SESSION: usize = 100_000;

/// How much this pass is allowed to transfer.
///
/// Operations always flow — they are small, and keeping the namespace converged is
/// what makes a constrained device still useful. Only content is rationed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SyncLimits {
    /// Request chunk content at all.
    pub content: bool,
    /// Ceiling on content bytes accepted this pass.
    pub max_content_bytes: u64,
}

impl SyncLimits {
    pub fn unlimited() -> Self {
        Self {
            content: true,
            max_content_bytes: u64::MAX,
        }
    }

    /// Take operations, defer every byte of content.
    pub fn metadata_only() -> Self {
        Self {
            content: false,
            max_content_bytes: 0,
        }
    }
}

impl Default for SyncLimits {
    fn default() -> Self {
        Self::unlimited()
    }
}

/// What the sync loop should do on this tick.
#[derive(Debug, Clone, PartialEq)]
pub struct SyncDecision {
    /// Contact peers at all this tick.
    pub sync: bool,
    pub limits: SyncLimits,
    /// Multiplier applied to the configured poll interval before the next tick.
    pub interval_scale: f32,
    /// Human-readable justification, surfaced in logs and the admin console.
    pub reason: String,
}

impl Default for SyncDecision {
    fn default() -> Self {
        Self {
            sync: true,
            limits: SyncLimits::unlimited(),
            interval_scale: 1.0,
            reason: "unconstrained".into(),
        }
    }
}

/// Where the sync loop asks permission before each pass.
///
/// A trait object rather than a dependency on `nexusfs-energy`, for the same reason
/// `admin::PeerSource` exists: `net` should carry the mechanism and not the policy. The
/// daemon is the only place that knows whether energy-aware scheduling is switched on,
/// and it injects the decision from there.
pub trait SyncGate: Send + Sync {
    fn decide(&self) -> SyncDecision;
}

/// What a session needs from the node it belongs to.
#[derive(Clone)]
pub struct SessionCtx {
    pub core: CoreState,
    pub identity: Identity,
    pub device_id: DeviceId,
    pub trust: TrustPolicy,
    /// Woken when a peer notifies that it holds operations this node lacks.
    ///
    /// A bare signal rather than a channel carrying the peer's identity: the sync loop
    /// pulls from every configured peer anyway, and a notification is a hint about
    /// *timing*, not about who to talk to. `None` on a node with no sync loop to wake —
    /// every test that drives a session directly, and any embedder that pulls on its
    /// own schedule.
    pub wake: Option<std::sync::Arc<tokio::sync::Notify>>,
}

impl SessionCtx {
    fn build_info(&self) -> BuildInfo {
        BuildInfo {
            version: env!("CARGO_PKG_VERSION").to_string(),
            git_hash: None,
        }
    }
}

/// Tell one peer that this node holds operations it may lack.
///
/// The full handshake for one tiny message, which is not as wasteful as it looks: the
/// alternative to authenticating is letting anyone who can reach the port make this node
/// start work. It returns whether the peer said it was behind, which nothing acts on —
/// only diagnosis cares, and "heard you, already current" is a very different thing to
/// be looking at than "never heard you".
pub async fn notify_peer<S>(
    stream: &mut S,
    ctx: &SessionCtx,
    summary: nexusfs_proto::ClockSummary,
) -> Result<bool>
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    let hello = make_hello(
        &ctx.identity,
        ctx.device_id,
        vec!["ops".into(), "blobs".into()],
        ctx.build_info(),
        nexusfs_core::now_ms(),
    )?;
    let nonce = hello_nonce(&hello)?;
    send_msg(stream, &env(1, None, hello)).await?;

    let ack = recv_msg(stream, MAX_FRAME_LEN).await?;
    if let Msg::Error { code, message, .. } = &ack.payload {
        bail!("peer error {code}: {message}");
    }
    let peer_info = verify_hello_ack(&ack.payload, &nonce)?;
    // Authorised even though nothing is being accepted from them: telling an unknown
    // device our clock summary leaks which devices we have heard from and how far along
    // they are, which is not much and is not nothing.
    authorize(&ctx.core, ctx.trust, peer_info.device_id, &peer_info.pubkey)?;

    send_msg(stream, &env(2, None, Msg::Notify { summary })).await?;
    let reply = recv_msg(stream, MAX_FRAME_LEN).await?;
    let behind = match reply.payload {
        Msg::Noted { behind } => behind,
        Msg::Error { code, message, .. } => bail!("peer error {code}: {message}"),
        other => bail!("unexpected reply to a notification: {other:?}"),
    };

    send_msg(stream, &env(3, None, Msg::Bye)).await.ok();
    Ok(behind)
}

/// What one pull achieved, for logging and the admin surface.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct SyncOutcome {
    pub peer: Option<DeviceId>,
    pub ops_received: usize,
    pub ops_applied: usize,
    pub blobs_received: usize,
    pub content_bytes: u64,
    pub still_pending: usize,
    /// True when content was left on the table because of the budget rather than
    /// because the peer had nothing more.
    pub content_deferred: bool,
}

// --- initiator --------------------------------------------------------------

/// Pull everything this node is missing, subject to `limits`.
pub async fn pull_from_peer<S>(
    stream: &mut S,
    ctx: &SessionCtx,
    limits: SyncLimits,
) -> Result<SyncOutcome>
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    let mut outcome = SyncOutcome::default();
    let mut msg_id = 0u64;
    let mut next_id = || {
        msg_id += 1;
        msg_id
    };

    // 1. Identify ourselves, then establish who answered before trusting anything.
    let hello = make_hello(
        &ctx.identity,
        ctx.device_id,
        vec!["ops".into(), "blobs".into()],
        ctx.build_info(),
        nexusfs_core::now_ms(),
    )?;
    let nonce = hello_nonce(&hello)?;
    send_msg(stream, &env(next_id(), None, hello)).await?;

    let ack = recv_msg(stream, MAX_FRAME_LEN).await?;
    if let Msg::Error { code, message, .. } = &ack.payload {
        bail!("peer error {code}: {message}");
    }

    // Verifies the responder's signature over our nonce, so the identity we are about
    // to pin is proven rather than merely claimed.
    let peer_info = verify_hello_ack(&ack.payload, &nonce)?;
    authorize(&ctx.core, ctx.trust, peer_info.device_id, &peer_info.pubkey)?;
    let peer = peer_info.device_id;
    outcome.peer = Some(peer);

    // 2. Tell the peer what we already hold; it replies with what we lack.
    loop {
        let summary = ctx.core.clock_summary()?;
        send_msg(
            stream,
            &env(
                next_id(),
                None,
                Msg::Have {
                    summary: summary.clone(),
                    head: None,
                },
            ),
        )
        .await?;

        let reply = recv_msg(stream, MAX_FRAME_LEN).await?;
        let (ops, more) = match reply.payload {
            Msg::OpsBatch { ops, more } => (ops, more),
            Msg::Error { code, message, .. } => bail!("peer error {code}: {message}"),
            other => bail!("expected OpsBatch, got {other:?}"),
        };

        if ops.is_empty() && !more {
            break;
        }

        outcome.ops_received += ops.len();
        if outcome.ops_received > MAX_OPS_PER_SESSION {
            bail!("peer sent more than {MAX_OPS_PER_SESSION} operations in one session");
        }

        // apply_op verifies the signature before anything touches state, so a peer
        // cannot inject operations it did not author.
        for op in ops {
            match ctx.core.apply_op(&op) {
                Ok(nexusfs_core::ApplyOutcome::Applied) => outcome.ops_applied += 1,
                Ok(_) => {}
                Err(e) => {
                    // One bad operation must not abort a whole sync, but it also must
                    // not be silently dropped.
                    warn!(error = %e, "rejected an operation from peer");
                }
            }
        }

        if !more {
            break;
        }

        // The peer answers relative to our clock summary, which only advances on the
        // highest *contiguous* counter per device. An operation we refuse never enters
        // the log, so a single unverifiable one low in a device's history pins the
        // summary below it — and every subsequent round asks for, and is served, the
        // identical window. Without this the session spins until `MAX_OPS_PER_SESSION`
        // trips, on every pass, forever.
        //
        // Comparing summaries is the exact test: any genuinely new operation advances
        // one, because the summary counts what has been *received*, not what applied.
        if ctx.core.clock_summary()? == summary {
            warn!(
                peer = %format!("{:x}", peer.0),
                "peer offered a batch that made no progress; ending the operation phase"
            );
            break;
        }
    }

    // 3. Ask for the content the applied operations turned out to reference — unless
    // the budget says to defer it. Operations that already arrived stay parked and
    // apply on a later pass, so deferring content costs nothing but freshness.
    loop {
        let wanted = ctx.core.missing_chunk_hashes()?;
        if wanted.is_empty() {
            break;
        }
        if !limits.content {
            outcome.content_deferred = true;
            debug!(
                chunks = wanted.len(),
                "content transfer deferred by the current budget"
            );
            break;
        }
        if outcome.content_bytes >= limits.max_content_bytes {
            outcome.content_deferred = true;
            debug!(
                bytes = outcome.content_bytes,
                "content budget for this pass is spent"
            );
            break;
        }

        send_msg(
            stream,
            &env(
                next_id(),
                None,
                Msg::WantBlobs {
                    hashes: wanted.clone(),
                    max_bytes: BLOB_BATCH_BYTES.min(
                        limits
                            .max_content_bytes
                            .saturating_sub(outcome.content_bytes),
                    ),
                },
            ),
        )
        .await?;

        let reply = recv_msg(stream, MAX_FRAME_LEN).await?;
        let (blobs, more) = match reply.payload {
            Msg::BlobsBatch { blobs, more } => (blobs, more),
            Msg::Error { code, message, .. } => bail!("peer error {code}: {message}"),
            other => bail!("expected BlobsBatch, got {other:?}"),
        };

        if blobs.is_empty() {
            // The peer cannot supply what we still need; stop rather than spin.
            debug!(
                "peer has none of the {} chunks we are missing",
                wanted.len()
            );
            break;
        }

        let before = outcome.blobs_received;
        for (hash, bytes) in blobs {
            // Content we did not ask for is refused outright. Verifying the hash proves
            // the bytes match their name, not that we wanted them — without this a peer
            // can write whatever it likes into our store, and because each unsolicited
            // blob still counts as progress the no-progress guard never fires.
            if !wanted.contains(&hash) {
                warn!("peer returned content that was not requested");
                continue;
            }
            // Content addressing is only a guarantee if it is checked. A peer that
            // returns different bytes for a hash we asked for is caught here.
            let actual = nexusfs_core::hash_bytes(&bytes);
            if actual != hash {
                warn!("peer returned content that does not match the requested hash");
                continue;
            }
            outcome.content_bytes += bytes.len() as u64;
            ctx.core.stores.blobs.put(hash, &bytes)?;
            outcome.blobs_received += 1;
        }

        outcome.ops_applied += ctx.core.retry_pending()?;

        if !more {
            break;
        }

        // Same reasoning as the operation phase. The wanted set only shrinks when a
        // blob is stored, so a round that stores nothing while claiming there is more
        // would repeat the identical request — which is what a peer serving content
        // that fails its hash check produces.
        if outcome.blobs_received == before {
            warn!(
                peer = %format!("{:x}", peer.0),
                "peer returned no usable content while reporting more; ending the \
                 content phase"
            );
            break;
        }
    }

    send_msg(stream, &env(next_id(), None, Msg::Bye)).await.ok();

    outcome.still_pending = ctx.core.pending_count()?;
    info!(
        peer = %format!("{:x}", peer.0),
        ops = outcome.ops_received,
        applied = outcome.ops_applied,
        blobs = outcome.blobs_received,
        bytes = outcome.content_bytes,
        deferred = outcome.content_deferred,
        pending = outcome.still_pending,
        "pull complete"
    );
    Ok(outcome)
}

// --- responder --------------------------------------------------------------

/// Serve one inbound session: answer the handshake, then satisfy requests until the
/// peer says it is done.
pub async fn serve_session<S>(stream: &mut S, ctx: &SessionCtx) -> Result<Option<DeviceId>>
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    let opening = recv_msg(stream, MAX_FRAME_LEN).await?;

    // The nonce has to come out of the Hello before anything else: every refusal is
    // still signed over it, so the initiator can tell a genuine rejection from an
    // injected one.
    let nonce = hello_nonce(&opening.payload).unwrap_or([0u8; 32]);

    let refuse = |reason: String| -> Result<Msg> {
        make_hello_ack(
            &ctx.identity,
            ctx.device_id,
            false,
            Some(reason),
            vec![],
            nonce,
        )
    };

    if opening.protocol_version != PROTOCOL_VERSION {
        let message = format!(
            "protocol version {} is not supported",
            opening.protocol_version
        );
        send_msg(
            stream,
            &env(1, Some(opening.msg_id), refuse(message.clone())?),
        )
        .await?;
        bail!("{message}");
    }

    let peer_hello = verify_hello(&opening.payload).context("verify peer Hello")?;

    if let Err(e) = authorize(
        &ctx.core,
        ctx.trust,
        peer_hello.device_id,
        &peer_hello.pubkey,
    ) {
        send_msg(
            stream,
            &env(1, Some(opening.msg_id), refuse(e.to_string())?),
        )
        .await?;
        return Err(e);
    }

    let peer = peer_hello.device_id;
    let ack = make_hello_ack(
        &ctx.identity,
        ctx.device_id,
        true,
        None,
        vec!["ops".into(), "blobs".into()],
        nonce,
    )?;
    send_msg(stream, &env(1, Some(opening.msg_id), ack)).await?;

    let mut msg_id = 1u64;
    loop {
        let request = match recv_msg(stream, MAX_FRAME_LEN).await {
            Ok(r) => r,
            // A peer that drops the connection after it has what it needs is normal.
            Err(_) => break,
        };
        msg_id += 1;

        match request.payload {
            Msg::Have { summary, .. } => {
                let ops = ctx.core.ops_missing_for(&summary, OPS_BATCH)?;
                // `more` tells the initiator to ask again with an updated summary.
                let more = ops.len() == OPS_BATCH;
                send_msg(
                    stream,
                    &env(msg_id, Some(request.msg_id), Msg::OpsBatch { ops, more }),
                )
                .await?;
            }

            Msg::WantBlobs { hashes, max_bytes } => {
                let mut blobs = Vec::new();
                let mut bytes = 0u64;
                let mut more = false;

                for hash in hashes {
                    if bytes >= max_bytes.min(BLOB_BATCH_BYTES) {
                        more = true;
                        break;
                    }
                    if let Some(data) = ctx.core.stores.blobs.get(&hash)? {
                        bytes += data.len() as u64;
                        blobs.push((hash, data));
                    }
                }

                send_msg(
                    stream,
                    &env(
                        msg_id,
                        Some(request.msg_id),
                        Msg::BlobsBatch { blobs, more },
                    ),
                )
                .await?;
            }

            Msg::Notify { summary } => {
                // Answered from the summary alone. A notification provokes a pull; it
                // never delivers one, so nothing here is trusted beyond the handshake
                // that already authenticated this peer — and the pull it triggers
                // verifies every operation exactly as a scheduled pass would.
                let behind = ctx.core.is_behind(&summary).unwrap_or(false);
                if behind {
                    if let Some(wake) = &ctx.wake {
                        debug!("peer reports operations we lack; waking the sync loop");
                        wake.notify_waiters();
                    }
                }
                send_msg(
                    stream,
                    &env(msg_id, Some(request.msg_id), Msg::Noted { behind }),
                )
                .await?;
            }

            Msg::Ping => {
                send_msg(stream, &env(msg_id, Some(request.msg_id), Msg::Pong)).await?;
            }

            Msg::Bye => break,

            other => {
                send_msg(
                    stream,
                    &env(
                        msg_id,
                        Some(request.msg_id),
                        Msg::Error {
                            code: 400,
                            message: format!("unexpected message: {other:?}"),
                            retry_after_ms: None,
                        },
                    ),
                )
                .await?;
            }
        }
    }

    Ok(Some(peer))
}

/// Ask one peer for specific chunks, and nothing else.
///
/// Distinct from a sync pass: no operations are exchanged and no clock summaries move.
/// This exists for a node that already agreed on *what* the filesystem contains and
/// deferred the bytes — the reason the energy budget separates the two in the first
/// place. A read of such a file can go and get exactly what it needs instead of waiting
/// for the next unconstrained pass.
///
/// Returns how many chunks were stored. Content is hash-checked before it is kept, the
/// same as in a sync pass, so a peer cannot slip in different bytes for a hash we asked
/// for.
pub async fn fetch_chunks<S>(stream: &mut S, ctx: &SessionCtx, wanted: &[Hash]) -> Result<usize>
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    if wanted.is_empty() {
        return Ok(0);
    }

    let mut msg_id = 0u64;
    let mut next_id = || {
        msg_id += 1;
        msg_id
    };

    let hello = make_hello(
        &ctx.identity,
        ctx.device_id,
        vec!["blobs".into()],
        ctx.build_info(),
        nexusfs_core::now_ms(),
    )?;
    let nonce = hello_nonce(&hello)?;
    send_msg(stream, &env(next_id(), None, hello)).await?;

    let ack = recv_msg(stream, MAX_FRAME_LEN).await?;
    if let Msg::Error { code, message, .. } = &ack.payload {
        bail!("peer error {code}: {message}");
    }
    let peer_info = verify_hello_ack(&ack.payload, &nonce)?;
    authorize(&ctx.core, ctx.trust, peer_info.device_id, &peer_info.pubkey)?;

    let mut stored = 0usize;
    let mut outstanding: Vec<Hash> = wanted.to_vec();

    while !outstanding.is_empty() {
        send_msg(
            stream,
            &env(
                next_id(),
                None,
                Msg::WantBlobs {
                    hashes: outstanding.clone(),
                    max_bytes: BLOB_BATCH_BYTES,
                },
            ),
        )
        .await?;

        let reply = recv_msg(stream, MAX_FRAME_LEN).await?;
        let (blobs, more) = match reply.payload {
            Msg::BlobsBatch { blobs, more } => (blobs, more),
            Msg::Error { code, message, .. } => bail!("peer error {code}: {message}"),
            other => bail!("expected BlobsBatch, got {other:?}"),
        };

        let before = stored;
        for (hash, bytes) in blobs {
            // Asked-for first: a hash that checks out is still not content we wanted,
            // and counting it as progress would let a peer loop us while filling the
            // store with whatever it chose to send.
            if !outstanding.contains(&hash) {
                warn!("peer returned content that was not requested");
                continue;
            }
            if nexusfs_core::hash_bytes(&bytes) != hash {
                warn!("peer returned content that does not match the requested hash");
                continue;
            }
            ctx.core.stores.blobs.put(hash, &bytes)?;
            outstanding.retain(|h| *h != hash);
            stored += 1;
        }

        // Same no-progress guard as a sync pass: a peer that keeps promising more while
        // handing back nothing usable would otherwise loop on an unchanged request.
        if stored == before {
            break;
        }
        if !more {
            break;
        }
    }

    // Content arriving can unblock operations that were parked waiting for it, and the
    // durability point has to sit after that work rather than before it.
    ctx.core.retry_pending()?;
    ctx.core.flush()?;

    send_msg(stream, &env(next_id(), None, Msg::Bye)).await.ok();
    debug!(stored, wanted = wanted.len(), "on-demand fetch complete");
    Ok(stored)
}
