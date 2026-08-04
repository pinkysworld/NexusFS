mod common;

use common::*;
use nexusfs_core::{inode_for_op, ApplyOutcome, EntryType, ROOT_INODE};
use nexusfs_crdt::conflicts::conflict_name;
use nexusfs_crypto::Identity;
use nexusfs_proto::{ChunkRef, DeviceId, FsOpKind, OpId};

fn op_inode(device: u128, counter: u64) -> u128 {
    inode_for_op(OpId {
        device_id: DeviceId(device),
        counter,
    })
}

#[test]
fn mkdir_then_write_then_read_round_trip() {
    let dir = tempfile::tempdir().unwrap();
    let core = bootstrapped(dir.path(), 0xA1);
    let id = Identity::generate();

    let mkdir_op = signed_op(&id, 0xA1, 1, 1_000, mkdir(ROOT_INODE, "docs"));
    assert_eq!(core.apply_op(&mkdir_op).unwrap(), ApplyOutcome::Applied);

    let docs = op_inode(0xA1, 1);
    let create_op = signed_op(&id, 0xA1, 2, 2_000, create_file(docs, "a.txt"));
    assert_eq!(core.apply_op(&create_op).unwrap(), ApplyOutcome::Applied);

    let file = op_inode(0xA1, 2);
    let payload = b"hello nexus";
    let write_op = signed_op(&id, 0xA1, 3, 3_000, write_all(&core, file, payload));
    assert_eq!(core.apply_op(&write_op).unwrap(), ApplyOutcome::Applied);

    assert_eq!(names(&core, "/"), vec!["docs"]);
    assert_eq!(names(&core, "/docs"), vec!["a.txt"]);
    assert_eq!(core.read_file_path("/docs/a.txt").unwrap(), payload);

    let (inode, kind) = core.resolve_path("/docs/a.txt").unwrap().unwrap();
    assert_eq!(inode, file);
    assert_eq!(kind, EntryType::File);
}

#[test]
fn write_spanning_multiple_chunks_reassembles() {
    let dir = tempfile::tempdir().unwrap();
    let mut core = bootstrapped(dir.path(), 0xB2);
    core.chunk_size = 8;
    let id = Identity::generate();

    let create_op = signed_op(&id, 0xB2, 1, 1_000, create_file(ROOT_INODE, "big.bin"));
    core.apply_op(&create_op).unwrap();
    let file = op_inode(0xB2, 1);

    let payload: Vec<u8> = (0..=255u8).cycle().take(1000).collect();
    let write_op = signed_op(&id, 0xB2, 2, 2_000, write_all(&core, file, &payload));
    core.apply_op(&write_op).unwrap();

    assert_eq!(core.read_file_path("/big.bin").unwrap(), payload);
}

#[test]
fn applying_the_same_ops_in_any_order_converges() {
    // The property that makes replication meaningful: state is a function of the op
    // set, not of the order the ops happened to arrive in.
    let id = Identity::generate();

    let ops = vec![
        signed_op(&id, 0xC1, 1, 1_000, mkdir(ROOT_INODE, "docs")),
        signed_op(&id, 0xC1, 2, 2_000, create_file(op_inode(0xC1, 1), "a.txt")),
        signed_op(&id, 0xC1, 3, 3_000, mkdir(op_inode(0xC1, 1), "nested")),
        signed_op(&id, 0xC1, 4, 4_000, create_file(op_inode(0xC1, 3), "b.txt")),
    ];

    let forward_dir = tempfile::tempdir().unwrap();
    let forward = bootstrapped(forward_dir.path(), 0xC1);
    for op in &ops {
        forward.apply_op(op).unwrap();
    }

    // Reverse order: every op arrives before its parent exists, so all of them park
    // and then drain as the root-most op finally lands.
    let reverse_dir = tempfile::tempdir().unwrap();
    let reverse = bootstrapped(reverse_dir.path(), 0xC2);
    for op in ops.iter().rev() {
        reverse.apply_op(op).unwrap();
    }

    assert_eq!(reverse.pending_count().unwrap(), 0, "ops should all drain");
    assert_eq!(
        forward.compute_state_root().unwrap(),
        reverse.compute_state_root().unwrap(),
        "replicas with the same ops must agree on state"
    );
    assert_eq!(names(&forward, "/docs"), names(&reverse, "/docs"));
    assert_eq!(
        names(&forward, "/docs/nested"),
        names(&reverse, "/docs/nested")
    );
}

