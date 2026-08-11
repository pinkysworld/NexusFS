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
///
/// v2 replaced the flat inode-map commitment with a Merkle root. Nothing stored changed
/// shape — the state root is derived — but its *value* did, and two builds that disagree
/// on the state root would never converge. That is what the stamp is for.
pub const CURRENT_FORMAT_VERSION: u32 = 2;

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
            .put_kv(CF_META, KEY_FORMAT_VERSION, &version.to_be_bytes())?;
        // The stamp is what a later run trusts to decide whether it may open the store
        // at all, so it must not be the thing lost to an unclean exit.
        self.flush()
    }

    /// Record that this repository is on the format this build writes.
    ///
    /// Called when a repository is created, which is the only moment its format is known
    /// rather than inferred.
    pub(crate) fn stamp_current_format(&self) -> Result<()> {
        self.set_format_version(CURRENT_FORMAT_VERSION)
    }

    /// Inspect the format, stamping unversioned repositories on the way.
    ///
    /// An unstamped store is one of two things, and the difference matters: a
    /// repository written before versioning existed, which is v1 and needs migrating;
    /// or one that has never been written at all, which is whatever this build is about
    /// to make it.
    ///
    /// A head separates them. Bootstrapping sets one, so every real repository has a
    /// head and an empty directory does not — and this runs *before* bootstrap, which is
    /// exactly when that distinction is still visible. Getting it wrong would make every
    /// newly created repository demand a migration on the spot.
    pub fn check_format(&self) -> Result<FormatState> {
        let found = match self.format_version()? {
            Some(v) => v,
            None => {
                let version = if self.get_head()?.is_some() {
                    FIRST_FORMAT_VERSION
                } else {
                    CURRENT_FORMAT_VERSION
                };
                self.set_format_version(version)?;
                version
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
        if from == 1 {
            // v1 -> v2: the inode-map commitment became a Merkle root.
            //
            // No record changes shape, so there is nothing to rewrite. What is stale is
            // the *derived* state root and the head object built from it, and both are
            // reproducible from live state — so the migration is a re-snapshot rather
            // than a rewrite. That is also why it is safe to interrupt: rerunning it
            // recomputes the same thing.
            info!("migrating to on-disk format v2: rebuilding the state commitment");
            self.build_snapshot()?;
            return self.set_format_version(2);
        }
        bail!("no migration is implemented from on-disk format v{from}")
    }
}
