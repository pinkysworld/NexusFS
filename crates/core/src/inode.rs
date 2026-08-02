use nexusfs_crdt::or_map::Dot;
use nexusfs_proto::OpId;

/// Reserved inode for the filesystem root.
pub const ROOT_INODE: u128 = 1;

/// Domain separator so inode ids can never collide with another BLAKE3 use.
const INODE_DOMAIN: &[u8] = b"nexusfs/inode/v1";

/// Derive the inode id allocated by an operation.
///
/// This must be a pure function of the `OpId`: every replica that applies the same
/// `Mkdir`/`CreateFile` has to name the same inode, and no replica can consult a
/// shared counter to do it. Two devices that concurrently create `/docs` therefore
/// allocate *different* inodes, which is correct — that is precisely the conflict
/// that deterministic conflict naming exists to resolve.
pub fn inode_for_op(op_id: OpId) -> u128 {
    let mut hasher = blake3::Hasher::new();
    hasher.update(INODE_DOMAIN);
    hasher.update(&op_id.device_id.0.to_be_bytes());
    hasher.update(&op_id.counter.to_be_bytes());

    let mut bytes = [0u8; 16];
    bytes.copy_from_slice(&hasher.finalize().as_bytes()[..16]);
    let id = u128::from_be_bytes(bytes);

    // Keep the low ids reserved (0 = unset, 1 = root).
    if id <= ROOT_INODE {
        id.wrapping_add(2)
    } else {
        id
    }
}

/// Ops and CRDT dots are the same `(device, counter)` pair, so an operation
/// identifies its own causal dot directly.
pub fn dot_for_op(op_id: OpId) -> Dot {
    Dot {
        device_id: op_id.device_id.0,
        counter: op_id.counter,
    }
}
