use std::path::Path;
use std::sync::Arc;

use nexusfs_core::{CoreState, Stores};
use nexusfs_proto::{CausalCtx, DeviceId, FsOp, FsOpKind, OpId};
use nexusfs_storage::sled_store::SledStore;

fn open_core(path: &Path) -> CoreState {
    let store = SledStore::open(path).unwrap();
    let stores = Stores {
        blobs: Arc::new(store.clone()),
        kv: Arc::new(store),
    };
    CoreState::new(stores, DeviceId(0xfeed))
}

#[test]
fn store_chunks_records_offsets_and_reconstructs_bytes() {
    let dir = tempfile::tempdir().unwrap();
    let mut core = open_core(dir.path());
    core.chunk_size = 4;

    let data = b"hello distributed world";
    let refs_first = core.store_chunks(data).unwrap();
    let refs_second = core.store_chunks(data).unwrap();

    let hashes_first: Vec<_> = refs_first.iter().map(|chunk| chunk.hash).collect();
    let hashes_second: Vec<_> = refs_second.iter().map(|chunk| chunk.hash).collect();
    assert_eq!(hashes_first, hashes_second);

    let mut rebuilt = Vec::new();
    for chunk in refs_first {
        assert_eq!(chunk.offset as usize, rebuilt.len());
        let bytes = core.stores.blobs.get(&chunk.hash).unwrap().unwrap();
        assert_eq!(bytes.len(), chunk.len as usize);
        rebuilt.extend_from_slice(&bytes);
    }

    assert_eq!(rebuilt, data);
}

#[test]
fn apply_op_minimal_is_idempotent() {
    let dir = tempfile::tempdir().unwrap();
    let core = open_core(dir.path());
    core.bootstrap_if_needed().unwrap();

    let op = FsOp {
        id: OpId {
            device_id: DeviceId(7),
            counter: 1,
        },
        time_unix_ms: 42,
        ctx: CausalCtx { deps: vec![] },
        kind: FsOpKind::Mkdir {
            parent: 1,
            name: "docs".into(),
            mode: 0o40755,
        },
        author_pubkey: [0; 32],
        sig: Vec::new(),
        proof: None,
    };

    core.apply_op_minimal(&op).unwrap();
    let first_head = core.get_head().unwrap();
    let first_summary = core.clock_summary().unwrap().entries;

    core.apply_op_minimal(&op).unwrap();

    assert_eq!(core.get_head().unwrap(), first_head);
    assert_eq!(core.clock_summary().unwrap().entries, first_summary);
    assert!(core.is_op_applied(op.id).unwrap());
}

#[test]
fn head_survives_store_reopen() {
    let dir = tempfile::tempdir().unwrap();

    let original = open_core(dir.path());
    original.bootstrap_if_needed().unwrap();
    let head = original.get_head().unwrap();
    drop(original);

    let reopened = open_core(dir.path());
    assert_eq!(reopened.get_head().unwrap(), head);
}
