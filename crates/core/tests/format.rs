//! On-disk format versioning.
//!
//! The point of the stamp is that a mismatched build refuses rather than reading
//! records it will misinterpret, so these check the refusals as much as the successes.

mod common;

use common::*;
use nexusfs_core::{FormatState, CF_META, CURRENT_FORMAT_VERSION};
use nexusfs_crypto::Identity;

const KEY: &[u8] = b"format/version";

#[test]
fn a_new_repository_is_stamped_with_the_current_format() {
    let dir = tempfile::tempdir().unwrap();
    let core = bootstrapped(dir.path(), 0xA1);

    assert_eq!(core.check_format().unwrap(), FormatState::Current);
    assert_eq!(core.format_version().unwrap(), Some(CURRENT_FORMAT_VERSION));
    core.require_current_format().unwrap();
}

#[test]
fn an_unstamped_repository_is_adopted_as_version_one() {
    // Repositories written before versioning existed are, by definition, v1. Adopting
    // them is filling in a known fact, not a migration — but it does mean they then
    // need one, because this build is past v1.
    let dir = tempfile::tempdir().unwrap();
    let core = bootstrapped(dir.path(), 0xA1);
    let id = Identity::generate();
    core.write_file(&id, "/a.txt", b"written before versioning", 1_000)
        .unwrap();

    core.stores.kv.delete_kv(CF_META, KEY).unwrap();
    assert_eq!(core.format_version().unwrap(), None);

    assert_eq!(
        core.check_format().unwrap(),
        FormatState::NeedsMigration { found: 1 }
    );
    assert_eq!(core.format_version().unwrap(), Some(1));

    // And migrating carries it forward without touching the data, one step at a time
    // rather than in a leap — a repository several versions behind runs each step in
    // turn, and each only has to understand its immediate predecessor.
    assert_eq!(core.migrate().unwrap(), vec![2, 3]);
    assert_eq!(core.check_format().unwrap(), FormatState::Current);
    assert_eq!(
        core.read_file_path("/a.txt").unwrap(),
        b"written before versioning"
    );
}

#[test]
fn migrating_to_v2_rebuilds_the_state_commitment() {
    // v1 -> v2 changed how the inode map is committed. Nothing stored changes shape, so
    // the migration is a re-snapshot — and the recorded root must actually move.
    let dir = tempfile::tempdir().unwrap();
    let core = bootstrapped(dir.path(), 0xA1);
    let id = Identity::generate();
    core.write_file(&id, "/a.txt", b"content", 1_000).unwrap();

    let expected = core.compute_state_root().unwrap();

    // Pose as a v1 repository whose recorded root predates the new commitment.
    core.stores
        .kv
        .put_kv(CF_META, KEY, &1u32.to_be_bytes())
        .unwrap();
    core.stores
        .kv
        .put_kv(CF_META, b"state/root", &[0u8; 32])
        .unwrap();

    let steps = core.migrate().unwrap();

    assert_eq!(steps, vec![2, 3], "each step runs in turn");
    assert_eq!(core.format_version().unwrap(), Some(CURRENT_FORMAT_VERSION));
    assert_eq!(
        core.get_state_root().unwrap(),
        Some(expected),
        "the v2 step must recompute the commitment, not leave the stale one"
    );
    assert_eq!(core.read_file_path("/a.txt").unwrap(), b"content");
}

#[test]
fn a_newer_format_is_refused_and_cannot_be_forced() {
    let dir = tempfile::tempdir().unwrap();
    let core = bootstrapped(dir.path(), 0xA1);

    let future = CURRENT_FORMAT_VERSION + 7;
    core.stores
        .kv
        .put_kv(CF_META, KEY, &future.to_be_bytes())
        .unwrap();

    assert_eq!(
        core.check_format().unwrap(),
        FormatState::TooNew { found: future }
    );

    let err = core.require_current_format().unwrap_err().to_string();
    assert!(err.contains("newer"), "message should say why: {err}");
    assert!(err.contains("Upgrade NexusFS"), "and what to do: {err}");

    // Migration is not an escape hatch: this build cannot know what the newer format
    // means, so there is nothing safe for it to do.
    assert!(core.migrate().is_err());
}

