mod common;

use std::sync::Arc;

use common::*;
use nexusfs_core::{now_ms, Object, ProofPolicy, ROOT_INODE};
use nexusfs_crypto::{Identity, RepoCipher};

fn encrypted_node(path: &std::path::Path, device: u128, key: [u8; 32]) -> nexusfs_core::CoreState {
    bootstrapped(path, device).with_encryption(Arc::new(RepoCipher::new(key)))
}

#[test]
fn encrypted_content_round_trips() {
    let dir = tempfile::tempdir().unwrap();
    let core = encrypted_node(dir.path(), 0x01, [9u8; 32]);
    let id = Identity::generate();

    let secret = b"the quick brown fox jumps over the lazy dog";
    core.write_file(&id, "/notes/secret.txt", secret, now_ms())
        .unwrap();

    assert_eq!(core.read_file_path("/notes/secret.txt").unwrap(), secret);
}

#[test]
fn plaintext_is_not_present_on_disk() {
    // The point of at-rest encryption: the stored bytes must not contain the content.
    let dir = tempfile::tempdir().unwrap();
    let core = encrypted_node(dir.path(), 0x02, [9u8; 32]);
    let id = Identity::generate();

    let secret = b"SUPERSECRETMARKERVALUE";
    core.write_file(&id, "/s.txt", secret, now_ms()).unwrap();

    let (inode, _, _) = core.stat_file("/s.txt").unwrap().unwrap();
    let record = core.load_inode(inode).unwrap().unwrap();
    let node_hash = record.content.value.node_hash.unwrap();

    let Some(Object::FileNode(file)) = core.get_object(&node_hash).unwrap() else {
        panic!("expected a file node");
    };
    assert!(file.encryption.is_some(), "the file should record a key");

    for chunk in &file.chunks {
        let stored = core.stores.blobs.get(&chunk.hash).unwrap().unwrap();
        assert!(
            !stored.windows(secret.len()).any(|w| w == secret),
            "plaintext found in a stored chunk"
        );
    }
}

#[test]
fn chunks_are_addressed_by_ciphertext_so_peers_can_verify() {
    // Replication checks hash(received) == requested without holding any key. That
    // only works if the hash names the bytes as stored.
    let dir = tempfile::tempdir().unwrap();
    let core = encrypted_node(dir.path(), 0x03, [9u8; 32]);
    let id = Identity::generate();

    core.write_file(&id, "/f.txt", b"content", now_ms())
        .unwrap();

    let (inode, _, _) = core.stat_file("/f.txt").unwrap().unwrap();
    let record = core.load_inode(inode).unwrap().unwrap();
    let Some(Object::FileNode(file)) = core
        .get_object(&record.content.value.node_hash.unwrap())
        .unwrap()
    else {
        panic!("expected a file node");
    };

    for chunk in &file.chunks {
        let stored = core.stores.blobs.get(&chunk.hash).unwrap().unwrap();
        assert_eq!(
            nexusfs_core::hash_bytes(&stored),
            chunk.hash,
            "the chunk hash must name the bytes as stored"
        );
        assert_eq!(stored.len(), chunk.len as usize);
    }
}

#[test]
fn the_wrong_repository_key_cannot_read() {
    let dir = tempfile::tempdir().unwrap();
    let id = Identity::generate();

    {
        let core = encrypted_node(dir.path(), 0x04, [1u8; 32]);
        core.write_file(&id, "/f.txt", b"private", now_ms())
            .unwrap();
        assert_eq!(core.read_file_path("/f.txt").unwrap(), b"private");
    }

    // Same store, different repository key. Scoped because sled allows one handle at
    // a time per directory.
    {
        let wrong =
            open_core(dir.path(), 0x04).with_encryption(Arc::new(RepoCipher::new([2u8; 32])));
        assert!(
            wrong.read_file_path("/f.txt").is_err(),
            "a different key must not decrypt"
        );
    }

    // And with no key at all.
    let none = open_core(dir.path(), 0x04);
    assert!(
        none.read_file_path("/f.txt").is_err(),
        "an encrypted file must not be readable without a key"
    );
}