#[test]
fn op_with_missing_parent_is_parked_then_drains() {
    let dir = tempfile::tempdir().unwrap();
    let core = bootstrapped(dir.path(), 0xD1);
    let id = Identity::generate();

    let docs = op_inode(0xD1, 1);
    let child = signed_op(&id, 0xD1, 2, 2_000, create_file(docs, "a.txt"));

    match core.apply_op(&child).unwrap() {
        ApplyOutcome::Pending(reason) => assert!(reason.contains("parent")),
        other => panic!("expected the op to be parked, got {other:?}"),
    }
    assert_eq!(core.pending_count().unwrap(), 1);
    assert!(core.resolve_path("/docs").unwrap().is_none());

    // The parent arrives; the parked child must apply without being resubmitted.
    let parent = signed_op(&id, 0xD1, 1, 1_000, mkdir(ROOT_INODE, "docs"));
    core.apply_op(&parent).unwrap();

    assert_eq!(core.pending_count().unwrap(), 0);
    assert_eq!(names(&core, "/docs"), vec!["a.txt"]);
}

#[test]
fn write_with_unfetched_chunks_is_parked_not_applied() {
    // A write whose blobs have not arrived must not publish a file that reads back as
    // an error. Both the whole-file and partial paths have to behave the same way.
    let dir = tempfile::tempdir().unwrap();
    let core = bootstrapped(dir.path(), 0xC0);
    let id = Identity::generate();

    core.apply_op(&signed_op(
        &id,
        0xC0,
        1,
        1_000,
        create_file(ROOT_INODE, "f.txt"),
    ))
    .unwrap();
    let file = op_inode(0xC0, 1);

    // Chunk refs describing bytes this node has never seen.
    let payload = b"content that lives on another node";
    let orphan = FsOpKind::Write {
        inode: file,
        offset: 0,
        chunks: vec![ChunkRef {
            hash: blake3::hash(payload).into(),
            len: payload.len() as u32,
            plain_len: payload.len() as u32,
            offset: 0,
        }],
        new_size: payload.len() as u64,
        encryption: None,
    };

    match core
        .apply_op(&signed_op(&id, 0xC0, 2, 2_000, orphan))
        .unwrap()
    {
        ApplyOutcome::Pending(reason) => assert!(
            reason.contains("fetched"),
            "reason should name the missing chunk, got {reason:?}"
        ),
        other => panic!("expected the write to be parked, got {other:?}"),
    }

    assert_eq!(core.pending_count().unwrap(), 1);
    // The file is still visible and still empty — never in a half-written state.
    assert_eq!(names(&core, "/"), vec!["f.txt"]);
    assert_eq!(core.read_file_path("/f.txt").unwrap(), b"");

    // The blob arrives out of band, as blob transfer will deliver it.
    core.store_chunks(payload).unwrap();
    assert_eq!(core.retry_pending().unwrap(), 1);

    assert_eq!(core.pending_count().unwrap(), 0);
    assert_eq!(core.read_file_path("/f.txt").unwrap(), payload);
}

#[test]
fn retry_pending_is_a_no_op_when_nothing_is_parked() {
    let dir = tempfile::tempdir().unwrap();
    let core = bootstrapped(dir.path(), 0xC5);
    let id = Identity::generate();

    core.apply_op(&signed_op(&id, 0xC5, 1, 1_000, mkdir(ROOT_INODE, "d")))
        .unwrap();
    let root = core.compute_state_root().unwrap();
    let head = core.get_head().unwrap();

    assert_eq!(core.retry_pending().unwrap(), 0);
    assert_eq!(core.compute_state_root().unwrap(), root);
    assert_eq!(core.get_head().unwrap(), head);
}

