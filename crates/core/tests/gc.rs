//! Garbage collection.
//!
//! The stakes are asymmetric: failing to reclaim wastes disk, but reclaiming something
//! reachable destroys data. So most of these assert what *survives*, and the few that
//! assert deletion also read the repository back afterwards.

mod common;

use common::*;
use nexusfs_core::{inode_for_op, ROOT_INODE};
use nexusfs_crypto::Identity;
use nexusfs_proto::{ChunkRef, DeviceId, FsOpKind, OpId};

fn op_inode(device: u128, counter: u64) -> u128 {
    inode_for_op(OpId {
        device_id: DeviceId(device),
        counter,
    })
}

#[test]
fn ordinary_history_accumulates_collectable_snapshots() {
    // Nothing here is overwritten or deleted, yet there is still garbage: each applied
    // operation rebuilds the snapshot, orphaning the previous SnapshotRoot and the
    // DirNodes along the changed path. This is the dominant source, and the reason
    // collection is worth having on a repository that only ever appends.
    let dir = tempfile::tempdir().unwrap();
    let core = bootstrapped(dir.path(), 0xA1);
    let id = Identity::generate();

    core.mkdir_p(&id, "/docs", 1_000).unwrap();
    core.write_file(&id, "/docs/a.txt", b"hello", 1_001)
        .unwrap();

    let report = core.collect_garbage(true).unwrap();
    assert!(report.reachable > 0);
    assert!(
        report.unreachable > 0,
        "superseded snapshots should be collectable"
    );

    core.collect_garbage(false).unwrap();
    assert_eq!(core.read_file_path("/docs/a.txt").unwrap(), b"hello");
    assert!(core.verify_repository().unwrap().ok());
}

#[test]
fn overwriting_a_file_leaves_its_old_content_collectable() {
    let dir = tempfile::tempdir().unwrap();
    let core = bootstrapped(dir.path(), 0xA1);
    let id = Identity::generate();

    core.write_file(&id, "/notes.txt", b"the first version", 1_000)
        .unwrap();
    core.write_file(&id, "/notes.txt", b"the second version", 2_000)
        .unwrap();

    let survey = core.collect_garbage(true).unwrap();
    assert!(
        survey.unreachable > 0,
        "the superseded version should be unreachable"
    );
    assert!(survey.bytes_reclaimable > 0);
    assert_eq!(survey.deleted, 0, "a survey must not delete");

    let collected = core.collect_garbage(false).unwrap();
    assert_eq!(collected.deleted, survey.unreachable);
    assert!(collected.refused.is_none());

    // The surviving version still reads, and the repository still audits clean.
    assert_eq!(
        core.read_file_path("/notes.txt").unwrap(),
        b"the second version"
    );
    assert!(core.verify_repository().unwrap().ok());

    // And a second pass finds nothing left.
    assert_eq!(core.collect_garbage(true).unwrap().unreachable, 0);
}

#[test]
fn collecting_twice_is_stable() {
    let dir = tempfile::tempdir().unwrap();
    let core = bootstrapped(dir.path(), 0xA1);
    let id = Identity::generate();

    core.mkdir_p(&id, "/a/b/c", 1_000).unwrap();
    for i in 0..5 {
        core.write_file(
            &id,
            "/a/b/c/f.txt",
            format!("version {i}").as_bytes(),
            1_000 + i,
        )
        .unwrap();
    }

    core.collect_garbage(false).unwrap();
    let after = core.collect_garbage(false).unwrap();
    assert_eq!(after.deleted, 0, "the second pass should find nothing");
    assert_eq!(core.read_file_path("/a/b/c/f.txt").unwrap(), b"version 4");
}

#[test]
fn unlinked_files_become_collectable() {
    let dir = tempfile::tempdir().unwrap();
    let core = bootstrapped(dir.path(), 0xA1);
    let id = Identity::generate();

    core.write_file(&id, "/temp.txt", &vec![b'x'; 4096], 1_000)
        .unwrap();
    core.write_file(&id, "/keep.txt", b"keep me", 1_001)
        .unwrap();

    // Clear the snapshot churn first, so what the unlink frees is measured on its own.
    core.collect_garbage(false).unwrap();
    let content_hashes: Vec<_> = core
        .read_dir_path("/")
        .unwrap()
        .iter()
        .filter(|e| e.name == "temp.txt")
        .filter_map(|e| core.load_inode(e.inode_id).unwrap())
        .filter_map(|r| r.content.value.node_hash)
        .collect();
    assert_eq!(content_hashes.len(), 1);

    core.remove_path(&id, "/temp.txt", 2_000).unwrap();

    let report = core.collect_garbage(false).unwrap();
    assert!(
        report.bytes_deleted >= 4096,
        "the unlinked content should go, freed {} bytes",
        report.bytes_deleted
    );
    assert!(
        !core.stores.blobs.has(&content_hashes[0]).unwrap(),
        "the unlinked file's node should be gone"
    );
    assert_eq!(core.read_file_path("/keep.txt").unwrap(), b"keep me");
}

