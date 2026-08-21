//! Replication tests.
//!
//! Two real `CoreState`s sync over an in-memory duplex, so these exercise the actual
//! protocol — handshake, trust, batching, verification — without sockets or
//! certificates. What is *not* covered here is QUIC itself.

use std::sync::Arc;

use nexusfs_core::{now_ms, CoreState, Stores};
use nexusfs_crypto::Identity;
use nexusfs_net::session::{pull_from_peer, serve_session, SessionCtx, SyncLimits};
use nexusfs_net::trust::TrustPolicy;
use nexusfs_proto::DeviceId;
use nexusfs_storage::mem_store::MemStore;

struct Node {
    ctx: SessionCtx,
}

fn node(device: u128, seed: u8, tofu: bool) -> Node {
    let store = MemStore::new();
    let core = CoreState::new(Stores::shared(store), DeviceId(device));
    core.bootstrap_if_needed().unwrap();

    Node {
        ctx: SessionCtx {
            core,
            identity: Identity::from_seed([seed; 32]),
            device_id: DeviceId(device),
            trust: TrustPolicy { tofu },
        },
    }
}

impl Node {
    fn write(&self, path: &str, content: &str) {
        self.ctx
            .core
            .write_file(&self.ctx.identity, path, content.as_bytes(), now_ms())
            .unwrap();
    }

    fn mkdir(&self, path: &str) {
        self.ctx
            .core
            .mkdir_p(&self.ctx.identity, path, now_ms())
            .unwrap();
    }

    fn state_root(&self) -> [u8; 32] {
        self.ctx.core.compute_state_root().unwrap()
    }

    fn read(&self, path: &str) -> String {
        String::from_utf8(self.ctx.core.read_file_path(path).unwrap()).unwrap()
    }

    fn listing(&self, path: &str) -> Vec<String> {
        self.ctx
            .core
            .read_dir_path(path)
            .unwrap()
            .into_iter()
            .map(|e| e.name)
            .collect()
    }
}

/// Run one pull of `puller` from `source` over an in-memory connection.
async fn pull(puller: &Node, source: &Node) -> anyhow::Result<nexusfs_net::session::SyncOutcome> {
    pull_with(puller, source, SyncLimits::unlimited()).await
}

/// As `pull`, but under an explicit budget.
async fn pull_with(
    puller: &Node,
    source: &Node,
    limits: SyncLimits,
) -> anyhow::Result<nexusfs_net::session::SyncOutcome> {
    let (mut client, mut server) = tokio::io::duplex(1 << 20);

    let server_ctx = source.ctx.clone();
    let responder = tokio::spawn(async move { serve_session(&mut server, &server_ctx).await });

    let outcome = pull_from_peer(&mut client, &puller.ctx, limits).await;
    // Dropping the client closes the pipe, which ends the responder loop.
    drop(client);
    let _ = responder.await;
    outcome
}

#[tokio::test]
async fn pulling_transfers_operations_and_content() {
    let a = node(1, 1, true);
    let b = node(2, 2, true);

    a.write("/notes/todo.txt", "buy milk");

    let outcome = pull(&b, &a).await.unwrap();

    assert_eq!(outcome.peer, Some(DeviceId(1)));
    assert!(outcome.ops_received > 0);
    assert!(outcome.blobs_received > 0);
    assert_eq!(outcome.still_pending, 0);

    assert_eq!(b.read("/notes/todo.txt"), "buy milk");
    assert_eq!(a.state_root(), b.state_root());
}

#[tokio::test]
async fn a_second_pull_transfers_nothing() {
    let a = node(1, 1, true);
    let b = node(2, 2, true);
    a.write("/f.txt", "x");

    pull(&b, &a).await.unwrap();
    let second = pull(&b, &a).await.unwrap();

    assert_eq!(second.ops_received, 0, "nothing new should be offered");
    assert_eq!(second.blobs_received, 0);
    assert_eq!(a.state_root(), b.state_root());
}

#[tokio::test]
async fn bidirectional_pulls_converge() {
    // Each node edited while apart; one pull each way should reconcile both.
    let a = node(1, 1, true);
    let b = node(2, 2, true);

    a.mkdir("/shared");
    a.write("/shared/from-a.txt", "written on A");
    b.mkdir("/shared");
    b.write("/shared/from-b.txt", "written on B");

    assert_ne!(a.state_root(), b.state_root());

    pull(&b, &a).await.unwrap();
    pull(&a, &b).await.unwrap();

    assert_eq!(
        a.state_root(),
        b.state_root(),
        "both directions pulled; the nodes must agree"
    );
    assert_eq!(a.listing("/"), b.listing("/"));
    // Concurrent same-name directories both survive, one renamed.
    assert_eq!(a.listing("/").len(), 2);
}

