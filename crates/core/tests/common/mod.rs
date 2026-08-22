#![allow(dead_code)]

use std::path::Path;

use nexusfs_core::{CoreState, Stores};
use nexusfs_crypto::{sign, Identity};
use nexusfs_proto::{CausalCtx, ChunkRef, DeviceId, FsOp, FsOpKind, OpId};
use nexusfs_storage::sled_store::SledStore;

pub fn open_core(path: &Path, device: u128) -> CoreState {
    let store = SledStore::open(path).unwrap();
    let stores = Stores::shared(store);
    CoreState::new(stores, DeviceId(device))
}

pub fn bootstrapped(path: &Path, device: u128) -> CoreState {
    let core = open_core(path, device);
    core.bootstrap_if_needed().unwrap();
    core
}

/// Build a signed op without touching any store.
///
/// Deliberately independent of `CoreState::make_op`, which allocates a counter from
/// local state: convergence tests need the *same* ops applied to several replicas, so
/// the identity of an op cannot depend on who applies it.
pub fn signed_op(
    identity: &Identity,
    device: u128,
    counter: u64,
    time_unix_ms: u64,
    kind: FsOpKind,
) -> FsOp {
    let mut op = FsOp {
        id: OpId {
            device_id: DeviceId(device),
            counter,
        },
        time_unix_ms,
        ctx: CausalCtx { deps: vec![] },
        kind,
        author_pubkey: identity.pubkey_bytes(),
        sig: Vec::new(),
        proof: None,
    };
    op.sig = sign(identity.signing_key(), &op.signing_bytes().unwrap());
    op
}

pub fn mkdir(parent: u128, name: &str) -> FsOpKind {
    FsOpKind::Mkdir {
        parent,
        name: name.to_string(),
        mode: 0o40755,
    }
}

pub fn create_file(parent: u128, name: &str) -> FsOpKind {
    FsOpKind::CreateFile {
        parent,
        name: name.to_string(),
        mode: 0o100644,
    }
}

/// An unlink that observed the entries created by `observed`.
///
/// Stating this explicitly is the point: a removal that derives its observed set from
/// whatever happens to be present when it is applied gives different answers on
/// replicas that received the same operations in different orders.
pub fn unlink(parent: u128, name: &str, observed: &[OpId]) -> FsOpKind {
    FsOpKind::Unlink {
        parent,
        name: name.to_string(),
        observed: observed.to_vec(),
    }
}

/// The id of an operation, for naming what a removal observed.
pub fn op_id(device: u128, counter: u64) -> OpId {
    OpId {
        device_id: DeviceId(device),
        counter,
    }
}

pub fn rename(
    old_parent: u128,
    old_name: &str,
    new_parent: u128,
    new_name: &str,
    observed: &[OpId],
) -> FsOpKind {
    FsOpKind::Rename {
        old_parent,
        old_name: old_name.to_string(),
        new_parent,
        new_name: new_name.to_string(),
        observed: observed.to_vec(),
    }
}

/// Chunk `data` into `core`'s blob store and return a whole-file Write op kind.
///
/// Uses `store_content`, so the op carries key material whenever the node encrypts.
pub fn write_all(core: &CoreState, inode: u128, data: &[u8]) -> FsOpKind {
    let (chunks, encryption) = core.store_content(data).unwrap();
    let _: &Vec<ChunkRef> = &chunks;
    FsOpKind::Write {
        inode,
        offset: 0,
        chunks,
        new_size: data.len() as u64,
        encryption,
    }
}

pub fn names(core: &CoreState, path: &str) -> Vec<String> {
    core.read_dir_path(path)
        .unwrap()
        .into_iter()
        .map(|e| e.name)
        .collect()
}
