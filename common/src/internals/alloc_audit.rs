//! [`CountingAllocator`]: a per-thread counting allocator, and the window an allocation gate
//! measures inside. A test binary installs it as its `#[global_allocator]`.
//!
//! Per-thread because cargo runs tests in parallel in one process: a global counter would fold
//! other tests' setup allocations in. Only the auditing thread counts, and only inside
//! [`allocations`].
//!
//! The metric is heap operations on the measured path, not residency: a pass that reuses its
//! buffers performs none, one that rebuilds them one per buffer however small.

#![expect(
    clippy::disallowed_types,
    reason = "a global allocator has no owner to carry its counters"
)]

use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;

thread_local! {
    /// `const` initializers of drop-free types register no thread destructor, so the allocator
    /// pays no lazy-init check.
    static IN_AUDIT: Cell<bool> = const { Cell::new(false) };
    static ALLOCS: Cell<u64> = const { Cell::new(0) };
}

/// The system allocator, counting the allocations of a thread inside [`allocations`].
#[derive(Debug)]
pub struct CountingAllocator;

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

/// `try_with` rather than `with`: an allocator that unwinds is worse than a miscount.
#[inline]
fn track() {
    IN_AUDIT
        .try_with(|in_audit| {
            if in_audit.get() {
                ALLOCS.try_with(|allocs| allocs.set(allocs.get() + 1)).ok();
            }
        })
        .ok();
}

/// Heap allocations this thread performs while `body` runs.
pub fn allocations(body: impl FnOnce()) -> u64 {
    ALLOCS.with(|allocs| allocs.set(0));
    let _window = AuditWindow::open();
    body();
    ALLOCS.with(Cell::get)
}

/// Closes the window on drop, so a panic in the measured body cannot leave every later allocation
/// on this thread counted.
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
