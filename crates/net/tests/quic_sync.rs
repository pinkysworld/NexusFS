//! End-to-end replication over real QUIC sockets.
//!
//! The duplex tests cover protocol logic; this covers the transport actually working —
//! endpoint setup, ALPN, certificate handling and stream plumbing.

use std::sync::Arc;
use std::time::Duration;

use nexusfs_core::{now_ms, CoreState, Stores};
use nexusfs_crypto::Identity;
use nexusfs_net::peers::{accept_loop, sync_once};
use nexusfs_net::quic;
use nexusfs_net::session::SessionCtx;
use nexusfs_net::trust::TrustPolicy;
use nexusfs_proto::DeviceId;
use nexusfs_storage::mem_store::MemStore;

fn ctx(device: u128, seed: u8) -> SessionCtx {
    let store = MemStore::new();
    let core = CoreState::new(
        Stores {
            blobs: Arc::new(store.clone()),
            kv: Arc::new(store),
        },
        DeviceId(device),
    );
    core.bootstrap_if_needed().unwrap();

    SessionCtx {
        core,
        identity: Identity::from_seed([seed; 32]),
        device_id: DeviceId(device),
        trust: TrustPolicy { tofu: true },
    }
}

#[tokio::test]
async fn two_nodes_converge_over_quic() {
    let server = ctx(1, 1);
    let client = ctx(2, 2);

    server
        .core
        .write_file(
            &server.identity,
            "/docs/report.txt",
            b"replicated over the wire",
            now_ms(),
        )
        .unwrap();
    // Something large enough to span several chunks.
    server
        .core
        .write_file(
            &server.identity,
            "/docs/big.bin",
            &vec![7u8; 250_000],
            now_ms(),
        )
        .unwrap();

    // Port 0 lets the OS pick, so the test cannot collide with anything.
    let server_endpoint = quic::endpoint("127.0.0.1:0".parse().unwrap()).unwrap();
    let addr = server_endpoint.local_addr().unwrap();

    let serving = tokio::spawn(accept_loop(server_endpoint.clone(), server.clone()));

    let client_endpoint = quic::endpoint("127.0.0.1:0".parse().unwrap()).unwrap();
    let outcome = tokio::time::timeout(
        Duration::from_secs(30),
        sync_once(&client_endpoint, addr, &client),
    )
    .await
    .expect("sync timed out")
    .expect("sync failed");

    assert_eq!(outcome.peer, Some(DeviceId(1)));
    assert!(outcome.ops_received > 0);
    assert_eq!(outcome.still_pending, 0);

    assert_eq!(
        client.core.read_file_path("/docs/report.txt").unwrap(),
        b"replicated over the wire"
    );
    assert_eq!(
        client.core.read_file_path("/docs/big.bin").unwrap().len(),
        250_000
    );
    assert_eq!(
        server.core.compute_state_root().unwrap(),
        client.core.compute_state_root().unwrap(),
        "the two nodes must agree after syncing"
    );

    serving.abort();
}

#[tokio::test]
async fn repeated_syncs_are_idempotent() {
    let server = ctx(1, 1);
    let client = ctx(2, 2);
    server
        .core
        .write_file(&server.identity, "/a.txt", b"once", now_ms())
        .unwrap();

    let server_endpoint = quic::endpoint("127.0.0.1:0".parse().unwrap()).unwrap();
    let addr = server_endpoint.local_addr().unwrap();
    let serving = tokio::spawn(accept_loop(server_endpoint.clone(), server.clone()));
    let client_endpoint = quic::endpoint("127.0.0.1:0".parse().unwrap()).unwrap();

    sync_once(&client_endpoint, addr, &client).await.unwrap();
    let root_after_first = client.core.compute_state_root().unwrap();

    for _ in 0..3 {
        let again = sync_once(&client_endpoint, addr, &client).await.unwrap();
        assert_eq!(again.ops_received, 0, "nothing new should be offered");
    }

    assert_eq!(client.core.compute_state_root().unwrap(), root_after_first);
    serving.abort();
}
