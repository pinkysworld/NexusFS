#![forbid(unsafe_code)]

#[cfg(feature = "posix")]
pub mod fuse;

use anyhow::Result;

/// Mount the filesystem (stub).
///
/// Real implementation lives in `fuse.rs` behind the `posix` feature.
pub async fn mount(_mountpoint: &str) -> Result<()> {
    anyhow::bail!("POSIX mount not implemented in skeleton. Enable feature `posix` and implement `fuse` module.")
}
