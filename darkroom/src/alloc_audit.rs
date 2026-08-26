//! Per-thread counting allocator, and the window the record-path allocation
//! gate measures inside.
//!
//! Per-thread rather than global because cargo runs tests in parallel in one
//! process: a global counter would fold other tests' setup allocations into
//! the audit. Only the auditing thread counts, and only while it sits inside
//! [`allocations`].
//!
//! `dealloc` is delegated unchanged. The metric is heap *operations* on the
//! record path, not residency: a frame that reuses its buffers performs none,
//! and one that rebuilds them performs one per buffer however small each is.

use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;

#[global_allocator]
static ALLOCATOR: CountingAllocator = CountingAllocator;

thread_local! {
    /// `const` initializers of drop-free types: neither key registers a
    /// thread destructor, so neither costs the allocator a lazy-init check.
    static IN_AUDIT: Cell<bool> = const { Cell::new(false) };
    static ALLOCS: Cell<u64> = const { Cell::new(0) };
}

#[derive(Debug)]
struct CountingAllocator;

unsafe impl GlobalAlloc for CountingAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        track();
        unsafe { System.alloc(layout) }
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        unsafe { System.dealloc(ptr, layout) }
    }

    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        track();
        unsafe { System.alloc_zeroed(layout) }
    }

    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        track();
        unsafe { System.realloc(ptr, layout, new_size) }
    }
}

/// `try_with` rather than `with`: an allocator that unwinds is a worse
/// failure than a miscount, and the counters are the one thing here that
/// must never panic.
#[inline]
fn track() {
    let _ = IN_AUDIT.try_with(|in_audit| {
        if in_audit.get() {
            let _ = ALLOCS.try_with(|allocs| allocs.set(allocs.get() + 1));
        }
    });
}

/// Heap allocations this thread performs while `body` runs.
pub(crate) fn allocations(body: impl FnOnce()) -> u64 {
    ALLOCS.with(|allocs| allocs.set(0));
    let _window = AuditWindow::open();
    body();
    ALLOCS.with(Cell::get)
}

/// Closes the window on drop, so a panic inside the measured body cannot
/// strand the flag and leave every later allocation on this thread counted.
#[derive(Debug)]
struct AuditWindow;

impl AuditWindow {
    fn open() -> Self {
        IN_AUDIT.with(|in_audit| {
            assert!(!in_audit.get(), "allocation audit windows do not nest");
            in_audit.set(true);
        });
        Self
    }
}

impl Drop for AuditWindow {
    fn drop(&mut self) {
        IN_AUDIT.with(|in_audit| in_audit.set(false));
    }
}