#[test]
fn an_older_format_is_refused_until_migrated() {
    let dir = tempfile::tempdir().unwrap();
    let core = bootstrapped(dir.path(), 0xA1);

    core.stores
        .kv
        .put_kv(CF_META, KEY, &0u32.to_be_bytes())
        .unwrap();

    assert_eq!(
        core.check_format().unwrap(),
        FormatState::NeedsMigration { found: 0 }
    );
    let err = core.require_current_format().unwrap_err().to_string();
    assert!(
        err.contains("nexusfs migrate"),
        "the message must name the fix: {err}"
    );
}

#[test]
fn migrating_a_current_repository_does_nothing() {
    let dir = tempfile::tempdir().unwrap();
    let core = bootstrapped(dir.path(), 0xA1);
    let id = Identity::generate();
    core.write_file(&id, "/a.txt", b"unchanged", 1_000).unwrap();

    let before = core.compute_state_root().unwrap();
    assert!(core.migrate().unwrap().is_empty());
    assert_eq!(core.compute_state_root().unwrap(), before);
    assert_eq!(core.format_version().unwrap(), Some(CURRENT_FORMAT_VERSION));
}

#[test]
fn a_corrupt_version_record_is_reported_rather_than_guessed() {
    let dir = tempfile::tempdir().unwrap();
    let core = bootstrapped(dir.path(), 0xA1);

    core.stores
        .kv
        .put_kv(CF_META, KEY, b"not-a-version")
        .unwrap();

    let err = core.format_version().unwrap_err().to_string();
    assert!(err.contains("expected 4"), "{err}");
}

#[test]
fn the_stamp_survives_a_restart() {
    let dir = tempfile::tempdir().unwrap();
    {
        let core = bootstrapped(dir.path(), 0xA1);
        core.require_current_format().unwrap();
    }
    let core = bootstrapped(dir.path(), 0xA1);
    assert_eq!(core.format_version().unwrap(), Some(CURRENT_FORMAT_VERSION));
}

#[test]
fn migrating_to_v3_carries_an_unencrypted_repository_forward() {
    // The common case, and the reason this migration is possible at all: a plaintext
    // file records `encryption: None`, which postcard writes as a single zero byte
    // whatever type it wraps. Every unencrypted record is therefore byte-identical
    // across v2 and v3, and the migration is just the stamp.
    let dir = tempfile::tempdir().unwrap();
    let core = bootstrapped(dir.path(), 0xA1);
    let id = Identity::generate();
    core.mkdir_p(&id, "/docs", 1_000).unwrap();
    core.write_file(&id, "/docs/a.txt", b"plaintext", 1_001)
        .unwrap();

    core.stores
        .kv
        .put_kv(CF_META, KEY, &2u32.to_be_bytes())
        .unwrap();
    assert_eq!(
        core.check_format().unwrap(),
        FormatState::NeedsMigration { found: 2 }
    );

    assert_eq!(core.migrate().unwrap(), vec![3]);
    assert_eq!(core.check_format().unwrap(), FormatState::Current);
    assert_eq!(core.read_file_path("/docs/a.txt").unwrap(), b"plaintext");
    assert!(core.verify_repository().unwrap().ok());
}

#[test]
fn migrating_to_v3_refuses_a_repository_holding_old_encrypted_records() {
    // The case that cannot be upgraded in place, and must say so rather than stamp and
    // leave objects nobody can decode. Posing as one: an operation record that does not
    // decode under the current schema is exactly what a v2 encrypted `Write` becomes.
    let dir = tempfile::tempdir().unwrap();
    let core = bootstrapped(dir.path(), 0xA1);
    let id = Identity::generate();
    core.write_file(&id, "/a.txt", b"content", 1_000).unwrap();

    let key = core
        .stores
        .kv
        .scan_prefix_keys("oplog", b"op\0")
        .unwrap()
        .into_iter()
        .next()
        .expect("the write left an operation");
    core.stores
        .kv
        .put_kv("oplog", &key, b"not a valid postcard FsOp at all")
        .unwrap();

    core.stores
        .kv
        .put_kv(CF_META, KEY, &2u32.to_be_bytes())
        .unwrap();

    let err = core.migrate().unwrap_err();
    let msg = format!("{err:#}");
    assert!(
        msg.contains("v2 encryption format"),
        "the refusal should name the reason, got: {msg}"
    );
    assert!(
        msg.contains("fresh v3 repository"),
        "and what to do about it, got: {msg}"
    );
    assert_eq!(
        core.format_version().unwrap(),
        Some(2),
        "a refused migration must not move the stamp"
    );
}
