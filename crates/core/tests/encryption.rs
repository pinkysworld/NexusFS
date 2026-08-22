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

// ---- per-recipient sealing ---------------------------------------------------------
//
// The property these exist for: with recipients, a peer that holds the ciphertext and
// every stored record but no envelope addressed to it genuinely cannot read the content.
// That is the difference between "encrypted at rest" and "one peer protected from
// another", and it is the whole reason envelopes were built.

/// A node that encrypts and seals to recipients, with `identity`'s sealing key.
fn sealing_node(
    path: &std::path::Path,
    device: u128,
    identity: &Identity,
) -> nexusfs_core::CoreState {
    bootstrapped(path, device)
        .with_encryption(Arc::new(RepoCipher::new([9u8; 32])))
        .with_sealing_key(identity.sealing_secret())
}

#[test]
fn a_write_seals_to_this_device_so_it_can_read_its_own_content() {
    // The failure this must never introduce: writing content the writer cannot read.
    let dir = tempfile::tempdir().unwrap();
    let id = Identity::generate();
    let core = sealing_node(dir.path(), 0x01, &id);

    core.write_file(&id, "/s.txt", b"only me", now_ms())
        .unwrap();
    assert_eq!(core.read_file_path("/s.txt").unwrap(), b"only me");

    let (inode, _, _) = core.stat_file("/s.txt").unwrap().unwrap();
    let node_hash = core
        .load_inode(inode)
        .unwrap()
        .unwrap()
        .content
        .value
        .node_hash
        .unwrap();
    let Some(Object::FileNode(file)) = core.get_object(&node_hash).unwrap() else {
        panic!("expected a file node");
    };
    let enc = file.encryption.expect("the file should be encrypted");
    assert_eq!(enc.recipients.len(), 1, "sealed to this device alone");
    assert!(
        enc.sealed_key.is_none(),
        "the repository key is not used once there are recipients"
    );
}

#[test]
fn an_enrolled_peer_becomes_a_recipient_and_a_stranger_does_not() {
    let dir = tempfile::tempdir().unwrap();
    let writer_id = Identity::generate();
    let friend = Identity::generate();
    let stranger = Identity::generate();

    let core = sealing_node(dir.path(), 0x01, &writer_id);
    core.enrol_peer(
        nexusfs_proto::DeviceId(0xB2),
        &friend.pubkey_bytes(),
        Some(&friend.sealing_pubkey()),
        false,
    )
    .unwrap();

    core.write_file(&writer_id, "/shared.txt", b"for us two", now_ms())
        .unwrap();

    let (inode, _, _) = core.stat_file("/shared.txt").unwrap().unwrap();
    let node_hash = core
        .load_inode(inode)
        .unwrap()
        .unwrap()
        .content
        .value
        .node_hash
        .unwrap();
    let Some(Object::FileNode(file)) = core.get_object(&node_hash).unwrap() else {
        panic!("expected a file node");
    };
    let enc = file.encryption.clone().unwrap();
    assert_eq!(enc.recipients.len(), 2, "this device and the enrolled peer");

    // The friend can open one of the envelopes; the stranger can open none. Checked at
    // the crypto layer because that is the claim — no state, no store, just the key.
    let opened: Vec<_> = enc
        .recipients
        .iter()
        .filter_map(|e| nexusfs_crypto::envelope::open(friend.sealing_secret(), e).ok())
        .collect();
    assert_eq!(opened.len(), 1, "exactly one envelope is the friend's");

    let stranger_opened = enc
        .recipients
        .iter()
        .any(|e| nexusfs_crypto::envelope::open(stranger.sealing_secret(), e).is_ok());
    assert!(!stranger_opened, "a stranger opens nothing");
}

#[test]
fn a_replica_that_is_not_a_recipient_holds_the_bytes_and_cannot_read_them() {
    // The end-to-end version of the claim, through the real read path rather than the
    // crypto layer: every stored record present, and the file still unreadable.
    let writer_dir = tempfile::tempdir().unwrap();
    let reader_dir = tempfile::tempdir().unwrap();
    let writer_id = Identity::generate();
    let outsider = Identity::generate();

    let writer = sealing_node(writer_dir.path(), 0x01, &writer_id);
    writer
        .write_file(&writer_id, "/s.txt", b"not for you", now_ms())
        .unwrap();

    // A second node with the *same repository key* — which under the old scheme would
    // have been enough — but not a recipient.
    let reader = bootstrapped(reader_dir.path(), 0x02)
        .with_encryption(Arc::new(RepoCipher::new([9u8; 32])))
        .with_sealing_key(outsider.sealing_secret());

    // Content first, then operations. The other order parks every write for missing
    // chunks, and a parked write has no content — so the read would come back *empty*
    // rather than refused, which would test the wrong thing entirely.
    for (hash, _) in writer.stores.blobs.list().unwrap() {
        let bytes = writer.stores.blobs.get(&hash).unwrap().unwrap();
        reader.stores.blobs.put(hash, &bytes).unwrap();
    }
    for op in writer.all_ops().unwrap() {
        reader.apply_op(&op).unwrap();
    }
    assert_eq!(
        reader.pending_count().unwrap(),
        0,
        "nothing should be parked"
    );

    assert!(
        reader.stat_file("/s.txt").unwrap().is_some(),
        "the namespace replicated, so the file is known to exist"
    );
    let err = reader.read_file_path("/s.txt").unwrap_err();
    let msg = format!("{err:#}");
    assert!(
        msg.contains("not a recipient"),
        "the error should say why, got: {msg}"
    );
}

