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
use nexusfs_proto::{BuildInfo, DeviceId, Msg};

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

/// What a session needs from the node it belongs to.
#[derive(Clone)]
pub struct SessionCtx {
    pub core: CoreState,
    pub identity: Identity,
    pub device_id: DeviceId,
    pub trust: TrustPolicy,
}

impl SessionCtx {
    fn build_info(&self) -> BuildInfo {
        BuildInfo {
            version: env!("CARGO_PKG_VERSION").to_string(),
            git_hash: None,
        }
    }
}

/// What one pull achieved, for logging and the admin surface.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct SyncOutcome {
    pub peer: Option<DeviceId>,
    pub ops_received: usize,
    pub ops_applied: usize,
    pub blobs_received: usize,
    pub still_pending: usize,
}

// --- initiator --------------------------------------------------------------

/// Pull everything this node is missing from the peer on the other end of `stream`.
pub async fn pull_from_peer<S>(stream: &mut S, ctx: &SessionCtx) -> Result<SyncOutcome>
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
                    summary,
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
    }

    // 3. Ask for the content the applied operations turned out to reference.
    loop {
        let wanted = ctx.core.missing_chunk_hashes()?;
        if wanted.is_empty() {
            break;
        }

        send_msg(
            stream,
            &env(
                next_id(),
                None,
                Msg::WantBlobs {
                    hashes: wanted.clone(),
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

        if blobs.is_empty() {
            // The peer cannot supply what we still need; stop rather than spin.
            debug!(
                "peer has none of the {} chunks we are missing",
                wanted.len()
            );
            break;
        }

        for (hash, bytes) in blobs {
            // Content addressing is only a guarantee if it is checked. A peer that
            // returns different bytes for a hash we asked for is caught here.
            let actual = nexusfs_core::hash_bytes(&bytes);
            if actual != hash {
                warn!("peer returned content that does not match the requested hash");
                continue;
            }
            ctx.core.stores.blobs.put(hash, &bytes)?;
            outcome.blobs_received += 1;
        }

        outcome.ops_applied += ctx.core.retry_pending()?;

        if !more {
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