#[test]
fn re_applying_an_op_changes_nothing() {
    let dir = tempfile::tempdir().unwrap();
    let core = bootstrapped(dir.path(), 0xE1);
    let id = Identity::generate();

    let op = signed_op(&id, 0xE1, 1, 1_000, mkdir(ROOT_INODE, "docs"));
    assert_eq!(core.apply_op(&op).unwrap(), ApplyOutcome::Applied);

    let head = core.get_head().unwrap();
    let root = core.compute_state_root().unwrap();
    let summary = core.clock_summary().unwrap().entries;

    assert_eq!(core.apply_op(&op).unwrap(), ApplyOutcome::AlreadyApplied);

    assert_eq!(core.get_head().unwrap(), head);
    assert_eq!(core.compute_state_root().unwrap(), root);
    assert_eq!(core.clock_summary().unwrap().entries, summary);
    assert_eq!(names(&core, "/"), vec!["docs"]);
}

#[test]
fn unsigned_and_tampered_ops_are_rejected() {
    let dir = tempfile::tempdir().unwrap();
    let core = bootstrapped(dir.path(), 0xE2);
    let id = Identity::generate();

    let mut unsigned = signed_op(&id, 0xE2, 1, 1_000, mkdir(ROOT_INODE, "docs"));
    unsigned.sig.clear();
    assert!(
        core.apply_op(&unsigned).is_err(),
        "unsigned op must not apply"
    );

    // Signature covers the op kind, so renaming the target invalidates it.
    let mut tampered = signed_op(&id, 0xE2, 2, 2_000, mkdir(ROOT_INODE, "docs"));
    tampered.kind = mkdir(ROOT_INODE, "evil");
    assert!(
        core.apply_op(&tampered).is_err(),
        "tampered op must not apply"
    );

    assert!(names(&core, "/").is_empty());
}

#[test]
fn concurrent_same_name_creates_get_deterministic_conflict_names() {
    // Two devices independently create /docs while partitioned. Both links survive;
    // exactly one keeps the plain name and every replica agrees which.
    let id = Identity::generate();
    let a = signed_op(&id, 0x01, 1, 1_000, mkdir(ROOT_INODE, "docs"));
    let b = signed_op(&id, 0x02, 1, 2_000, mkdir(ROOT_INODE, "docs"));

    let dir_ab = tempfile::tempdir().unwrap();
    let core_ab = bootstrapped(dir_ab.path(), 0x01);
    core_ab.apply_op(&a).unwrap();
    core_ab.apply_op(&b).unwrap();

    let dir_ba = tempfile::tempdir().unwrap();
    let core_ba = bootstrapped(dir_ba.path(), 0x02);
    core_ba.apply_op(&b).unwrap();
    core_ba.apply_op(&a).unwrap();

    let listing = names(&core_ab, "/");
    assert_eq!(listing.len(), 2, "both concurrent creates must survive");
    assert_eq!(listing, names(&core_ba, "/"), "orders must agree");

    // Dot ordering is (device, counter), so device 0x01 holds the plain name and
    // device 0x02 is the one renamed.
    assert!(listing.contains(&"docs".to_string()));
    assert!(listing.contains(&conflict_name("docs", 0x02, 2_000)));

    assert_eq!(
        core_ab.compute_state_root().unwrap(),
        core_ba.compute_state_root().unwrap()
    );
}

