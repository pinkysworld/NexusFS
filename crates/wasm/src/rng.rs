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

use core::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;

static SEED: Mutex<[u8; 32]> = Mutex::new([0u8; 32]);
static COUNTER: AtomicU64 = AtomicU64::new(0);

/// Install host-provided entropy. Called once at startup.
pub fn seed(bytes: [u8; 32]) {
    *SEED.lock().expect("seed poisoned") = bytes;
}

pub fn fill(dest: &mut [u8]) {
    let key = *SEED.lock().expect("seed poisoned");
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);

    let mut hasher = blake3::Hasher::new_keyed(&key);
    hasher.update(&n.to_le_bytes());
    hasher.finalize_xof().fill(dest);
}

#[cfg(target_arch = "wasm32")]
fn getrandom_shim(dest: &mut [u8]) -> Result<(), getrandom::Error> {
    fill(dest);
    Ok(())
}

#[cfg(target_arch = "wasm32")]
getrandom::register_custom_getrandom!(getrandom_shim);