#[tokio::test]
async fn large_content_transfers_across_batches() {
    let a = node(1, 1, true);
    let b = node(2, 2, true);

    let payload = "y".repeat(400_000);
    a.write("/big.bin", &payload);

    pull(&b, &a).await.unwrap();

    assert_eq!(b.read("/big.bin").len(), payload.len());
    assert_eq!(a.state_root(), b.state_root());
}

#[tokio::test]
async fn many_operations_stream_in_multiple_batches() {
    // More operations than one batch holds, so the Have/OpsBatch loop must repeat.
    let a = node(1, 1, true);
    let b = node(2, 2, true);

    a.mkdir("/many");
    for i in 0..200 {
        a.write(&format!("/many/f{i}.txt"), &format!("content {i}"));
    }

    let outcome = pull(&b, &a).await.unwrap();

    assert!(outcome.ops_received >= 400, "got {}", outcome.ops_received);
    assert_eq!(b.listing("/many").len(), 200);
    assert_eq!(a.state_root(), b.state_root());
}

#[tokio::test]
async fn deletions_replicate() {
    let a = node(1, 1, true);
    let b = node(2, 2, true);

    a.write("/gone.txt", "temporary");
    pull(&b, &a).await.unwrap();
    assert_eq!(b.listing("/"), vec!["gone.txt"]);

    a.ctx
        .core
        .remove_path(&a.ctx.identity, "/gone.txt", now_ms())
        .unwrap();
    pull(&b, &a).await.unwrap();

    assert!(
        b.listing("/").is_empty(),
        "the unlink should have replicated"
    );
    assert_eq!(a.state_root(), b.state_root());
}

#[tokio::test]
async fn an_unknown_peer_is_refused_when_tofu_is_off() {
    let a = node(1, 1, true);
    // b does not trust on first use and has never seen a.
    let b = node(2, 2, false);

    a.write("/f.txt", "x");
    let result = pull(&b, &a).await;

    assert!(result.is_err(), "unknown peer must be refused");
    assert!(b.listing("/").is_empty(), "no state should have changed");
}

#[tokio::test]
async fn a_peer_that_changes_its_key_is_refused() {
    let a = node(1, 1, true);
    let b = node(2, 2, true);
    a.write("/f.txt", "x");

    // First contact pins a's key.
    pull(&b, &a).await.unwrap();

    // Same device id, different signing key — impersonation or an unannounced
    // rotation. Either way it must not be accepted silently.
    let mut imposter = node(1, 99, true);
    imposter.ctx.core = a.ctx.core.clone();
    imposter.ctx.device_id = DeviceId(1);

    let result = pull(&b, &imposter).await;
    assert!(result.is_err(), "a changed key must be refused");
}

#[tokio::test]
async fn tampered_operations_are_rejected_without_aborting_the_sync() {
    let a = node(1, 1, true);
    let b = node(2, 2, true);

    a.write("/good.txt", "legitimate");

    // Forge an operation signed by nobody and inject it into a's oplog directly,
    // bypassing a's own apply path.
    let mut forged = a
        .ctx
        .core
        .make_op(
            &a.ctx.identity,
            nexusfs_proto::FsOpKind::Mkdir {
                parent: 1,
                name: "evil".into(),
                mode: 0o40755,
            },
            now_ms(),
        )
        .unwrap();
    forged.sig = vec![0u8; 64];
    a.ctx.core.append_op(&forged).unwrap();

    pull(&b, &a).await.unwrap();

    // The good file arrived; the forged directory did not.
    assert_eq!(b.read("/good.txt"), "legitimate");
    assert!(
        !b.listing("/").contains(&"evil".to_string()),
        "an operation with an invalid signature must not apply"
    );
}

#[tokio::test]
async fn content_that_does_not_match_its_hash_is_discarded() {
    let a = node(1, 1, true);
    let b = node(2, 2, true);

    a.write("/f.txt", "real content");

    // Corrupt the stored chunk so a serves bytes that do not match the hash the
    // operation references.
    let entries = a.ctx.core.read_dir_path("/").unwrap();
    let inode = entries[0].inode_id;
    let record = a.ctx.core.load_inode(inode).unwrap().unwrap();
    let node_hash = record.content.value.node_hash.unwrap();
    if let Some(nexusfs_core::Object::FileNode(f)) = a.ctx.core.get_object(&node_hash).unwrap() {
        let chunk = f.chunks[0].hash;
        a.ctx.core.stores.blobs.put(chunk, b"tampered!!!").unwrap();
    }

    pull(&b, &a).await.unwrap();

    // The write parks rather than publishing content that failed verification.
    assert!(
        b.ctx.core.read_file_path("/f.txt").is_err() || b.ctx.core.pending_count().unwrap() > 0,
        "unverified content must not become readable"
    );
}

