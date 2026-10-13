//! [`LiveHeap`]: the bytes this binary's allocations hold, counted as the allocator serves them.

use std::alloc::{GlobalAlloc, Layout, System};
use std::sync::atomic::{AtomicUsize, Ordering};

/// The process's allocator: [`System`], with a count of the bytes its allocations hold and the
/// most they held since [`Self::restart_peak`].
///
/// The kernel's figure for the process is no measure of this: glibc keeps freed blocks in
/// per-thread arenas, so the resident set holds what no allocation does.
#[derive(Debug)]
pub(crate) struct LiveHeap {
    live: AtomicUsize,
    peak: AtomicUsize,
}

impl LiveHeap {
    pub(crate) const fn new() -> Self {
        Self {
            live: AtomicUsize::new(0),
            peak: AtomicUsize::new(0),
        }
    }

    pub(crate) fn live_bytes(&self) -> usize {
        self.live.load(Ordering::Relaxed)
    }

    /// The most the allocations held since [`Self::restart_peak`].
    pub(crate) fn peak_bytes(&self) -> usize {
        self.peak.load(Ordering::Relaxed)
    }

    /// Count the peak again from what the allocations hold now, which this returns.
    pub(crate) fn restart_peak(&self) -> usize {
        let live = self.live_bytes();
        self.peak.store(live, Ordering::Relaxed);
        live
    }

    fn grew(&self, bytes: usize) {
        let live = self.live.fetch_add(bytes, Ordering::Relaxed) + bytes;
        self.peak.fetch_max(live, Ordering::Relaxed);
    }

    fn shrank(&self, bytes: usize) {
        self.live.fetch_sub(bytes, Ordering::Relaxed);
    }
}

unsafe impl GlobalAlloc for LiveHeap {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        self.grew(layout.size());
        unsafe { System.alloc(layout) }
    }

    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        self.grew(layout.size());
        unsafe { System.alloc_zeroed(layout) }
    }

    unsafe fn dealloc(&self, pointer: *mut u8, layout: Layout) {
        self.shrank(layout.size());
        unsafe { System.dealloc(pointer, layout) }
    }

    unsafe fn realloc(&self, pointer: *mut u8, layout: Layout, size: usize) -> *mut u8 {
        if size > layout.size() {
            self.grew(size - layout.size());
        } else {
            self.shrank(layout.size() - size);
        }
        unsafe { System.realloc(pointer, layout, size) }
    }
}
