#[test]
fn conflict_name_is_deterministic() {
    let a = nexusfs_crdt::conflicts::conflict_name("file", 42, 1000);
    let b = nexusfs_crdt::conflicts::conflict_name("file", 42, 1000);
    assert_eq!(a, b);
    assert!(a.contains("conflict"));
}
