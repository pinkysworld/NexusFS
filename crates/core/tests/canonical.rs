#[test]
fn dirnode_entries_are_sorted_canonically() {
    use nexusfs_core::object::{DirEntry, DirNode, EntryType};

    let now = 0;
    let entries = vec![
        DirEntry {
            name: "b".into(),
            inode_id: 2,
            entry_type: EntryType::File,
        },
        DirEntry {
            name: "a".into(),
            inode_id: 1,
            entry_type: EntryType::File,
        },
    ];

    let dir = DirNode::new_canonical(entries, 0o40755, 0, 0, now);
    assert_eq!(dir.entries[0].name, "a");
    assert_eq!(dir.entries[1].name, "b");
}

#[test]
fn hash_bytes_is_deterministic() {
    use nexusfs_core::hash::hash_bytes;

    let h1 = hash_bytes(b"hello");
    let h2 = hash_bytes(b"hello");
    assert_eq!(h1, h2);
}

#[test]
fn encode_object_normalizes_unsorted_directory_entries() {
    use nexusfs_core::object::{DirEntry, DirNode, EntryType, Object, ObjectHeader};
    use nexusfs_core::{encode_object, hash_object};

    let now = 123;
    let unsorted = Object::DirNode(DirNode {
        header: ObjectHeader {
            type_tag: 2,
            version: 1,
        },
        entries: vec![
            DirEntry {
                name: "b".into(),
                inode_id: 2,
                entry_type: EntryType::File,
            },
            DirEntry {
                name: "a".into(),
                inode_id: 1,
                entry_type: EntryType::Dir,
            },
        ],
        mode: 0o40755,
        uid: 0,
        gid: 0,
        mtime_unix_ms: now,
        ctime_unix_ms: now,
    });
    let canonical = Object::DirNode(DirNode::new_canonical(
        vec![
            DirEntry {
                name: "a".into(),
                inode_id: 1,
                entry_type: EntryType::Dir,
            },
            DirEntry {
                name: "b".into(),
                inode_id: 2,
                entry_type: EntryType::File,
            },
        ],
        0o40755,
        0,
        0,
        now,
    ));

    let encoded_unsorted = encode_object(&unsorted).unwrap();
    let encoded_canonical = encode_object(&canonical).unwrap();
    assert_eq!(encoded_unsorted, encoded_canonical);
    assert_eq!(
        hash_object(&unsorted).unwrap(),
        hash_object(&canonical).unwrap()
    );
}
