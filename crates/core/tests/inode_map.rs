//! The incremental inode map must never disagree with a full walk.
//!
//! The map is what the state root commits to, so a wrong entry is not a slow path or a
//! stale cache — it is two replicas silently disagreeing about what the filesystem is.
//! Every test here re-derives the map the expensive way and demands equality.

mod common;

use common::*;
use nexusfs_core::{inode_for_op, ROOT_INODE};
use nexusfs_crypto::Identity;
use nexusfs_proto::{DeviceId, OpId};

fn op_inode(device: u128, counter: u64) -> u128 {
    inode_for_op(OpId {
        device_id: DeviceId(device),
        counter,
    })
}

/// The map as a full walk produces it, ignoring whatever is cached.
fn walked(core: &nexusfs_core::CoreState) -> Vec<(u128, [u8; 32])> {
    core.rebuild_inode_map().unwrap()
}

fn assert_agrees(core: &nexusfs_core::CoreState, note: &str) {
    let incremental = core.inode_map().unwrap();
    let full = walked(core);
    assert_eq!(
        incremental, full,
        "the incremental map disagrees with a full walk after {note}"
    );
}

#[test]
fn creates_and_writes_keep_the_map_exact() {
    let dir = tempfile::tempdir().unwrap();
    let core = bootstrapped(dir.path(), 0xA1);
    let id = Identity::generate();

    core.mkdir_p(&id, "/a/b", 1_000).unwrap();
    assert_agrees(&core, "nested mkdir");

    for i in 0..6 {
        core.write_file(
            &id,
            &format!("/a/b/f{i}.txt"),
            format!("body {i}").as_bytes(),
            2_000 + i,
        )
        .unwrap();
        assert_agrees(&core, &format!("write {i}"));
    }

    // Overwrites: the hot path the incremental map exists for.
    for i in 0..6 {
        core.write_file(&id, &format!("/a/b/f{i}.txt"), b"replaced", 3_000 + i)
            .unwrap();
        assert_agrees(&core, &format!("overwrite {i}"));
    }
}

#[test]
fn removals_shrink_the_map_exactly() {
    let dir = tempfile::tempdir().unwrap();
    let core = bootstrapped(dir.path(), 0xA1);
    let id = Identity::generate();

    core.mkdir_p(&id, "/keep", 1_000).unwrap();
    core.mkdir_p(&id, "/doomed/inner", 1_001).unwrap();
    core.write_file(&id, "/doomed/inner/deep.txt", b"content", 1_002)
        .unwrap();
    core.write_file(&id, "/keep/stays.txt", b"content", 1_003)
        .unwrap();
    let before = core.inode_map().unwrap().len();

    // Removing a whole subtree must drop every inode beneath it, not just the entry.
    core.remove_path(&id, "/doomed", 2_000).unwrap();
    assert_agrees(&core, "subtree removal");
    assert!(
        core.inode_map().unwrap().len() < before,
        "the map should have shrunk"
    );
    assert_eq!(core.read_file_path("/keep/stays.txt").unwrap(), b"content");
}

#[test]
fn renames_keep_the_map_exact() {
    let dir = tempfile::tempdir().unwrap();
    let core = bootstrapped(dir.path(), 0xA1);
    let id = Identity::generate();

    core.mkdir_p(&id, "/src", 1_000).unwrap();
    core.mkdir_p(&id, "/dst", 1_001).unwrap();
    core.write_file(&id, "/src/f.txt", b"payload", 1_002)
        .unwrap();

    core.rename_path(&id, "/src/f.txt", "/dst/g.txt", 2_000)
        .unwrap();
    assert_agrees(&core, "rename between directories");
    assert_eq!(core.read_file_path("/dst/g.txt").unwrap(), b"payload");
}

#[test]
fn work_inside_a_removed_directory_stays_out_of_the_map() {
    // The case an incremental map gets wrong if it trusts the parent blindly: an
    // operation whose parent has been unlinked adds nothing reachable, so it must add
    // nothing to the map either.
    let dir = tempfile::tempdir().unwrap();
    let core = bootstrapped(dir.path(), 0xA1);
    let id = Identity::generate();

    let create_dir = signed_op(&id, 0xA1, 1, 1_000, mkdir(ROOT_INODE, "gone"));
    core.apply_op(&create_dir).unwrap();
    let gone = op_inode(0xA1, 1);

    core.apply_op(&signed_op(
        &id,
        0xA1,
        2,
        2_000,
        unlink(ROOT_INODE, "gone", &[op_id(0xA1, 1)]),
    ))
    .unwrap();
    assert_agrees(&core, "removing the directory");

    // A creation inside the detached directory, arriving late.
    core.apply_op(&signed_op(
        &id,
        0xA1,
        3,
        3_000,
        create_file(gone, "orphan.txt"),
    ))
    .unwrap();
    assert_agrees(&core, "a create inside a removed directory");

    let orphan = op_inode(0xA1, 3);
    core.apply_op(&signed_op(
        &id,
        0xA1,
        4,
        4_000,
        write_all(&core, orphan, b"data"),
    ))
    .unwrap();
    assert_agrees(&core, "a write inside a removed directory");
}

#[test]
fn a_parked_operation_draining_keeps_the_map_exact() {
    // Draining applies operations outside the normal path, so the map maintenance has
    // to happen there too.
    let dir = tempfile::tempdir().unwrap();
    let core = bootstrapped(dir.path(), 0xA1);
    let id = Identity::generate();

    let parent = op_inode(0xA1, 1);
    // Child first: it parks until its parent shows up.
    core.apply_op(&signed_op(
        &id,
        0xA1,
        2,
        2_000,
        create_file(parent, "child.txt"),
    ))
    .unwrap();
    assert_agrees(&core, "a parked create");

    core.apply_op(&signed_op(&id, 0xA1, 1, 1_000, mkdir(ROOT_INODE, "parent")))
        .unwrap();
    assert_agrees(&core, "the parent arriving and the child draining");
    assert_eq!(names(&core, "/parent"), vec!["child.txt".to_string()]);
}

#[test]
fn the_map_survives_a_restart() {
    let dir = tempfile::tempdir().unwrap();
    let id = Identity::generate();
    {
        let core = bootstrapped(dir.path(), 0xA1);
        core.mkdir_p(&id, "/a", 1_000).unwrap();
        core.write_file(&id, "/a/f.txt", b"content", 1_001).unwrap();
        assert_agrees(&core, "before restart");
    }
    let core = bootstrapped(dir.path(), 0xA1);
    assert_agrees(&core, "after restart");
}
