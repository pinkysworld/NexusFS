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
fn clock_summary_reports_the_highest_contiguous_counter() {
    // The summary is what peers diff against, so it must mean "I have everything up
    // to N" and not "the largest number I have seen". Claiming a counter above a gap
    // would cause the peer to skip the missing operations permanently.
    let dir = tempfile::tempdir().unwrap();
    let core = bootstrapped(dir.path(), 0x0a);
    let id = Identity::generate();

    // Device 0x0a holds a complete run: 1, 2.
    core.apply_op(&signed_op(&id, 0x0a, 1, 1_000, mkdir(ROOT_INODE, "one")))
        .unwrap();
    core.apply_op(&signed_op(&id, 0x0a, 2, 2_000, mkdir(ROOT_INODE, "two")))
        .unwrap();
    // Device 0x0b holds only counter 7 — everything below it is missing.
    core.apply_op(&signed_op(&id, 0x0b, 7, 3_000, mkdir(ROOT_INODE, "three")))
        .unwrap();

    let entries = core.clock_summary().unwrap().entries;
    assert!(entries.iter().any(|(d, c)| d.0 == 0x0a && *c == 2));
    assert!(
        entries.iter().any(|(d, c)| d.0 == 0x0b && *c == 0),
        "a device with a gap at 1 has no contiguous prefix, so it must claim 0"
    );
    assert_eq!(core.applied_count().unwrap(), 3);
}

#[test]
fn ops_missing_for_respects_what_the_peer_already_has() {
    let dir = tempfile::tempdir().unwrap();
    let core = bootstrapped(dir.path(), 0x0c);
    let id = Identity::generate();

    for counter in 1..=4 {
        core.apply_op(&signed_op(
            &id,
            0x0c,
            counter,
            counter * 1_000,
            mkdir(ROOT_INODE, &format!("d{counter}")),
        ))
        .unwrap();
    }

    // A peer that has nothing should be offered everything.
    let empty = nexusfs_proto::ClockSummary::default();
    assert_eq!(core.ops_missing_for(&empty, 100).unwrap().len(), 4);

    // A peer already holding the first two should be offered only the last two.
    let partial = nexusfs_proto::ClockSummary {
        entries: vec![(nexusfs_proto::DeviceId(0x0c), 2)],
        above: vec![],
    };
    let missing = core.ops_missing_for(&partial, 100).unwrap();
    assert_eq!(missing.len(), 2);
    assert!(missing.iter().all(|op| op.id.counter > 2));

    // A caught-up peer should be offered nothing.
    let caught_up = nexusfs_proto::ClockSummary {
        entries: vec![(nexusfs_proto::DeviceId(0x0c), 4)],
        above: vec![],
    };
    assert!(core.ops_missing_for(&caught_up, 100).unwrap().is_empty());

    // The limit bounds a batch.
    assert_eq!(core.ops_missing_for(&empty, 3).unwrap().len(), 3);

    // A peer holding operations *above* a gap is not offered them again. Without this
    // an operation it has permanently refused would pin its watermark, and every round
    // would re-send the identical window while everything past it stayed unreachable.
    let with_gap = nexusfs_proto::ClockSummary {
        entries: vec![(nexusfs_proto::DeviceId(0x0c), 0)],
        above: vec![(nexusfs_proto::DeviceId(0x0c), vec![2, 3, 4])],
    };
    let missing = core.ops_missing_for(&with_gap, 100).unwrap();
    assert_eq!(missing.len(), 1, "only the gap itself should be offered");
    assert_eq!(missing[0].id.counter, 1);
}
