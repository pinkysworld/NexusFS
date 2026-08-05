//! Peer enrolment.
//!
//! The value of pinning is that an unexpected key is *noticed*. So the tests that
//! matter here are the ones asserting a silent overwrite cannot happen.

mod common;

use common::*;
use nexusfs_core::Enrolment;
use nexusfs_proto::DeviceId;

#[test]
fn a_peer_can_be_enrolled_before_it_ever_connects() {
    // The whole point of not relying on trust-on-first-use: the key is known in
    // advance, so first contact has nothing to decide.
    let dir = tempfile::tempdir().unwrap();
    let core = bootstrapped(dir.path(), 0xA1);

    assert_eq!(
        core.enrol_peer(DeviceId(0xB2), &[7u8; 32], false).unwrap(),
        Enrolment::Added
    );
    assert_eq!(core.peer_key(DeviceId(0xB2)).unwrap(), Some([7u8; 32]));
    assert_eq!(core.enrolled_peers().unwrap().len(), 1);
}

#[test]
fn re_enrolling_the_same_key_is_not_a_change() {
    let dir = tempfile::tempdir().unwrap();
    let core = bootstrapped(dir.path(), 0xA1);

    core.enrol_peer(DeviceId(0xB2), &[7u8; 32], false).unwrap();
    assert_eq!(
        core.enrol_peer(DeviceId(0xB2), &[7u8; 32], false).unwrap(),
        Enrolment::Unchanged
    );
}

#[test]
fn a_different_key_is_refused_unless_rotation_is_explicit() {
    let dir = tempfile::tempdir().unwrap();
    let core = bootstrapped(dir.path(), 0xA1);
    core.enrol_peer(DeviceId(0xB2), &[7u8; 32], false).unwrap();

    let err = core
        .enrol_peer(DeviceId(0xB2), &[9u8; 32], false)
        .unwrap_err()
        .to_string();
    assert!(
        err.contains("--rotate"),
        "the message must name the fix: {err}"
    );
    assert_eq!(
        core.peer_key(DeviceId(0xB2)).unwrap(),
        Some([7u8; 32]),
        "the refusal must leave the pinned key alone"
    );

    assert_eq!(
        core.enrol_peer(DeviceId(0xB2), &[9u8; 32], true).unwrap(),
        Enrolment::Rotated
    );
    assert_eq!(core.peer_key(DeviceId(0xB2)).unwrap(), Some([9u8; 32]));
}

#[test]
fn revocation_reports_whether_anything_was_there() {
    let dir = tempfile::tempdir().unwrap();
    let core = bootstrapped(dir.path(), 0xA1);

    assert!(!core.revoke_peer(DeviceId(0xB2)).unwrap());
    core.enrol_peer(DeviceId(0xB2), &[7u8; 32], false).unwrap();
    assert!(core.revoke_peer(DeviceId(0xB2)).unwrap());
    assert_eq!(core.peer_key(DeviceId(0xB2)).unwrap(), None);
}

#[test]
fn enrolments_are_listed_in_a_stable_order() {
    // The listing is what an operator compares against another node, so it must not
    // depend on insertion order.
    let dir = tempfile::tempdir().unwrap();
    let core = bootstrapped(dir.path(), 0xA1);

    for id in [0xF0u128, 0x02, 0xAA, 0x01] {
        core.enrol_peer(DeviceId(id), &[id as u8; 32], false)
            .unwrap();
    }
    let ids: Vec<u128> = core
        .enrolled_peers()
        .unwrap()
        .iter()
        .map(|p| p.device_id.0)
        .collect();
    assert_eq!(ids, vec![0x01, 0x02, 0xAA, 0xF0]);
}

#[test]
fn enrolments_survive_a_restart() {
    let dir = tempfile::tempdir().unwrap();
    {
        let core = bootstrapped(dir.path(), 0xA1);
        core.enrol_peer(DeviceId(0xB2), &[7u8; 32], false).unwrap();
    }
    let core = bootstrapped(dir.path(), 0xA1);
    assert_eq!(core.peer_key(DeviceId(0xB2)).unwrap(), Some([7u8; 32]));
}
