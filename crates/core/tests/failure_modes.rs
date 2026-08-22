//! What happens when the store is damaged.
//!
//! M6's exit criterion is that an operator can *inspect, recover and maintain* state
//! with built-in tooling. Inspection is only worth anything if it notices damage, so
//! these break a repository in the ways it can realistically break and assert that the
//! tools say so rather than returning something plausible.
//!
//! The recurring theme: a wrong answer is worse than an error. Every case here checks
//! that the failure is *reported*, not absorbed.

mod common;

use common::*;
use nexusfs_core::{hash_bytes, ROOT_INODE};
use nexusfs_crypto::Identity;
use nexusfs_proto::{DeviceId, FsOpKind, OpId};

#[test]
fn a_missing_chunk_is_reported_by_verify_rather_than_read_as_a_short_file() {
    let dir = tempfile::tempdir().unwrap();
    let core = bootstrapped(dir.path(), 0xA1);
    let id = Identity::generate();

    core.write_file(&id, "/a.txt", b"content that will be deleted", 1_000)
        .unwrap();
    let expected = hash_bytes(b"content that will be deleted");
    assert!(core.stores.blobs.has(&expected).unwrap());
    core.stores.blobs.delete(&expected).unwrap();

    assert!(
        core.read_file_path("/a.txt").is_err(),
        "a file whose content is gone must not read as empty or truncated"
    );

    let report = core.verify_repository().unwrap();
    assert!(!report.ok());
    assert_eq!(report.unreadable_files, vec!["/a.txt".to_string()]);
}

#[test]
fn corrupted_content_is_caught_by_its_hash() {
    // Content addressing means detection is free: the name of a chunk *is* its
    // checksum, so silent bit-rot cannot survive a read.
    let dir = tempfile::tempdir().unwrap();
    let core = bootstrapped(dir.path(), 0xA1);
    let id = Identity::generate();

    core.write_file(&id, "/a.txt", b"the original bytes", 1_000)
        .unwrap();
    let hash = hash_bytes(b"the original bytes");

    // Overwrite the blob in place with content that does not match its address.
    core.stores.blobs.put(hash, b"tampered-with bytes").unwrap();

    let report = core.verify_repository().unwrap();
    assert!(
        !report.ok(),
        "a chunk that does not match its hash must fail the audit"
    );
}

#[test]
fn a_forged_operation_is_reported_by_verify() {
    let dir = tempfile::tempdir().unwrap();
    let core = bootstrapped(dir.path(), 0xA1);
    let id = Identity::generate();
    core.mkdir_p(&id, "/docs", 1_000).unwrap();

    // Splice an operation into the log whose signature covers different bytes.
    let mut forged = signed_op(&id, 0xA1, 99, 2_000, mkdir(ROOT_INODE, "legit"));
    forged.kind = mkdir(ROOT_INODE, "forged");
    core.append_op(&forged).unwrap();

    let report = core.verify_repository().unwrap();
    assert!(
        report.signature_failures >= 1,
        "the forgery must be counted"
    );
    assert!(!report.ok());
}

#[test]
fn an_operation_that_can_never_apply_stays_visible_rather_than_vanishing() {
    // A parked operation is not an error, but it must remain countable: an operator
    // needs to be able to tell "waiting for content" from "silently dropped".
    let dir = tempfile::tempdir().unwrap();
    let core = bootstrapped(dir.path(), 0xA1);
    let id = Identity::generate();

    let orphan = nexusfs_core::inode_for_op(OpId {
        device_id: DeviceId(0xBB),
        counter: 1,
    });
    let op = signed_op(
        &id,
        0xBB,
        2,
        3_000,
        FsOpKind::Write {
            inode: orphan,
            offset: 0,
            chunks: vec![],
            new_size: 0,
            encryption: None,
        },
    );
    core.apply_op(&op).unwrap();

    assert_eq!(core.pending_count().unwrap(), 1);
    assert_eq!(core.op_count().unwrap(), 1);
    assert_ne!(
        core.applied_count().unwrap(),
        core.op_count().unwrap(),
        "the gap between received and applied is what makes the backlog visible"
    );
}

#[test]
fn losing_the_head_is_recoverable_from_the_operation_log() {
    // The head is a cache of what the operations imply. Losing it should cost a
    // rebuild, not the repository.
    let dir = tempfile::tempdir().unwrap();
    let core = bootstrapped(dir.path(), 0xA1);
    let id = Identity::generate();

    core.mkdir_p(&id, "/docs", 1_000).unwrap();
    core.write_file(&id, "/docs/a.txt", b"survives", 1_001)
        .unwrap();
    let root_before = core.compute_state_root().unwrap();

    core.stores
        .kv
        .delete_kv(nexusfs_core::CF_META, b"head/current")
        .unwrap();
    assert!(core.get_head().unwrap().is_none());

    // Rebuilding from live state restores the head, and it commits to the same thing.
    let head = core.build_snapshot().unwrap();
    assert!(!head.is_empty());
    assert_eq!(core.compute_state_root().unwrap(), root_before);
    assert_eq!(core.read_file_path("/docs/a.txt").unwrap(), b"survives");
}

