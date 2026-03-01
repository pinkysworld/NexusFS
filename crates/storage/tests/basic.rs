#[cfg(feature = "sled")]
#[test]
fn sled_blob_and_kv_roundtrip() {
    use nexusfs_storage::sled_store::SledStore;
    use nexusfs_storage::{BlobStore, KvStore};

    let dir = tempfile::tempdir().unwrap();
    let store = SledStore::open(dir.path()).unwrap();

    let key: [u8; 32] = [7u8; 32];
    store.put(key, b"hello").unwrap();
    assert!(store.has(&key).unwrap());
    assert_eq!(store.get(&key).unwrap().unwrap(), b"hello");

    store.delete(&key).unwrap();
    assert!(!store.has(&key).unwrap());

    store.put_kv("cf1", b"k1", b"v1").unwrap();
    assert_eq!(store.get_kv("cf1", b"k1").unwrap().unwrap(), b"v1");

    store.put_kv("cf1", b"pref/a", b"1").unwrap();
    store.put_kv("cf1", b"pref/b", b"2").unwrap();
    let scanned = store.scan_prefix("cf1", b"pref/").unwrap();
    assert_eq!(scanned.len(), 2);
}