#[test]
fn tampered_ciphertext_fails_authentication() {
    let dir = tempfile::tempdir().unwrap();
    let core = encrypted_node(dir.path(), 0x05, [3u8; 32]);
    let id = Identity::generate();
    core.write_file(&id, "/f.txt", b"authentic", now_ms())
        .unwrap();

    let (inode, _, _) = core.stat_file("/f.txt").unwrap().unwrap();
    let record = core.load_inode(inode).unwrap().unwrap();
    let Some(Object::FileNode(file)) = core
        .get_object(&record.content.value.node_hash.unwrap())
        .unwrap()
    else {
        panic!("expected a file node");
    };

    // Flip a bit in the stored ciphertext, keeping the same length.
    let chunk = file.chunks[0];
    let mut stored = core.stores.blobs.get(&chunk.hash).unwrap().unwrap();
    stored[0] ^= 0xff;
    core.stores.blobs.put(chunk.hash, &stored).unwrap();

    assert!(
        core.read_file_path("/f.txt").is_err(),
        "AEAD must reject altered ciphertext rather than return garbage"
    );
}

#[test]
fn every_write_uses_a_fresh_key() {
    // Nonces are derived from (file key, chunk index), so reusing a key across writes
    // would reuse a nonce — the one thing that breaks this AEAD outright.
    let dir = tempfile::tempdir().unwrap();
    let core = encrypted_node(dir.path(), 0x06, [4u8; 32]);
    let id = Identity::generate();

    let sealed_key = |core: &nexusfs_core::CoreState| {
        let (inode, _, _) = core.stat_file("/f.txt").unwrap().unwrap();
        let record = core.load_inode(inode).unwrap().unwrap();
        let Some(Object::FileNode(file)) = core
            .get_object(&record.content.value.node_hash.unwrap())
            .unwrap()
        else {
            panic!("expected a file node");
        };
        file.encryption.unwrap().sealed_key
    };

    core.write_file(&id, "/f.txt", b"first", now_ms()).unwrap();
    let first = sealed_key(&core);
    core.write_file(&id, "/f.txt", b"second", now_ms()).unwrap();
    let second = sealed_key(&core);

    assert_ne!(first, second, "each write must mint a new file key");
    assert_eq!(core.read_file_path("/f.txt").unwrap(), b"second");
}

#[test]
fn plaintext_files_written_before_encryption_stay_readable() {
    // Whether a file is encrypted is recorded on the file, not the node, so turning
    // encryption on must not strand existing content.
    let dir = tempfile::tempdir().unwrap();
    let id = Identity::generate();

    {
        let plain = bootstrapped(dir.path(), 0x07);
        plain
            .write_file(&id, "/old.txt", b"written in the clear", now_ms())
            .unwrap();
    }

    let encrypted =
        open_core(dir.path(), 0x07).with_encryption(Arc::new(RepoCipher::new([5u8; 32])));
    assert_eq!(
        encrypted.read_file_path("/old.txt").unwrap(),
        b"written in the clear"
    );

    encrypted
        .write_file(&id, "/new.txt", b"written encrypted", now_ms())
        .unwrap();
    assert_eq!(
        encrypted.read_file_path("/new.txt").unwrap(),
        b"written encrypted"
    );
}

#[test]
fn multi_chunk_encrypted_files_reassemble() {
    let dir = tempfile::tempdir().unwrap();
    let mut core = encrypted_node(dir.path(), 0x08, [6u8; 32]);
    core.chunk_size = 64;
    let id = Identity::generate();

    let payload: Vec<u8> = (0..=255u8).cycle().take(5000).collect();
    core.write_file(&id, "/big.bin", &payload, now_ms())
        .unwrap();

    assert_eq!(core.read_file_path("/big.bin").unwrap(), payload);
}

// --- proofs -----------------------------------------------------------------

#[test]
fn local_operations_carry_transparent_proofs() {
    let dir = tempfile::tempdir().unwrap();
    let core = bootstrapped(dir.path(), 0x10).with_proofs(ProofPolicy::Transparent);
    let id = Identity::generate();

    core.mkdir_p(&id, "/docs", now_ms()).unwrap();
    core.write_file(&id, "/docs/a.txt", b"proven", now_ms())
        .unwrap();

    let ops = core.all_ops().unwrap();
    assert!(!ops.is_empty());
    for op in &ops {
        let bundle = op.proof.as_ref().expect("every local op should be proven");
        let proof = nexusfs_core::proof::decode_proof(&bundle.bytes).unwrap();
        assert!(proof.new_root.is_some(), "a proof must record the new root");
        // The signature must cover the proof, or the evidence could be swapped.
        core.verify_op(op).unwrap();
    }
}