#[test]
fn rename_moves_an_entry_between_directories() {
    let dir = tempfile::tempdir().unwrap();
    let core = bootstrapped(dir.path(), 0xF1);
    let id = Identity::generate();

    core.apply_op(&signed_op(&id, 0xF1, 1, 1_000, mkdir(ROOT_INODE, "src")))
        .unwrap();
    core.apply_op(&signed_op(&id, 0xF1, 2, 2_000, mkdir(ROOT_INODE, "dst")))
        .unwrap();
    let src = op_inode(0xF1, 1);
    let dst = op_inode(0xF1, 2);

    core.apply_op(&signed_op(&id, 0xF1, 3, 3_000, create_file(src, "a.txt")))
        .unwrap();
    let file = op_inode(0xF1, 3);
    core.apply_op(&signed_op(
        &id,
        0xF1,
        4,
        4_000,
        write_all(&core, file, b"payload"),
    ))
    .unwrap();

    core.apply_op(&signed_op(
        &id,
        0xF1,
        5,
        5_000,
        rename(src, "a.txt", dst, "b.txt"),
    ))
    .unwrap();

    assert!(names(&core, "/src").is_empty());
    assert_eq!(names(&core, "/dst"), vec!["b.txt"]);
    // The inode moved rather than being copied, so content follows the rename.
    assert_eq!(core.read_file_path("/dst/b.txt").unwrap(), b"payload");
}

#[test]
fn rename_after_unlink_is_a_no_op() {
    // Per docs/ops_semantics.md: a rename that is causally behind an unlink must not
    // resurrect the entry.
    let dir = tempfile::tempdir().unwrap();
    let core = bootstrapped(dir.path(), 0xF2);
    let id = Identity::generate();

    core.apply_op(&signed_op(&id, 0xF2, 1, 1_000, mkdir(ROOT_INODE, "d")))
        .unwrap();
    let d = op_inode(0xF2, 1);
    core.apply_op(&signed_op(&id, 0xF2, 2, 2_000, create_file(d, "a.txt")))
        .unwrap();

    core.apply_op(&signed_op(&id, 0xF2, 3, 3_000, unlink(d, "a.txt")))
        .unwrap();
    assert!(names(&core, "/d").is_empty());

    core.apply_op(&signed_op(
        &id,
        0xF2,
        4,
        4_000,
        rename(d, "a.txt", d, "b.txt"),
    ))
    .unwrap();
    assert!(
        names(&core, "/d").is_empty(),
        "rename must not resurrect an unlinked entry"
    );
}

#[test]
fn moving_a_directory_into_its_own_subtree_is_refused() {
    let dir = tempfile::tempdir().unwrap();
    let core = bootstrapped(dir.path(), 0xF3);
    let id = Identity::generate();

    core.apply_op(&signed_op(&id, 0xF3, 1, 1_000, mkdir(ROOT_INODE, "outer")))
        .unwrap();
    let outer = op_inode(0xF3, 1);
    core.apply_op(&signed_op(&id, 0xF3, 2, 2_000, mkdir(outer, "inner")))
        .unwrap();
    let inner = op_inode(0xF3, 2);

    let bad = signed_op(
        &id,
        0xF3,
        3,
        3_000,
        rename(ROOT_INODE, "outer", inner, "outer"),
    );
    assert!(
        core.apply_op(&bad).is_err(),
        "moving a directory beneath itself would detach the subtree"
    );
}

#[test]
fn concurrent_writes_resolve_to_one_winner_on_every_replica() {
    let id = Identity::generate();
    let create = signed_op(&id, 0x11, 1, 1_000, create_file(ROOT_INODE, "f.txt"));
    let file = op_inode(0x11, 1);

    let build = |device: u128, seed: &str| {
        let dir = tempfile::tempdir().unwrap();
        let core = bootstrapped(dir.path(), device);
        core.apply_op(&create).unwrap();
        // Same timestamp on both writes, so the tie-break must be deterministic.
        let w1 = signed_op(&id, 0x11, 2, 5_000, write_all(&core, file, b"from-one"));
        let w2 = signed_op(&id, 0x22, 2, 5_000, write_all(&core, file, b"from-two"));
        let ops = if seed == "forward" {
            vec![w1, w2]
        } else {
            vec![w2, w1]
        };
        for op in &ops {
            core.apply_op(op).unwrap();
        }
        (
            core.read_file_path("/f.txt").unwrap(),
            core.compute_state_root().unwrap(),
            dir,
        )
    };

    let (bytes_a, root_a, _da) = build(0x11, "forward");
    let (bytes_b, root_b, _db) = build(0x22, "reverse");

    assert_eq!(bytes_a, bytes_b, "both replicas must pick the same winner");
    assert_eq!(root_a, root_b);
    // Higher writer_id wins the timestamp tie.
    assert_eq!(bytes_a, b"from-two");
}