// --- encryption over the wire ------------------------------------------------

fn encrypted_node(device: u128, seed: u8, repo_key: [u8; 32]) -> Node {
    let n = node(device, seed, true);
    Node {
        ctx: nexusfs_net::session::SessionCtx {
            core: n
                .ctx
                .core
                .clone()
                .with_encryption(Arc::new(nexusfs_crypto::RepoCipher::new(repo_key))),
            ..n.ctx
        },
    }
}

#[tokio::test]
async fn encrypted_content_replicates_to_a_peer_sharing_the_key() {
    // The end-to-end M4 claim: at-rest encryption must not break replication.
    let a = encrypted_node(1, 1, [42u8; 32]);
    let b = encrypted_node(2, 2, [42u8; 32]);

    a.write("/vault/secret.txt", "classified");
    pull(&b, &a).await.unwrap();

    assert_eq!(b.read("/vault/secret.txt"), "classified");
    assert_eq!(a.state_root(), b.state_root());
}

#[tokio::test]
async fn a_peer_without_the_repository_key_replicates_but_cannot_read() {
    // Chunks are named by ciphertext hash, so verification succeeds without the key.
    // The content simply stays unreadable — which is the point of encrypting it.
    let a = encrypted_node(1, 1, [42u8; 32]);
    let b = encrypted_node(2, 2, [99u8; 32]);

    a.write("/vault/secret.txt", "classified");
    let outcome = pull(&b, &a).await.unwrap();

    assert!(outcome.ops_received > 0, "operations should still transfer");
    assert!(outcome.blobs_received > 0, "content should still transfer");
    assert_eq!(
        a.state_root(),
        b.state_root(),
        "structure converges even without the key"
    );
    assert!(
        b.ctx.core.read_file_path("/vault/secret.txt").is_err(),
        "content must not be readable with the wrong repository key"
    );
}

// --- energy budget -----------------------------------------------------------
//
// The claim M5 rests on is that a constrained device can stay useful by taking the
// namespace and skipping the bytes. These check that the budget actually produces that
// split, rather than just being carried around unread.

#[tokio::test]
async fn a_metadata_only_budget_converges_the_namespace_without_the_bytes() {
    let a = node(1, 1, true);
    let b = node(2, 2, true);

    a.mkdir("/reports");
    a.write("/reports/q3.txt", "revenue up");

    let outcome = pull_with(&b, &a, SyncLimits::metadata_only())
        .await
        .unwrap();

    assert!(outcome.ops_received > 0, "operations should still transfer");
    assert_eq!(outcome.blobs_received, 0, "no content should transfer");
    assert_eq!(outcome.content_bytes, 0);
    assert!(
        outcome.content_deferred,
        "the outcome must say the content was withheld, not that there was none"
    );

    // The point of the exercise: the device knows the file exists and where it lives.
    assert_eq!(b.listing("/reports"), vec!["q3.txt".to_string()]);
    assert!(
        !b.ctx.core.missing_chunk_hashes().unwrap().is_empty(),
        "the content is known to be outstanding"
    );

    // And the deferral is recoverable — nothing was dropped on the floor.
    let second = pull(&b, &a).await.unwrap();
    assert!(second.blobs_received > 0);
    assert!(!second.content_deferred);
    assert_eq!(b.read("/reports/q3.txt"), "revenue up");
    assert_eq!(a.state_root(), b.state_root());
}

#[tokio::test]
async fn a_byte_cap_stops_part_way_and_resumes_on_the_next_pass() {
    let a = node(1, 1, true);
    let b = node(2, 2, true);

    // Several separate files, so the transfer has natural stopping points. The contents
    // must differ: identical bytes deduplicate to a single blob under content
    // addressing, and there would be nothing left to defer.
    for i in 0..6 {
        a.write(&format!("/bulk/file{i}.bin"), &format!("{i}").repeat(4096));
    }

    let capped = pull_with(
        &b,
        &a,
        SyncLimits {
            content: true,
            max_content_bytes: 4096,
        },
    )
    .await
    .unwrap();

    assert!(capped.ops_received > 0);
    assert!(
        capped.content_deferred,
        "the cap should have been reached before the backlog cleared"
    );
    assert!(
        !b.ctx.core.missing_chunk_hashes().unwrap().is_empty(),
        "some content should still be outstanding"
    );

    // Uncapped, the rest arrives and the two agree.
    pull(&b, &a).await.unwrap();
    assert!(b.ctx.core.missing_chunk_hashes().unwrap().is_empty());
    assert_eq!(a.state_root(), b.state_root());
}