#[test]
fn content_a_pending_operation_needs_is_never_collected() {
    // A write that has not applied yet still holds references. Collecting them would
    // strand the operation permanently.
    let dir = tempfile::tempdir().unwrap();
    let core = bootstrapped(dir.path(), 0xA1);
    let id = Identity::generate();

    // A write against an inode that does not exist yet: the op parks, but its content
    // is already in the store.
    let orphan_inode = op_inode(0xBB, 1);
    let kind = write_all(
        &core,
        orphan_inode,
        b"content for an operation that cannot apply yet",
    );
    let chunk_hashes: Vec<_> = match &kind {
        FsOpKind::Write { chunks, .. } => chunks.iter().map(|c| c.hash).collect(),
        _ => unreachable!(),
    };
    let op = signed_op(&id, 0xBB, 2, 3_000, kind);
    core.apply_op(&op).unwrap();

    assert!(
        !core.pending_ops().unwrap().is_empty(),
        "the op should be parked"
    );

    let reachable = core.reachable_hashes().unwrap();
    for hash in &chunk_hashes {
        assert!(
            reachable.contains(hash),
            "content held by a pending operation must stay reachable"
        );
    }

    core.collect_garbage(false).unwrap();
    for hash in &chunk_hashes {
        assert!(
            core.stores.blobs.has(hash).unwrap(),
            "collection must not strand the pending operation"
        );
    }
}

#[test]
fn a_superseded_write_applies_without_its_content() {
    // The property garbage collection rests on. Once the content register has moved
    // past a write, that write can never affect state — so it must not sit waiting for
    // bytes that a peer may already have collected.
    let dir = tempfile::tempdir().unwrap();
    let core = bootstrapped(dir.path(), 0xA1);
    let id = Identity::generate();

    let file_inode = op_inode(0xA1, 1);
    let create = signed_op(&id, 0xA1, 1, 1_000, create_file(ROOT_INODE, "f.txt"));
    core.apply_op(&create).unwrap();

    // The newer write lands first and its content is present.
    let newer = signed_op(&id, 0xA1, 3, 3_000, write_all(&core, file_inode, b"newer"));
    core.apply_op(&newer).unwrap();

    // Now an older write arrives whose chunks were never transferred. It loses the
    // register, so it should apply as a no-op rather than parking.
    let older = signed_op(
        &id,
        0xA1,
        2,
        2_000,
        FsOpKind::Write {
            inode: file_inode,
            offset: 0,
            chunks: vec![nexusfs_proto::ChunkRef {
                hash: [0x42; 32],
                len: 5,
                plain_len: 5,
                offset: 0,
            }],
            new_size: 5,
            encryption: None,
        },
    );
    core.apply_op(&older).unwrap();

    assert!(
        core.pending_ops().unwrap().is_empty(),
        "a superseded write must not park waiting for content nothing will read"
    );
    assert!(
        core.missing_chunk_hashes().unwrap().is_empty(),
        "and it must not keep asking peers for that content"
    );
    assert_eq!(core.read_file_path("/f.txt").unwrap(), b"newer");
}

#[test]
fn a_write_that_can_still_win_keeps_waiting() {
    // The converse: an op that would win the register must park, not be silently
    // dropped, or content would be lost whenever it arrives out of order.
    let dir = tempfile::tempdir().unwrap();
    let core = bootstrapped(dir.path(), 0xA1);
    let id = Identity::generate();

    let file_inode = op_inode(0xA1, 1);
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
        write_all(&core, file_inode, b"first"),
    ))
    .unwrap();

    let future = signed_op(
        &id,
        0xA1,
        3,
        9_000,
        FsOpKind::Write {
            inode: file_inode,
            offset: 0,
            chunks: vec![nexusfs_proto::ChunkRef {
                hash: [0x77; 32],
                len: 6,
                plain_len: 6,
                offset: 0,
            }],
            new_size: 6,
            encryption: None,
        },
    );
    core.apply_op(&future).unwrap();

    assert_eq!(
        core.pending_ops().unwrap().len(),
        1,
        "a write that can still win must wait for its content"
    );
    assert_eq!(core.missing_chunk_hashes().unwrap().len(), 1);
    assert_eq!(core.read_file_path("/f.txt").unwrap(), b"first");
}