#[test]
fn a_node_with_no_sealing_key_still_uses_the_repository_key() {
    // The fallback, and the only path that still produces a repository-key-sealed file:
    // a `CoreState` built without a sealing key at all.
    let dir = tempfile::tempdir().unwrap();
    let core = encrypted_node(dir.path(), 0x03, [9u8; 32]);
    let id = Identity::generate();

    core.write_file(&id, "/s.txt", b"old scheme", now_ms())
        .unwrap();
    assert_eq!(core.read_file_path("/s.txt").unwrap(), b"old scheme");

    let (inode, _, _) = core.stat_file("/s.txt").unwrap().unwrap();
    let node_hash = core
        .load_inode(inode)
        .unwrap()
        .unwrap()
        .content
        .value
        .node_hash
        .unwrap();
    let Some(Object::FileNode(file)) = core.get_object(&node_hash).unwrap() else {
        panic!("expected a file node");
    };
    let enc = file.encryption.unwrap();
    assert!(enc.recipients.is_empty());
    assert!(enc.sealed_key.is_some());
}

#[test]
fn a_repository_key_file_stays_readable_after_sealing_is_switched_on() {
    // Upgrading a node must not strand what it already wrote.
    let dir = tempfile::tempdir().unwrap();
    let id = Identity::generate();

    {
        let before = encrypted_node(dir.path(), 0x04, [9u8; 32]);
        before
            .write_file(&id, "/old.txt", b"written before", now_ms())
            .unwrap();
    }

    let after = bootstrapped(dir.path(), 0x04)
        .with_encryption(Arc::new(RepoCipher::new([9u8; 32])))
        .with_sealing_key(id.sealing_secret());
    assert_eq!(after.read_file_path("/old.txt").unwrap(), b"written before");

    // And new writes use the new scheme, side by side with the old file.
    after
        .write_file(&id, "/new.txt", b"written after", now_ms())
        .unwrap();
    assert_eq!(after.read_file_path("/new.txt").unwrap(), b"written after");
    assert_eq!(after.read_file_path("/old.txt").unwrap(), b"written before");
}

#[test]
fn resealing_lets_a_newly_enrolled_peer_read_what_came_before() {
    // The gap `share` exists to close: enrolment makes a peer a recipient of what comes
    // *after* it, and files already on disk carry the old envelope set.
    let dir = tempfile::tempdir().unwrap();
    let writer_id = Identity::generate();
    let latecomer = Identity::generate();
    let core = sealing_node(dir.path(), 0x01, &writer_id);

    core.write_file(&writer_id, "/early.txt", b"written first", now_ms())
        .unwrap();

    let node_hash_of = |path: &str| {
        let (inode, _, _) = core.stat_file(path).unwrap().unwrap();
        core.load_inode(inode)
            .unwrap()
            .unwrap()
            .content
            .value
            .node_hash
            .unwrap()
    };
    let opens_for = |hash, id: &Identity| {
        let Some(Object::FileNode(file)) = core.get_object(&hash).unwrap() else {
            panic!("expected a file node");
        };
        file.encryption
            .unwrap()
            .recipients
            .iter()
            .any(|e| nexusfs_crypto::envelope::open(id.sealing_secret(), e).is_ok())
    };

    assert!(!opens_for(node_hash_of("/early.txt"), &latecomer));

    core.enrol_peer(
        nexusfs_proto::DeviceId(0xB2),
        &latecomer.pubkey_bytes(),
        Some(&latecomer.sealing_pubkey()),
        false,
    )
    .unwrap();

    // A survey changes nothing.
    let survey = core
        .reseal_to_recipients(&writer_id, now_ms(), true)
        .unwrap();
    assert_eq!(survey.resealed, 1);
    assert!(!opens_for(node_hash_of("/early.txt"), &latecomer));

    let done = core
        .reseal_to_recipients(&writer_id, now_ms(), false)
        .unwrap();
    assert_eq!(done.resealed, 1);
    assert!(opens_for(node_hash_of("/early.txt"), &latecomer));

    // The content is unchanged and still readable by the writer.
    assert_eq!(core.read_file_path("/early.txt").unwrap(), b"written first");
}

#[test]
fn resealing_twice_finds_nothing_to_do() {
    let dir = tempfile::tempdir().unwrap();
    let id = Identity::generate();
    let core = sealing_node(dir.path(), 0x01, &id);
    core.write_file(&id, "/a.txt", b"x", now_ms()).unwrap();

    core.reseal_to_recipients(&id, now_ms(), false).unwrap();
    let again = core.reseal_to_recipients(&id, now_ms(), true).unwrap();
    assert_eq!(
        again.resealed, 0,
        "already sealed to exactly these recipients"
    );
    assert_eq!(again.already_current, 1);
}

#[test]
fn resealing_skips_files_this_node_cannot_read() {
    // A node that is not a recipient has no file key to re-seal with. That is a count,
    // not a failure: one such file must not stop the rest of the run.
    let writer_dir = tempfile::tempdir().unwrap();
    let reader_dir = tempfile::tempdir().unwrap();
    let writer_id = Identity::generate();
    let outsider = Identity::generate();

    let writer = sealing_node(writer_dir.path(), 0x01, &writer_id);
    writer
        .write_file(&writer_id, "/s.txt", b"not for you", now_ms())
        .unwrap();

    let reader = bootstrapped(reader_dir.path(), 0x02).with_sealing_key(outsider.sealing_secret());
    for (hash, _) in writer.stores.blobs.list().unwrap() {
        let bytes = writer.stores.blobs.get(&hash).unwrap().unwrap();
        reader.stores.blobs.put(hash, &bytes).unwrap();
    }
    for op in writer.all_ops().unwrap() {
        reader.apply_op(&op).unwrap();
    }

    let report = reader
        .reseal_to_recipients(&outsider, now_ms(), true)
        .unwrap();
    assert_eq!(report.unreadable, 1);
    assert_eq!(report.resealed, 0);
}