#[test]
fn a_malformed_proof_is_rejected() {
    let dir = tempfile::tempdir().unwrap();
    let core = bootstrapped(dir.path(), 0x11).with_proofs(ProofPolicy::Transparent);
    let id = Identity::generate();

    let mut op = core
        .make_op(&id, mkdir(ROOT_INODE, "docs"), now_ms())
        .unwrap();
    op.proof = Some(nexusfs_proto::ProofBundle {
        mode: nexusfs_proto::ProofMode::Transparent,
        bytes: vec![0xff; 8], // not a valid encoding
    });
    op.sig = nexusfs_crypto::sign(id.signing_key(), &op.signing_bytes().unwrap());

    assert!(
        core.apply_op(&op).is_err(),
        "malformed evidence must be refused, not ignored"
    );
    assert!(names(&core, "/").is_empty());
}

#[test]
fn an_unsupported_proof_mode_is_rejected() {
    let dir = tempfile::tempdir().unwrap();
    let core = bootstrapped(dir.path(), 0x12).with_proofs(ProofPolicy::Transparent);
    let id = Identity::generate();

    let mut op = core
        .make_op(&id, mkdir(ROOT_INODE, "docs"), now_ms())
        .unwrap();
    op.proof = Some(nexusfs_proto::ProofBundle {
        mode: nexusfs_proto::ProofMode::ZkFull,
        bytes: vec![],
    });
    op.sig = nexusfs_crypto::sign(id.signing_key(), &op.signing_bytes().unwrap());

    assert!(
        core.apply_op(&op).is_err(),
        "a mode this build cannot check must not be accepted as proven"
    );
}

#[test]
fn required_policy_refuses_unproven_operations() {
    let dir = tempfile::tempdir().unwrap();
    let core = bootstrapped(dir.path(), 0x13).with_proofs(ProofPolicy::Required);
    let id = Identity::generate();

    // Signed correctly, but carries no evidence.
    let op = core
        .make_op(&id, mkdir(ROOT_INODE, "docs"), now_ms())
        .unwrap();
    assert!(op.proof.is_none());
    assert!(core.apply_op(&op).is_err());

    // The same node's own writes go through, because it attaches proofs.
    core.mkdir_p(&id, "/allowed", now_ms()).unwrap();
    assert_eq!(names(&core, "/"), vec!["allowed"]);
}

#[test]
fn unproven_operations_are_accepted_when_policy_is_lenient() {
    let dir = tempfile::tempdir().unwrap();
    let core = bootstrapped(dir.path(), 0x14).with_proofs(ProofPolicy::Transparent);
    let id = Identity::generate();

    let op = core
        .make_op(&id, mkdir(ROOT_INODE, "legacy"), now_ms())
        .unwrap();
    core.apply_op(&op).unwrap();
    assert_eq!(names(&core, "/"), vec!["legacy"]);
}

#[test]
fn verify_reports_a_healthy_repository() {
    let dir = tempfile::tempdir().unwrap();
    let core = encrypted_node(dir.path(), 0x15, [7u8; 32]).with_proofs(ProofPolicy::Transparent);
    let id = Identity::generate();

    core.mkdir_p(&id, "/docs", now_ms()).unwrap();
    core.write_file(&id, "/docs/a.txt", b"one", now_ms())
        .unwrap();
    core.write_file(&id, "/docs/b.txt", b"two", now_ms())
        .unwrap();

    let report = core.verify_repository().unwrap();
    assert!(report.ok(), "expected a clean report, got {report:?}");
    assert_eq!(report.without_proof, 0);
    assert_eq!(report.malformed, 0);
    assert_eq!(report.signature_failures, 0);
    assert!(report.unreadable_files.is_empty());
    assert!(report.operations >= 3);
}

#[test]
fn verify_notices_content_it_cannot_read() {
    let dir = tempfile::tempdir().unwrap();
    let core = encrypted_node(dir.path(), 0x16, [8u8; 32]).with_proofs(ProofPolicy::Transparent);
    let id = Identity::generate();
    core.write_file(&id, "/f.txt", b"content", now_ms())
        .unwrap();

    // Delete the chunk out from under the file.
    let (inode, _, _) = core.stat_file("/f.txt").unwrap().unwrap();
    let record = core.load_inode(inode).unwrap().unwrap();
    let Some(Object::FileNode(file)) = core
        .get_object(&record.content.value.node_hash.unwrap())
        .unwrap()
    else {
        panic!("expected a file node");
    };
    core.stores.blobs.delete(&file.chunks[0].hash).unwrap();

    let report = core.verify_repository().unwrap();
    assert!(!report.ok(), "a missing chunk must fail verification");
    assert_eq!(report.unreadable_files, vec!["/f.txt".to_string()]);
}