#[tokio::test]
async fn an_unlimited_budget_is_indistinguishable_from_no_budget() {
    // Guards against the throttle leaking into the default path.
    let a = node(1, 1, true);
    let b = node(2, 2, true);

    a.write("/notes/plain.txt", "hello");
    let outcome = pull_with(&b, &a, SyncLimits::unlimited()).await.unwrap();

    assert!(outcome.blobs_received > 0);
    assert!(!outcome.content_deferred);
    assert_eq!(b.read("/notes/plain.txt"), "hello");
}

// --- loop termination --------------------------------------------------------
//
// The Have/OpsBatch exchange advances by our clock summary, which is the highest
// *contiguous* counter per device. An operation we refuse never enters the log, so the
// summary stops below it — and the peer keeps answering with the identical batch.

/// Build a signed operation with a chosen device and counter.
///
/// `make_op` allocates a counter from local state, which is exactly what these tests
/// need to control.
fn signed_op(
    identity: &Identity,
    device: u128,
    counter: u64,
    time_unix_ms: u64,
    kind: nexusfs_proto::FsOpKind,
) -> nexusfs_proto::FsOp {
    let mut op = nexusfs_proto::FsOp {
        id: nexusfs_proto::OpId {
            device_id: DeviceId(device),
            counter,
        },
        time_unix_ms,
        ctx: nexusfs_proto::CausalCtx { deps: vec![] },
        kind,
        author_pubkey: identity.pubkey_bytes(),
        sig: Vec::new(),
        proof: None,
    };
    op.sig = nexusfs_crypto::sign(identity.signing_key(), &op.signing_bytes().unwrap());
    op
}

fn mkdir_op(name: &str) -> nexusfs_proto::FsOpKind {
    nexusfs_proto::FsOpKind::Mkdir {
        parent: nexusfs_core::ROOT_INODE,
        name: name.to_string(),
        mode: 0o40755,
    }
}

#[tokio::test]
async fn a_rejected_operation_low_in_the_log_does_not_spin_the_session() {
    let a = node(1, 1, true);
    let b = node(2, 2, true);

    // More operations than one batch holds, all from one device, with the *first*
    // refused. Our contiguous counter for that device can never pass zero, so every
    // later Have asks for the identical window and nothing in it can be applied.
    let author = Identity::from_seed([9u8; 32]);
    let mut forged = signed_op(&author, 0xBB, 1, 1_000, mkdir_op("poisoned"));
    forged.kind = mkdir_op("tampered-after-signing");
    a.ctx.core.append_op(&forged).unwrap();

    for counter in 2..=300u64 {
        let op = signed_op(
            &author,
            0xBB,
            counter,
            1_000 + counter,
            mkdir_op(&format!("d{counter}")),
        );
        a.ctx.core.apply_op(&op).unwrap();
    }

    let outcome = pull(&b, &a).await.unwrap();

    assert!(
        outcome.ops_received < 1_500,
        "the session re-requested the same window instead of stopping: {} operations \
         received from a peer holding 300",
        outcome.ops_received
    );
    // The healthy operations still land; only the poisoned one is dropped.
    assert_eq!(b.listing("/").len(), 299);
}

// --- fetching content that was deliberately deferred -------------------------
//
// The state these exercise is the one the energy scheduler creates on purpose: a node
// that took every operation and none of the bytes. It knows the file exists and cannot
// read it, and the question is whether a reader can close that gap on demand instead of
// waiting for the next unconstrained pass.

/// Fetch exactly `wanted` from `source`, over a fresh connection.
async fn fetch(
    puller: &Node,
    source: &Node,
    wanted: &[nexusfs_proto::Hash],
) -> anyhow::Result<usize> {
    let (mut client, mut server) = tokio::io::duplex(1 << 20);
    let server_ctx = source.ctx.clone();
    let responder = tokio::spawn(async move { serve_session(&mut server, &server_ctx).await });

    let got = nexusfs_net::session::fetch_chunks(&mut client, &puller.ctx, wanted).await;
    drop(client);
    let _ = responder.await;
    got
}

