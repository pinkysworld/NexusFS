//! On-disk format versioning.
//!
//! # Why a stored version at all
//!
//! Every record in the store is `postcard`-encoded, which is compact precisely because
//! it carries no field names or type tags. That makes it fast and deterministic, and it
//! means a decoder given bytes from a different schema does not reliably fail — it can
//! succeed and produce nonsense. A version stamp is what turns that silent
//! misinterpretation into a refusal.
//!
//! # Why opening refuses instead of migrating
//!
//! An old store is opened by a binary that could upgrade it, so upgrading automatically
//! is tempting. It is the wrong default: a migration rewrites data in place, and the
//! operator may not have a backup, may be running the wrong binary by accident, or may
//! have several nodes mid-rollout. Refusing costs one command. Migrating without being
//! asked can cost the repository.
//!
//! A *newer* store is refused outright and cannot be forced. This build cannot know
//! what a later format means, and guessing would apply an old state machine to records
//! it does not understand.
//!
//! # Adding a version
//!
//! Bump [`CURRENT_FORMAT_VERSION`], add an arm to [`CoreState::migrate`] that moves
//! `n -> n + 1`, and leave the earlier arms alone. Migrations run in sequence, so a
//! repository several versions behind upgrades one step at a time and each step only
//! has to understand its immediate predecessor.

use anyhow::{bail, Result};
use tracing::info;

use crate::state::{CoreState, CF_META};

/// The format this build reads and writes.
pub const CURRENT_FORMAT_VERSION: u32 = 1;

/// The version an unstamped repository is taken to be. Never changes: it names the
/// format that existed before the stamp did, not the current one.
const FIRST_FORMAT_VERSION: u32 = 1;

const KEY_FORMAT_VERSION: &[u8] = b"format/version";

/// What opening a repository found.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FormatState {
    /// Matches this build. Safe to use.
    Current,
    /// Older than this build. Needs `nexusfs migrate` before use.
    NeedsMigration { found: u32 },
    /// Written by a later build. Not usable here at all.
    TooNew { found: u32 },
}

impl CoreState {
    /// The version recorded in the store, if any.
    ///
    /// `None` means the repository predates versioning *or* is brand new; the caller
    /// distinguishes those by whether anything has been written yet.
    pub fn format_version(&self) -> Result<Option<u32>> {
        let Some(raw) = self.stores.kv.get_kv(CF_META, KEY_FORMAT_VERSION)? else {
            return Ok(None);
        };
        if raw.len() != 4 {
            bail!("format version record is {} bytes, expected 4", raw.len());
        }
        let mut b = [0u8; 4];
        b.copy_from_slice(&raw);
        Ok(Some(u32::from_be_bytes(b)))
    }

    fn set_format_version(&self, version: u32) -> Result<()> {
        self.stores
            .kv
            .put_kv(CF_META, KEY_FORMAT_VERSION, &version.to_be_bytes())
    }

    /// Inspect the format, stamping unversioned repositories on the way.
    ///
    /// An unstamped store is version 1 by definition: versioning was introduced
    /// alongside it, so anything without a stamp was written by a build whose format
    /// *is* version 1. Recording that is not a migration, it is filling in a fact.
    pub fn check_format(&self) -> Result<FormatState> {
        let found = match self.format_version()? {
            Some(v) => v,
            None => {
                self.set_format_version(FIRST_FORMAT_VERSION)?;
                FIRST_FORMAT_VERSION
            }
        };

        Ok(match found.cmp(&CURRENT_FORMAT_VERSION) {
            std::cmp::Ordering::Equal => FormatState::Current,
            std::cmp::Ordering::Less => FormatState::NeedsMigration { found },
            std::cmp::Ordering::Greater => FormatState::TooNew { found },
        })
    }

    /// Refuse to proceed unless the store matches this build.
    ///
    /// Called on every path that opens a repository, so a mismatch surfaces as a clear
    /// message at startup rather than as corrupt reads later.
    pub fn require_current_format(&self) -> Result<()> {
        match self.check_format()? {
            FormatState::Current => Ok(()),
            FormatState::NeedsMigration { found } => bail!(
                "this repository is on-disk format v{found}, but this build expects \
                 v{CURRENT_FORMAT_VERSION}. Back it up, then run `nexusfs migrate` to \
                 upgrade it."
            ),
            FormatState::TooNew { found } => bail!(
                "this repository is on-disk format v{found}, which is newer than the \
                 v{CURRENT_FORMAT_VERSION} this build understands. Upgrade NexusFS; \
                 there is no way to open it safely with this version."
            ),
        }
    }

    /// Upgrade the store to [`CURRENT_FORMAT_VERSION`], one step at a time.
    ///
    /// Returns the versions it moved through, so the caller can report what happened
    /// (empty when the repository was already current).
    pub fn migrate(&self) -> Result<Vec<u32>> {
        let mut applied = Vec::new();

        loop {
            match self.check_format()? {
                FormatState::Current => break,
                FormatState::TooNew { found } => bail!(
                    "cannot migrate: the repository is on format v{found}, newer than \
                     the v{CURRENT_FORMAT_VERSION} this build understands"
                ),
                FormatState::NeedsMigration { found } => {
                    self.migrate_step(found)?;
                    let now = self.format_version()?.unwrap_or(found);
                    if now <= found {
                        // Without this the loop would spin forever on a step that
                        // reported success but did not advance the stamp.
                        bail!(
                            "migration from format v{found} did not advance the recorded \
                             version; refusing to continue"
                        );
                    }
                    applied.push(now);
                }
            }
        }

        if applied.is_empty() {
            info!(
                version = CURRENT_FORMAT_VERSION,
                "repository already current"
            );
        }
        Ok(applied)
    }

    /// Move the store from `from` to `from + 1`, ending with the stamp updated.
    fn migrate_step(&self, from: u32) -> Result<()> {
        // The first real migration goes here, shaped like:
        //
        //     if from == 1 {
        //         ...rewrite the affected records...
        //         return self.set_format_version(2);
        //     }
        //
        // v1 is the earliest format, so today there is nothing below it to come from
        // and any request to migrate is a corrupt or hand-edited stamp.
        bail!("no migration is implemented from on-disk format v{from}")
    }
}
