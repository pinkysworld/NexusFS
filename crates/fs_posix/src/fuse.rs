#![forbid(unsafe_code)]

#[cfg(feature = "posix")]
pub fn mount_fuse(_mountpoint: &str) -> anyhow::Result<()> {
    anyhow::bail!("FUSE implementation TODO (skeleton).")
}
