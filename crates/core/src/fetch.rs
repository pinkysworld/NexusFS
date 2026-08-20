//! Fetching content a node deliberately did not take.
//!
//! The energy scheduler can decide to accept operations and defer the bytes. That
//! leaves a node that knows a file exists, where it lives and what it is made of — and
//! cannot read it. Waiting for the next unconstrained sync pass is a poor answer when
//! somebody is asking for the file right now.
//!
//! `core` has no business knowing about peers, so it does not do the fetching. It
//! exposes the question — which chunks would this read need? — and this trait, which a
//! facade fills in with whatever transport the daemon has. The layering stays intact:
//! the filesystem asks, the daemon answers.

use std::future::Future;
use std::pin::Pin;

use anyhow::Result;
use nexusfs_storage::Hash;

/// A fetch in flight. Boxed so the trait stays object-safe without an async-trait
/// dependency in a crate that is otherwise entirely synchronous.
pub type FetchFuture<'a> = Pin<Box<dyn Future<Output = Result<usize>> + Send + 'a>>;

/// Somewhere content can be obtained from on demand.
pub trait ContentFetcher: Send + Sync {
    /// Try to obtain `hashes`, returning how many were stored.
    ///
    /// Partial success is normal and not an error: one peer may hold some of what is
    /// wanted. The caller re-checks what it actually needs rather than trusting the
    /// count.
    fn fetch<'a>(&'a self, hashes: &'a [Hash]) -> FetchFuture<'a>;
}

/// Bring in whatever a read of `path` needs, in as many rounds as it takes.
///
/// More than one round is normal rather than exceptional: a node that deferred content
/// is missing the `FileNode` too, and only once that arrives can it name the chunks it
/// describes. Bounded because each round must learn something new — a round that asks
/// for the same set again is a peer that cannot help, not progress.
///
/// Returns whatever is *still* missing, so the caller can tell "fetched" from "asked and
/// did not get it" and answer honestly instead of serving a short file.
pub async fn ensure_content(
    core: &crate::state::CoreState,
    fetcher: &dyn ContentFetcher,
    path: &str,
) -> Result<Vec<Hash>> {
    /// One round for the file object, one for its chunks, and slack for a peer that
    /// answers partially.
    const MAX_ROUNDS: usize = 4;

    let mut wanted = core.missing_chunks_for_path(path)?;
    for _ in 0..MAX_ROUNDS {
        if wanted.is_empty() {
            break;
        }
        fetcher.fetch(&wanted).await?;

        let next = core.missing_chunks_for_path(path)?;
        if next == wanted {
            // Nothing changed, so asking again would ask the same question.
            break;
        }
        wanted = next;
    }
    Ok(wanted)
}