#[test]
fn collection_refuses_when_the_namespace_is_missing() {
    // The catastrophic case. Marking works by materializing the tree, so on a wiped
    // repository it would store a fresh empty root and call that object reachable —
    // then treat every real object as garbage. The precondition check has to run first.
    let dir = tempfile::tempdir().unwrap();
    let core = bootstrapped(dir.path(), 0xA1);
    let id = Identity::generate();
    core.write_file(&id, "/a.txt", b"data", 1_000).unwrap();

    let before = core.stores.blobs.list().unwrap().len();
    for (key, _) in core.stores.kv.scan_prefix("state", b"").unwrap() {
        core.stores.kv.delete_kv("state", &key).unwrap();
    }

    let report = core.collect_garbage(false).unwrap();
    assert!(
        report.refused.is_some(),
        "it must decline, not empty the store"
    );
    assert_eq!(report.deleted, 0);
    assert_eq!(
        core.stores.blobs.list().unwrap().len(),
        before,
        "no blob should have been removed"
    );
}

#[test]
fn collection_refuses_without_a_head() {
    let dir = tempfile::tempdir().unwrap();
    let core = bootstrapped(dir.path(), 0xA1);
    let id = Identity::generate();
    core.write_file(&id, "/a.txt", b"data", 1_000).unwrap();

    let before = core.stores.blobs.list().unwrap().len();
    core.stores.kv.delete_kv("meta", b"head/current").unwrap();

    let report = core.collect_garbage(false).unwrap();
    assert!(report.refused.is_some());
    assert_eq!(core.stores.blobs.list().unwrap().len(), before);
}

#[test]
fn collection_survives_a_restart() {
    let dir = tempfile::tempdir().unwrap();
    let id = Identity::generate();

    {
        let core = bootstrapped(dir.path(), 0xA1);
        core.write_file(&id, "/f.txt", b"one", 1_000).unwrap();
        core.write_file(&id, "/f.txt", b"two", 2_000).unwrap();
        let report = core.collect_garbage(false).unwrap();
        assert!(report.deleted > 0);
    }

    let core = bootstrapped(dir.path(), 0xA1);
    assert_eq!(core.read_file_path("/f.txt").unwrap(), b"two");
    assert!(core.verify_repository().unwrap().ok());
}

// ---- namespace records ------------------------------------------------------------
//
// Collection used to reclaim blobs and leave every unlinked inode's key-value state
// behind. These assert the new sweep, and — more importantly — that it leaves alone
// everything a reader or a parked operation can still reach. A wrongly deleted blob can
// be fetched from a peer again; a wrongly deleted record cannot, because the operation
// that produced it is already marked applied.

#[test]
fn unlinking_leaves_records_behind_until_they_are_collected() {
    let dir = tempfile::tempdir().unwrap();
    let core = bootstrapped(dir.path(), 0xA1);
    let id = Identity::generate();

    core.mkdir_p(&id, "/scratch", 1_000).unwrap();
    core.write_file(&id, "/scratch/a.txt", b"hello", 1_001)
        .unwrap();
    let doomed = core
        .read_dir_path("/scratch")
        .unwrap()
        .iter()
        .find(|e| e.name == "a.txt")
        .unwrap()
        .inode_id;

    core.collect_garbage(false).unwrap();
    assert!(
        core.reachable_inodes().unwrap().contains(&doomed),
        "the file is still in the tree"
    );

    core.remove_path(&id, "/scratch/a.txt", 2_000).unwrap();
    assert!(
        !core.reachable_inodes().unwrap().contains(&doomed),
        "unlinked, so nothing can reach it"
    );

    let survey = core.collect_garbage(true).unwrap();
    assert!(
        survey.records_unreachable > 0,
        "the unlinked inode's records should be collectable"
    );
    assert_eq!(survey.records_deleted, 0, "a survey deletes nothing");
    assert!(
        core.load_inode(doomed).unwrap().is_some(),
        "and leaves the record in place"
    );

    let swept = core.collect_garbage(false).unwrap();
    assert_eq!(swept.records_deleted, survey.records_unreachable);
    assert!(
        core.load_inode(doomed).unwrap().is_none(),
        "the orphaned inode record should be gone"
    );

    // The rest of the repository is untouched and still verifies.
    assert_eq!(core.read_dir_path("/scratch").unwrap().len(), 0);
    assert!(core.verify_repository().unwrap().ok());
}

