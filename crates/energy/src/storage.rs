//! How much room is left where the store lives.
//!
//! `Telemetry` has carried a `storage_free_bytes` field since the scheduler was
//! written, and nothing ever filled it. This is the probe.
//!
//! It matters differently from the other readings. Battery, heat and link cost are
//! *tradeoffs* — transferring content costs power, or money, or thermal headroom, and
//! the scheduler weighs that against staying current. Disk is not a tradeoff. Bytes
//! cannot be stored where there is no room, so this is the only reading that describes
//! something replication simply cannot do.
//!
//! It is also the only one that is about a *path* rather than the machine. A node whose
//! store is on an external volume cares about that volume, not about `/`, and the two
//! can differ by orders of magnitude — so the probe takes the data directory and the
//! caller supplies it.
//!
//! As everywhere in this crate, an unreadable source yields `None` and never a zero.
//! "We could not ask" and "there is no space" would otherwise be the same value, and
//! only one of them should stop content moving.

use std::path::Path;

/// Free bytes on the filesystem holding `dir`, or `None` when it cannot be read.
pub fn free_bytes(dir: &Path) -> Option<u64> {
    // `df` rather than `statvfs`: this crate is `forbid(unsafe_code)` and has no libc
    // dependency, and a subprocess once per sync pass is not a cost worth adding one
    // for. It also keeps the shape the rest of this crate uses — a spawn and a pure
    // parser — so the parsing is tested on any host against captured output.
    //
    // `-P` is POSIX output: one line per filesystem and a fixed column order, which
    // plain `df` does not promise. `-k` fixes the unit at 1024-byte blocks, so the
    // arithmetic below does not depend on the host's `BLOCKSIZE`.
    let out = std::process::Command::new("df")
        .arg("-Pk")
        .arg(dir)
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    parse_df_available_kb(&String::from_utf8_lossy(&out.stdout))?.checked_mul(1024)
}

/// Read the "Available" column out of `df -Pk` output.
///
/// Located by the capacity column rather than by position. Under `-P` the order is
/// filesystem, blocks, used, available, capacity, mount point — but the *first* and
/// *last* of those can both contain spaces (`//host/My Share`, `/Volumes/Backup Disk`),
/// so counting fields from either end is wrong on exactly the setups least likely to be
/// tested. Capacity is the only purely numeric field ending in `%`, and available is
/// always the field before it.
pub(crate) fn parse_df_available_kb(out: &str) -> Option<u64> {
    // One line per filesystem, and querying a single path prints exactly one — but take
    // the last rather than the first so a header or a warning line cannot be mistaken
    // for data.
    let line = out.lines().rfind(|l| !l.trim().is_empty())?;
    let cols: Vec<&str> = line.split_whitespace().collect();

    let capacity = cols.iter().position(|c| {
        c.len() > 1 && c.ends_with('%') && c[..c.len() - 1].chars().all(|ch| ch.is_ascii_digit())
    })?;

    cols.get(capacity.checked_sub(1)?)?.parse::<u64>().ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_available_column_is_read_from_ordinary_output() {
        // Linux coreutils.
        let out = "\
Filesystem     1024-blocks      Used Available Capacity Mounted on
/dev/nvme0n1p2   959786032 412887044 498062092      46% /
";
        assert_eq!(parse_df_available_kb(out), Some(498_062_092));
    }

    #[test]
    fn a_device_name_with_spaces_does_not_shift_the_columns() {
        // The case that breaks counting fields from the left.
        let out = "\
Filesystem     1024-blocks      Used Available Capacity Mounted on
//host/My Share   10485760   1048576   9437184      10% /Volumes/share
";
        assert_eq!(parse_df_available_kb(out), Some(9_437_184));
    }

    #[test]
    fn a_mount_point_with_spaces_does_not_shift_them_either() {
        // And the case that breaks counting from the right.
        let out = "\
Filesystem   1024-blocks     Used Available Capacity  Mounted on
/dev/disk4s1     1953514  1234567    718947      64%  /Volumes/Backup Disk 2
";
        assert_eq!(parse_df_available_kb(out), Some(718_947));
    }

    #[test]
    fn macos_output_with_its_extra_columns_still_parses() {
        // macOS prints iused/ifree/%iused after capacity. Anchoring on the first
        // numeric percentage keeps those from being mistaken for the answer.
        let out = "\
Filesystem   1024-blocks      Used Available Capacity iused      ifree %iused  Mounted on
/dev/disk3s5   971350180 615577552 344582628      65%  501150 3445826280    0%   /System/Volumes/Data
";
        assert_eq!(parse_df_available_kb(out), Some(344_582_628));
    }

    #[test]
    fn output_with_no_data_line_reports_nothing() {
        assert_eq!(parse_df_available_kb(""), None);
        assert_eq!(
            parse_df_available_kb("df: /nope: No such file or directory\n"),
            None
        );
        // A header alone: no percentage column, so nothing to read.
        assert_eq!(
            parse_df_available_kb("Filesystem 1024-blocks Used Available Capacity Mounted on\n"),
            None
        );
    }

    #[test]
    fn trailing_blank_lines_do_not_hide_the_answer() {
        let out = "\
Filesystem     1024-blocks      Used Available Capacity Mounted on
/dev/sda1          1024000    512000    512000      50% /

";
        assert_eq!(parse_df_available_kb(out), Some(512_000));
    }

    #[test]
    fn the_probe_on_this_host_answers_without_panicking() {
        // Whatever it returns is host-dependent; that it does not panic and does not
        // report a suspiciously exact zero is not.
        let answer = free_bytes(Path::new("."));
        if let Some(free) = answer {
            assert!(free > 0, "a writable checkout should have some room");
        }
    }
}
