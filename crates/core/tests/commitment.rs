//! Commitment proofs.
//!
//! The property that separates this mode from transparent proofs: a claim about state
//! is checkable by someone holding nothing but the claim and the root. So most of these
//! verify *without* the repository that produced them.

mod common;

use common::*;
use nexusfs_core::{decode_commit, ProofPolicy};
use nexusfs_crypto::Identity;
use nexusfs_proto::ProofMode;

fn committing(path: &std::path::Path, device: u128) -> nexusfs_core::CoreState {
    let core = open_core(path, device).with_proofs(ProofPolicy::Commit);
    core.bootstrap_if_needed().unwrap();
    core
}

#[test]
fn a_local_operation_carries_a_checkable_inclusion_path() {
    let dir = tempfile::tempdir().unwrap();
    let core = committing(dir.path(), 0xA1);
    let id = Identity::generate();

    let op = core.mkdir_p(&id, "/docs", 1_000).unwrap();
    let _ = op;

    let latest = core.all_ops().unwrap().pop().unwrap();
    let bundle = latest.proof.expect("commit policy must attach evidence");
    assert_eq!(bundle.mode, ProofMode::ZkCommit);

    let proof = decode_commit(&bundle.bytes).unwrap();
    assert_eq!(proof.new_root, core.compute_state_root().unwrap());

    // The whole point: this holds with no repository in hand.
    nexusfs_zk::merkle::check(&proof.entry, &proof.new_root).unwrap();
}

#[test]
fn a_proof_verifies_against_nothing_but_the_root() {
    // Serialize the proof, drop the repository entirely, and check it from the bytes.
    let root_and_bytes = {
        let dir = tempfile::tempdir().unwrap();
        let core = committing(dir.path(), 0xA1);
        let id = Identity::generate();
        core.write_file(&id, "/a.txt", b"content", 1_000).unwrap();

        let latest = core.all_ops().unwrap().pop().unwrap();
        let bundle = latest.proof.unwrap();
        (core.compute_state_root().unwrap(), bundle.bytes)
    };

    let (root, bytes) = root_and_bytes;
    let proof = decode_commit(&bytes).unwrap();
    assert_eq!(proof.new_root, root);
    assert!(proof.entry.verify(&root));
}

#[test]
fn a_proof_against_the_wrong_root_is_refused() {
    let dir = tempfile::tempdir().unwrap();
    let core = committing(dir.path(), 0xA1);
    let id = Identity::generate();
    core.write_file(&id, "/a.txt", b"first", 1_000).unwrap();

    let early =
        decode_commit(&core.all_ops().unwrap().pop().unwrap().proof.unwrap().bytes).unwrap();

    // Move the state on; the earlier proof must not verify against the new root.
    core.write_file(&id, "/b.txt", b"second", 2_000).unwrap();
    let now = core.compute_state_root().unwrap();

    assert_ne!(early.new_root, now);
    assert!(!early.entry.verify(&now));
}

#[test]
fn a_tampered_path_is_rejected_on_receipt() {
    let dir = tempfile::tempdir().unwrap();
    let core = committing(dir.path(), 0xA1);
    let id = Identity::generate();
    core.mkdir_p(&id, "/docs", 1_000).unwrap();

    let mut op = core.all_ops().unwrap().pop().unwrap();
    let mut proof = decode_commit(&op.proof.as_ref().unwrap().bytes).unwrap();
    proof.entry.value[0] ^= 0xff;
    op.proof = Some(nexusfs_proto::ProofBundle {
        mode: ProofMode::ZkCommit,
        bytes: nexusfs_core::encode_commit(&proof).unwrap(),
    });

    let err = core
        .check_proof(&op, ProofPolicy::Commit)
        .unwrap_err()
        .to_string();
    assert!(
        err.contains("inclusion path"),
        "the receiver must say what failed: {err}"
    );
}

#[test]
fn a_transparent_proof_is_still_accepted_under_commit_policy() {
    // The fallback that keeps a mixed cluster working: a transparent proof proves less,
    // but it is not wrong, and refusing it would make the mode unusable during rollout.
    let transparent_dir = tempfile::tempdir().unwrap();
    let transparent = open_core(transparent_dir.path(), 0xB2).with_proofs(ProofPolicy::Transparent);
    transparent.bootstrap_if_needed().unwrap();
    let id = Identity::generate();
    transparent.mkdir_p(&id, "/docs", 1_000).unwrap();
    let op = transparent.all_ops().unwrap().pop().unwrap();
    assert_eq!(op.proof.as_ref().unwrap().mode, ProofMode::Transparent);

    let strict_dir = tempfile::tempdir().unwrap();
    let strict = committing(strict_dir.path(), 0xA1);
    strict.check_proof(&op, ProofPolicy::Commit).unwrap();
}

#[test]
fn an_unlink_falls_back_rather_than_proving_the_wrong_entry() {
    // An unlink's subject is the parent directory, which still exists — but if the
    // subject were ever absent, emitting a proof of some other entry would be worse
    // than emitting a weaker one. The fallback is what makes that impossible.
    let dir = tempfile::tempdir().unwrap();
    let core = committing(dir.path(), 0xA1);
    let id = Identity::generate();

    core.write_file(&id, "/gone.txt", b"temporary", 1_000)
        .unwrap();
    core.remove_path(&id, "/gone.txt", 2_000).unwrap();

    let op = core.all_ops().unwrap().pop().unwrap();
    let bundle = op.proof.clone().expect("evidence is still attached");
    // Either mode is acceptable; what matters is that whatever it claims, it checks.
    core.check_proof(&op, ProofPolicy::Commit).unwrap();
    if bundle.mode == ProofMode::ZkCommit {
        let proof = decode_commit(&bundle.bytes).unwrap();
        assert!(proof.entry.verify(&core.compute_state_root().unwrap()));
    }
}

#[test]
fn every_live_entry_can_prove_itself() {
    let dir = tempfile::tempdir().unwrap();
    let core = committing(dir.path(), 0xA1);
    let id = Identity::generate();

    core.mkdir_p(&id, "/a/b/c", 1_000).unwrap();
    for i in 0..7 {
        core.write_file(
            &id,
            &format!("/a/b/c/f{i}.txt"),
            format!("{i}").as_bytes(),
            2_000 + i,
        )
        .unwrap();
    }

    let root = core.compute_state_root().unwrap();
    let map = core.inode_map().unwrap();
    assert!(map.len() >= 8);

    for (inode, _) in &map {
        let proof = core
            .inclusion_proof(*inode)
            .unwrap()
            .expect("a live entry must be provable");
        assert!(proof.verify(&root), "inode {inode:x} failed");
    }
}