#[test]
fn a_partial_write_that_cannot_be_spliced_parks_instead_of_tearing_the_file() {
    // The splice path needs the existing bytes. If they are gone, applying anyway would
    // publish a file built on zeroes — corruption that reads back cleanly.
    let dir = tempfile::tempdir().unwrap();
    let core = bootstrapped(dir.path(), 0xA1);
    let id = Identity::generate();

    let file_inode = nexusfs_core::inode_for_op(OpId {
        device_id: DeviceId(0xA1),
        counter: 1,
    });
    core.apply_op(&signed_op(
        &id,
        0xA1,
        1,
        1_000,
        create_file(ROOT_INODE, "f.txt"),
    ))
    .unwrap();
    core.apply_op(&signed_op(
        &id,
        0xA1,
        2,
        2_000,
        write_all(&core, file_inode, b"0123456789"),
    ))
    .unwrap();

    // Remove the existing content, then attempt a partial overwrite of the middle.
    core.stores
        .blobs
        .delete(&hash_bytes(b"0123456789"))
        .unwrap();
    let (chunks, encryption) = core.store_content(b"XX").unwrap();
    let partial = signed_op(
        &id,
        0xA1,
        3,
        3_000,
        FsOpKind::Write {
            inode: file_inode,
            offset: 4,
            chunks,
            new_size: 10,
            encryption,
        },
    );
    core.apply_op(&partial).unwrap();

    assert_eq!(
        core.pending_count().unwrap(),
        1,
        "the splice must park, not fabricate the missing bytes"
    );
}

#[test]
fn garbage_collection_does_not_hide_pre_existing_damage() {
    // Collecting must not be mistakable for repairing: a repository that was already
    // broken should still report broken afterwards.
    let dir = tempfile::tempdir().unwrap();
    let core = bootstrapped(dir.path(), 0xA1);
    let id = Identity::generate();

    core.write_file(&id, "/a.txt", b"content", 1_000).unwrap();
    core.stores.blobs.delete(&hash_bytes(b"content")).unwrap();

    core.collect_garbage(false).unwrap();

    let report = core.verify_repository().unwrap();
    assert!(!report.ok());
    assert_eq!(report.unreadable_files, vec!["/a.txt".to_string()]);
}

#[test]
fn a_write_cannot_claim_more_content_than_it_supplies() {
    // `new_size` rides inside a signed operation, but a signature proves authorship,
    // not good faith. Before this was bounded, a peer could name any size it liked and
    // the splice path would allocate it — `new_size: u64::MAX` meant a zero-filled
    // allocation of the whole address space.
    let dir = tempfile::tempdir().unwrap();
    let core = bootstrapped(dir.path(), 0xA1);
    let id = Identity::generate();

    let file_inode = nexusfs_core::inode_for_op(OpId {
        device_id: DeviceId(0xA1),
        counter: 1,
    });
    core.apply_op(&signed_op(
        &id,
        0xA1,
        1,
        1_000,
        create_file(ROOT_INODE, "f.txt"),
    ))
    .unwrap();
    core.apply_op(&signed_op(
        &id,
        0xA1,
        2,
        2_000,
        write_all(&core, file_inode, b"small"),
    ))
    .unwrap();

    let (chunks, encryption) = core.store_content(b"XX").unwrap();
    let greedy = signed_op(
        &id,
        0xA1,
        3,
        3_000,
        FsOpKind::Write {
            inode: file_inode,
            offset: 0,
            chunks,
            new_size: u64::MAX,
            encryption,
        },
    );

    let err = core.apply_op(&greedy).unwrap_err().to_string();
    assert!(
        err.contains("supplies only"),
        "the size claim must be refused, not allocated: {err}"
    );
    assert_eq!(core.read_file_path("/f.txt").unwrap(), b"small");
}

#[test]
fn a_sparse_write_past_the_end_still_works() {
    // The bound must not break the legitimate case it sits next to: writing at an
    // offset beyond the current end, which zero-fills the hole.
    let dir = tempfile::tempdir().unwrap();
    let core = bootstrapped(dir.path(), 0xA1);
    let id = Identity::generate();

    let file_inode = nexusfs_core::inode_for_op(OpId {
        device_id: DeviceId(0xA1),
        counter: 1,
    });
    core.apply_op(&signed_op(
        &id,
        0xA1,
        1,
        1_000,
        create_file(ROOT_INODE, "sparse.bin"),
    ))
    .unwrap();
    core.apply_op(&signed_op(
        &id,
        0xA1,
        2,
        2_000,
        write_all(&core, file_inode, b"head"),
    ))
    .unwrap();

    let (chunks, encryption) = core.store_content(b"tail").unwrap();
    core.apply_op(&signed_op(
        &id,
        0xA1,
        3,
        3_000,
        FsOpKind::Write {
            inode: file_inode,
            offset: 64,
            chunks,
            new_size: 68,
            encryption,
        },
    ))
    .unwrap();

    let content = core.read_file_path("/sparse.bin").unwrap();
    assert_eq!(content.len(), 68);
    assert_eq!(&content[..4], b"head");
    assert_eq!(&content[4..64], &[0u8; 60]);
    assert_eq!(&content[64..], b"tail");
}
