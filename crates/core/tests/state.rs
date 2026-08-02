mod common;

use common::*;
use nexusfs_core::ROOT_INODE;
use nexusfs_crypto::Identity;

#[test]
fn store_chunks_records_offsets_and_reconstructs_bytes() {
    let dir = tempfile::tempdir().unwrap();
    let mut core = open_core(dir.path(), 0xfeed);
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
fn head_survives_store_reopen() {
    let dir = tempfile::tempdir().unwrap();

    let original = bootstrapped(dir.path(), 0xfeed);
    let head = original.get_head().unwrap();
    drop(original);

    let reopened = open_core(dir.path(), 0xfeed);
    assert_eq!(reopened.get_head().unwrap(), head);
}

#[test]
fn bootstrap_is_identical_across_devices() {
    // Two fresh repositories must agree on the empty-filesystem state root, otherwise
    // replicas could never converge no matter what operations they exchange.
    let a_dir = tempfile::tempdir().unwrap();
    let b_dir = tempfile::tempdir().unwrap();
    let a = bootstrapped(a_dir.path(), 0x01);
    let b = bootstrapped(b_dir.path(), 0x02);

    assert_eq!(
        a.compute_state_root().unwrap(),
        b.compute_state_root().unwrap()
    );
}

#[test]
fn clock_summary_tracks_applied_ops() {
    let dir = tempfile::tempdir().unwrap();
    let core = bootstrapped(dir.path(), 0x0a);
    let id = Identity::generate();

    core.apply_op(&signed_op(&id, 0x0a, 1, 1_000, mkdir(ROOT_INODE, "one")))
        .unwrap();
    core.apply_op(&signed_op(&id, 0x0a, 2, 2_000, mkdir(ROOT_INODE, "two")))
        .unwrap();
    core.apply_op(&signed_op(&id, 0x0b, 7, 3_000, mkdir(ROOT_INODE, "three")))
        .unwrap();

    let entries = core.clock_summary().unwrap().entries;
    assert_eq!(entries.len(), 2);
    assert!(entries.iter().any(|(d, c)| d.0 == 0x0a && *c == 2));
    assert!(entries.iter().any(|(d, c)| d.0 == 0x0b && *c == 7));
    assert_eq!(core.applied_count().unwrap(), 3);
}
