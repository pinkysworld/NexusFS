//! Entropy for the WebAssembly build.
//!
//! `getrandom` has no OS to call in a browser without a JS shim, and pulling in its
//! "js" backend would make the build depend on wasm-bindgen and its CLI. Instead the
//! host seeds us once from `crypto.getRandomValues`, and we expand that seed with
//! BLAKE3 in XOF mode.
//!
//! Nothing on the playground's code path actually draws randomness — identities are
//! built from a caller-supplied seed and no encryption runs — but the handler has to
//! exist for the crate to link, and it should be sound if that ever changes.

use core::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Mutex;

static SEED: Mutex<[u8; 32]> = Mutex::new([0u8; 32]);
static SEEDED: AtomicBool = AtomicBool::new(false);
static COUNTER: AtomicU64 = AtomicU64::new(0);

/// Install host-provided entropy. Called once at startup.
pub fn seed(bytes: [u8; 32]) {
    *SEED.lock().expect("seed poisoned") = bytes;
    SEEDED.store(true, Ordering::Release);
}

/// Whether the host has supplied entropy yet.
pub fn is_seeded() -> bool {
    SEEDED.load(Ordering::Acquire)
}

/// Expand the seed. Callers must check [`is_seeded`] first.
pub fn fill(dest: &mut [u8]) {
    let key = *SEED.lock().expect("seed poisoned");
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);

    let mut hasher = blake3::Hasher::new_keyed(&key);
    hasher.update(&n.to_le_bytes());
    hasher.finalize_xof().fill(dest);
}

#[cfg(target_arch = "wasm32")]
fn getrandom_shim(dest: &mut [u8]) -> Result<(), getrandom::Error> {
    // Refuse rather than expand an all-zero seed. Nothing on the playground's path
    // draws randomness today, so an unseeded call would go unnoticed — and the bytes it
    // returned would be fully predictable, which for a key is worse than an error.
    if !is_seeded() {
        return Err(getrandom::Error::from(
            core::num::NonZeroU32::new(getrandom::Error::CUSTOM_START + 1)
                .expect("custom error code is non-zero"),
        ));
    }
    fill(dest);
    Ok(())
}

#[cfg(target_arch = "wasm32")]
getrandom::register_custom_getrandom!(getrandom_shim);
