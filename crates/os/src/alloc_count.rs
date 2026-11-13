//! A memory allocator that counts what a thread asks for while it is
//! marked, for tests that prove code never allocates or frees (the sound
//! card's callback must not: either can wait on a lock inside the system).
//!
//! A test program installs it with
//! `#[global_allocator] static A: os::alloc_count::Counting = os::alloc_count::Counting;`
//! and runs the code under test inside [`counted`]. Everything is handed
//! to the system's allocator unchanged; only calls made on a marked thread
//! are counted. The app itself never installs it.
#![allow(unsafe_code)]

use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;
use std::sync::atomic::{AtomicUsize, Ordering};

/// The counting allocator.
#[derive(Debug, Clone, Copy, Default)]
pub struct Counting;

static COUNTED: AtomicUsize = AtomicUsize::new(0);

thread_local! {
    static MARKED: Cell<bool> = const { Cell::new(false) };
}

fn note() {
    // `try_with`: a thread being torn down has no locals left; its calls
    // are not counted.
    if MARKED.try_with(Cell::get).unwrap_or(false) {
        COUNTED.fetch_add(1, Ordering::Relaxed);
    }
}

// SAFETY: every call goes straight to the system allocator with the same
// arguments, so its guarantees are the system allocator's. Counting only
// touches an atomic and a thread-local flag that needs no allocation (a
// `const` thread-local of a `Cell<bool>`).
unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        note();
        // SAFETY: the caller keeps `alloc`'s contract; passed on unchanged.
        unsafe { System.alloc(layout) }
    }

    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        note();
        // SAFETY: as `alloc`.
        unsafe { System.alloc_zeroed(layout) }
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        note();
        // SAFETY: `ptr` came from this allocator, that is from `System`.
        unsafe { System.dealloc(ptr, layout) }
    }

    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        note();
        // SAFETY: as `dealloc`; the caller keeps `realloc`'s contract.
        unsafe { System.realloc(ptr, layout, new_size) }
    }
}

/// Runs `f` with this thread marked, and returns what it returned and how
/// many allocations, frees and reallocations it made (on this thread only).
/// Counts from every marked thread are added together, so tests that use
/// this run one at a time or in a program of their own.
pub fn counted<T>(f: impl FnOnce() -> T) -> (T, usize) {
    let before = COUNTED.load(Ordering::Relaxed);
    MARKED.with(|m| m.set(true));
    let out = f();
    MARKED.with(|m| m.set(false));
    (out, COUNTED.load(Ordering::Relaxed) - before)
}
