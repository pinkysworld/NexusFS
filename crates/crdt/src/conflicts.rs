/// Deterministic conflict naming.
///
/// Keep this stable so all replicas converge on the same filenames.
pub fn conflict_name(base: &str, device_id: u128, time_ms: u64) -> String {
    // Keep it filesystem-friendly and deterministic.
    // Example: "file~conflict-<deviceid16>-<time_ms>"
    format!(
        "{}~conflict-{:016x}-{}",
        base,
        (device_id & 0xffff_ffff_ffff_ffff),
        time_ms
    )
}
