//! Browser build of the NexusFS local core.
//!
//! This is not a reimplementation. It drives the same `CoreState`, the same signed
//! operations and the same CRDT namespace state the native binary uses — only the
//! storage backend differs, which is possible because storage is a trait. That means
//! the playground demonstrates real convergence rather than an animation of it.
//!
//! The boundary is deliberately plain: JSON in, JSON out, over raw wasm exports with
//! no wasm-bindgen. That keeps the build to a single `cargo build --target
//! wasm32-unknown-unknown` with no extra toolchain, so CI can produce the artefact.

pub mod api;
pub mod replica;
pub mod rng;

use std::cell::RefCell;

thread_local! {
    /// Response buffer for the most recent `nx_dispatch`. The host reads it via
    /// `nx_response_ptr`/the returned length before issuing another call.
    static RESPONSE: RefCell<Vec<u8>> = const { RefCell::new(Vec::new()) };
}

/// Allocate `len` bytes for the host to write a request into.
///
/// # Safety
/// The returned pointer must be handed back to `nx_dispatch` or `nx_dealloc` with the
/// same length; anything else leaks or corrupts the allocator.
#[no_mangle]
pub extern "C" fn nx_alloc(len: usize) -> *mut u8 {
    let mut buf = Vec::<u8>::with_capacity(len);
    let ptr = buf.as_mut_ptr();
    core::mem::forget(buf);
    ptr
}

/// Release a buffer previously returned by `nx_alloc`.
///
/// # Safety
/// `ptr` must come from `nx_alloc` with the same `len`, and must not be used after.
#[no_mangle]
pub unsafe extern "C" fn nx_dealloc(ptr: *mut u8, len: usize) {
    if ptr.is_null() {
        return;
    }
    drop(Vec::from_raw_parts(ptr, 0, len));
}

/// Seed the entropy pool from the host. `ptr` must point at 32 bytes.
///
/// # Safety
/// `ptr` must be valid for 32 bytes of reads.
#[no_mangle]
pub unsafe extern "C" fn nx_seed(ptr: *const u8) {
    let mut seed = [0u8; 32];
    core::ptr::copy_nonoverlapping(ptr, seed.as_mut_ptr(), 32);
    rng::seed(seed);
}

/// Execute one JSON command and return the response length.
///
/// The response bytes live in the thread-local buffer until the next call; read them
/// with `nx_response_ptr`.
///
/// # Safety
/// `ptr`/`len` must describe an initialised buffer from `nx_alloc`.
#[no_mangle]
pub unsafe extern "C" fn nx_dispatch(ptr: *mut u8, len: usize) -> usize {
    let request = Vec::from_raw_parts(ptr, len, len);
    let response = api::dispatch(&request);
    RESPONSE.with(|slot| {
        let mut slot = slot.borrow_mut();
        *slot = response;
        slot.len()
    })
}

/// Pointer to the buffer filled by the most recent `nx_dispatch`.
#[no_mangle]
pub extern "C" fn nx_response_ptr() -> *const u8 {
    RESPONSE.with(|slot| slot.borrow().as_ptr())
}