#[test]
fn a_second_pass_finds_no_records_left() {
    // Same stability property the blob sweep has: if a second pass still reports
    // orphans, the walk and the sweep disagree about what is reachable.
    let dir = tempfile::tempdir().unwrap();
    let core = bootstrapped(dir.path(), 0xA1);
    let id = Identity::generate();

    core.mkdir_p(&id, "/a/b/c", 1_000).unwrap();
    core.write_file(&id, "/a/b/c/x.txt", b"x", 1_001).unwrap();
    core.remove_path(&id, "/a/b/c/x.txt", 2_000).unwrap();
    core.collect_garbage(false).unwrap();

    let again = core.collect_garbage(true).unwrap();
    assert_eq!(
        again.records_unreachable, 0,
        "collection should be stable, found {} orphans on the second pass",
        again.records_unreachable
    );
}

#[test]
fn a_file_with_no_content_yet_keeps_its_record() {
    // The case the object walk cannot answer: a created-but-unwritten file has no
    // FileNode, so it appears in no inode map. Its record must still survive, or the
    // write that is still coming would apply into nothing.
    let dir = tempfile::tempdir().unwrap();
    let core = bootstrapped(dir.path(), 0xA1);
    let id = Identity::generate();

    let op = signed_op(
        &id,
        0xA1,
        1,
        1_000,
        FsOpKind::CreateFile {
            parent: ROOT_INODE,
            name: "empty.txt".into(),
            mode: 0o100644,
        },
    );
    core.apply_op(&op).unwrap();
    let inode = op_inode(0xA1, 1);
    assert!(core.load_inode(inode).unwrap().is_some());

    core.collect_garbage(false).unwrap();
    assert!(
        core.load_inode(inode).unwrap().is_some(),
        "a file with no content is still a file"
    );
    assert!(core
        .read_dir_path("/")
        .unwrap()
        .iter()
        .any(|e| e.name == "empty.txt"));
}

#[test]
fn records_a_pending_operation_names_are_never_collected() {
    // A write parked waiting for content names an inode. Collecting that inode's record
    // would leave the operation permanently unappliable — the same hazard the blob
    // sweep already guards against, one level up.
    let dir = tempfile::tempdir().unwrap();
    let core = bootstrapped(dir.path(), 0xA1);
    let id = Identity::generate();

    core.write_file(&id, "/a.txt", b"first", 1_000).unwrap();
    let inode = core
        .read_dir_path("/")
        .unwrap()
        .iter()
        .find(|e| e.name == "a.txt")
        .unwrap()
        .inode_id;

    // A write whose chunk never arrives, for a file that is then unlinked.
    let missing = ChunkRef {
        hash: [0x5A; 32],
        len: 9,
        plain_len: 9,
        offset: 0,
    };
    let parked = signed_op(
        &id,
        0xB2,
        1,
        3_000,
        FsOpKind::Write {
            inode,
            offset: 0,
            chunks: vec![missing],
            new_size: 9,
            encryption: None,
        },
    );
    core.apply_op(&parked).unwrap();
    assert_eq!(
        core.pending_count().unwrap(),
        1,
        "the write should be parked"
    );

    core.remove_path(&id, "/a.txt", 4_000).unwrap();
    assert!(
        core.reachable_inodes().unwrap().contains(&inode),
        "a parked operation still names this inode"
    );

    core.collect_garbage(false).unwrap();
    assert!(
        core.load_inode(inode).unwrap().is_some(),
        "the record the parked write needs must survive"
    );
}

#[test]
fn a_refused_sweep_removes_no_records_either() {
    // The precondition guard covers records as well as blobs: a repository whose root
    // record is missing would report every inode unreachable.
    let dir = tempfile::tempdir().unwrap();
    let core = bootstrapped(dir.path(), 0xA1);
    let id = Identity::generate();
    core.write_file(&id, "/a.txt", b"hello", 1_000).unwrap();

    core.stores
        .kv
        .delete_kv("state", &nexusfs_core::inode_key(ROOT_INODE))
        .unwrap();

    let report = core.collect_garbage(false).unwrap();
    assert!(report.refused.is_some(), "the sweep should refuse");
    assert_eq!(report.records_deleted, 0);
    assert_eq!(report.deleted, 0);
}
