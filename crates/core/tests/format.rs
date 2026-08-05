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
    // them is filling in a known fact, not a migration.
    let dir = tempfile::tempdir().unwrap();
    let core = bootstrapped(dir.path(), 0xA1);
    let id = Identity::generate();
    core.write_file(&id, "/a.txt", b"written before versioning", 1_000)
        .unwrap();

    core.stores.kv.delete_kv(CF_META, KEY).unwrap();
    assert_eq!(core.format_version().unwrap(), None);

    assert_eq!(core.check_format().unwrap(), FormatState::Current);
    assert_eq!(core.format_version().unwrap(), Some(1));
    assert_eq!(
        core.read_file_path("/a.txt").unwrap(),
        b"written before versioning"
    );
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
