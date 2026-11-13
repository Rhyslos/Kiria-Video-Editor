//! Counts allocations and frees made on a thread while it is marked as
//! the real-time thread. The answer must be zero.

use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;
use std::sync::atomic::{AtomicUsize, Ordering};

pub struct Counting;

static ON_RT: AtomicUsize = AtomicUsize::new(0);

thread_local! {
    static MARKED: Cell<bool> = const { Cell::new(false) };
}

fn note() {
    if MARKED.try_with(|m| m.get()).unwrap_or(false) {
        ON_RT.fetch_add(1, Ordering::Relaxed);
    }
}

// SAFETY: hands every call straight to the system allocator.
unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        note();
        unsafe { System.alloc(layout) }
    }
    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        note();
        unsafe { System.dealloc(ptr, layout) }
    }
    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new: usize) -> *mut u8 {
        note();
        unsafe { System.realloc(ptr, layout, new) }
    }
}

/// Runs `f` with this thread marked as the real-time thread.
pub fn on_rt<T>(f: impl FnOnce() -> T) -> T {
    MARKED.with(|m| m.set(true));
    let out = f();
    MARKED.with(|m| m.set(false));
    out
}

pub fn count() -> usize {
    ON_RT.load(Ordering::Relaxed)
}

pub fn reset() {
    ON_RT.store(0, Ordering::Relaxed);
}

/// Makes very small numbers count as zero on this thread, so filters and
/// envelopes dying away never fall into the slow arithmetic for them.
pub fn flush_denormals() {
    #[cfg(target_arch = "x86_64")]
    #[allow(deprecated)]
    // SAFETY: sets the flush-to-zero and denormals-are-zero bits of this
    // thread's floating-point control register; nothing else changes.
    unsafe {
        use std::arch::x86_64::{_mm_getcsr, _mm_setcsr};
        _mm_setcsr(_mm_getcsr() | 0x8040);
    }
}
