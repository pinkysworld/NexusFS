use nexusfs_proto::types::DeviceId;

use crate::object::{ObjectHeader, SnapshotRoot};

pub fn new_snapshot_root(root_dir_inode: u128, author: DeviceId, now_ms: u64) -> SnapshotRoot {
    SnapshotRoot {
        header: ObjectHeader {
            type_tag: 3,
            version: 1,
        },
        root_dir_inode,
        inode_map_root: None,
        timestamp_unix_ms: now_ms,
        author,
    }
}