#[test]
fn namespace_survives_reopen() {
    let dir = tempfile::tempdir().unwrap();
    let id = Identity::generate();

    let (head, root) = {
        let core = bootstrapped(dir.path(), 0x99);
        core.apply_op(&signed_op(&id, 0x99, 1, 1_000, mkdir(ROOT_INODE, "docs")))
            .unwrap();
        let docs = op_inode(0x99, 1);
        core.apply_op(&signed_op(&id, 0x99, 2, 2_000, create_file(docs, "a.txt")))
            .unwrap();
        let file = op_inode(0x99, 2);
        core.apply_op(&signed_op(
            &id,
            0x99,
            3,
            3_000,
            write_all(&core, file, b"persisted"),
        ))
        .unwrap();
        (core.get_head().unwrap(), core.compute_state_root().unwrap())
    };

    let reopened = open_core(dir.path(), 0x99);
    assert_eq!(reopened.get_head().unwrap(), head);
    assert_eq!(reopened.compute_state_root().unwrap(), root);
    assert_eq!(names(&reopened, "/docs"), vec!["a.txt"]);
    assert_eq!(
        reopened.read_file_path("/docs/a.txt").unwrap(),
        b"persisted"
    );
}

#[test]
fn setattr_updates_mode() {
    let dir = tempfile::tempdir().unwrap();
    let core = bootstrapped(dir.path(), 0xAB);
    let id = Identity::generate();

    core.apply_op(&signed_op(
        &id,
        0xAB,
        1,
        1_000,
        create_file(ROOT_INODE, "f"),
    ))
    .unwrap();
    let file = op_inode(0xAB, 1);

    core.apply_op(&signed_op(
        &id,
        0xAB,
        2,
        2_000,
        FsOpKind::SetAttr {
            inode: file,
            mode: Some(0o100600),
            uid: None,
            gid: None,
        },
    ))
    .unwrap();

    let record = core.load_inode(file).unwrap().unwrap();
    assert_eq!(record.attrs.value.mode, 0o100600);
}

#[test]
fn invalid_names_are_rejected() {
    let dir = tempfile::tempdir().unwrap();
    let core = bootstrapped(dir.path(), 0xAC);
    let id = Identity::generate();

    for bad in ["", "a/b", ".", ".."] {
        let op = signed_op(&id, 0xAC, 1, 1_000, mkdir(ROOT_INODE, bad));
        assert!(core.apply_op(&op).is_err(), "name {bad:?} must be rejected");
    }
    assert!(core.resolve_path("/../etc").is_err());
}

#[test]
fn content_change_moves_the_state_root() {
    // A DirNode commits to inode ids, not bytes, so only the inode-map commitment can
    // make a pure content edit visible in the head.
    let dir = tempfile::tempdir().unwrap();
    let core = bootstrapped(dir.path(), 0xAD);
    let id = Identity::generate();

    core.apply_op(&signed_op(
        &id,
        0xAD,
        1,
        1_000,
        create_file(ROOT_INODE, "f"),
    ))
    .unwrap();
    let file = op_inode(0xAD, 1);
    core.apply_op(&signed_op(
        &id,
        0xAD,
        2,
        2_000,
        write_all(&core, file, b"one"),
    ))
    .unwrap();
    let before = core.compute_state_root().unwrap();

    core.apply_op(&signed_op(
        &id,
        0xAD,
        3,
        3_000,
        write_all(&core, file, b"two"),
    ))
    .unwrap();
    let after = core.compute_state_root().unwrap();

    assert_ne!(before, after, "content edits must change the state root");
}