#[tokio::test]
async fn a_deferred_file_can_be_fetched_when_someone_reads_it() {
    let a = node(1, 1, true);
    let b = node(2, 2, true);
    a.write("/notes/deferred.txt", "content that was never transferred");

    // Take the namespace, skip the bytes — exactly what a metadata-only budget does.
    let outcome = pull_with(&b, &a, SyncLimits::metadata_only())
        .await
        .unwrap();
    assert!(outcome.content_deferred);
    assert_eq!(b.listing("/notes"), vec!["deferred.txt".to_string()]);

    // The file is known and unreadable, and the node can say precisely what it lacks.
    let wanted = b
        .ctx
        .core
        .missing_chunks_for_path("/notes/deferred.txt")
        .unwrap();
    assert!(
        !wanted.is_empty(),
        "the read should have something to ask for"
    );
    // Note what a bare read gives you before the fetch: an *empty* file, because the
    // write is parked rather than applied. That is precisely why the facades consult
    // `missing_chunks_for_path` and refuse rather than serving this.
    assert_eq!(
        b.ctx.core.read_file_path("/notes/deferred.txt").unwrap(),
        b""
    );

    // Ask for exactly that, and the read succeeds.
    let stored = fetch(&b, &a, &wanted).await.unwrap();
    assert_eq!(stored, wanted.len());
    assert_eq!(
        b.read("/notes/deferred.txt"),
        "content that was never transferred"
    );
    assert!(b
        .ctx
        .core
        .missing_chunks_for_path("/notes/deferred.txt")
        .unwrap()
        .is_empty());
}

#[tokio::test]
async fn fetching_takes_only_what_was_asked_for() {
    // An on-demand fetch is not a sync in disguise: reading one file must not drag the
    // whole backlog across, or the budget the scheduler set means nothing.
    let a = node(1, 1, true);
    let b = node(2, 2, true);
    a.write("/wanted.txt", "the file being read");
    a.write("/unwanted.bin", &"x".repeat(50_000));

    pull_with(&b, &a, SyncLimits::metadata_only())
        .await
        .unwrap();

    let wanted = b.ctx.core.missing_chunks_for_path("/wanted.txt").unwrap();
    fetch(&b, &a, &wanted).await.unwrap();

    assert_eq!(b.read("/wanted.txt"), "the file being read");
    assert!(
        !b.ctx
            .core
            .missing_chunks_for_path("/unwanted.bin")
            .unwrap()
            .is_empty(),
        "the untouched file's content should still be deferred"
    );
}

#[tokio::test]
async fn a_peer_that_does_not_have_the_content_is_not_an_error() {
    // The caller reports the content as unavailable rather than pretending the file is
    // short — and a fetch that finds nothing must simply return, not hang or fail.
    let a = node(1, 1, true);
    let b = node(2, 2, true);
    a.write("/f.txt", "content");

    pull_with(&b, &a, SyncLimits::metadata_only())
        .await
        .unwrap();
    let wanted = b.ctx.core.missing_chunks_for_path("/f.txt").unwrap();

    // A third node that holds nothing at all.
    let empty = node(3, 3, true);
    let stored = fetch(&b, &empty, &wanted).await.unwrap();
    assert_eq!(stored, 0);
    assert!(
        !b.ctx
            .core
            .missing_chunks_for_path("/f.txt")
            .unwrap()
            .is_empty(),
        "the content is still outstanding, and a caller must be able to see that"
    );
}

#[tokio::test]
async fn content_that_does_not_match_its_hash_is_refused_on_demand_too() {
    // The on-demand path must verify exactly as a sync pass does; it is a second door
    // into the same store.
    let a = node(1, 1, true);
    let b = node(2, 2, true);
    a.write("/f.txt", "genuine content");

    pull_with(&b, &a, SyncLimits::metadata_only())
        .await
        .unwrap();
    let wanted = b.ctx.core.missing_chunks_for_path("/f.txt").unwrap();

    // Corrupt the source's copy in place, so it serves bytes that do not match.
    for hash in &wanted {
        a.ctx.core.stores.blobs.put(*hash, b"tampered").unwrap();
    }

    let stored = fetch(&b, &a, &wanted).await.unwrap();
    assert_eq!(stored, 0, "mismatched content must be discarded");
    assert!(!b
        .ctx
        .core
        .missing_chunks_for_path("/f.txt")
        .unwrap()
        .is_empty());
}
