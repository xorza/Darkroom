//! [`LiveHeap`]: the bytes this binary's allocations hold, counted as the allocator serves them.

use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;
use std::sync::atomic::{AtomicBool, AtomicIsize, Ordering};

/// The process's allocator: [`System`], with a count of the bytes the allocations of every thread
/// but the harness's own hold, and the most they held since [`Self::restart_peak`].
///
/// The kernel's figure for the process is no measure of this: glibc keeps freed blocks in
/// per-thread arenas, so the resident set holds what no allocation does.
///
/// The harness's thread — libtest's main thread, which starts the test on a thread of its own and
/// waits for its result — allocates on its own time: its first wait for that result builds the
/// channel's waker, inside a measurement when the test reaches one first. It is the one thread
/// left out. Every other thread is counted, the threads a run starts among them, so the count is
/// never less than what the work held.
#[derive(Debug)]
pub(crate) struct LiveHeap {
    live: AtomicIsize,
    peak: AtomicIsize,
    /// Whether a thread allocated yet: the first to is the harness's.
    harness_seen: AtomicBool,
}

/// What a thread is to the count, decided at its first allocation or free.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Role {
    Unseen,
    /// The process's first thread to allocate, before it started any other: the harness's.
    Harness,
    Counted,
}

thread_local! {
    static ROLE: Cell<Role> = const { Cell::new(Role::Unseen) };
}

impl LiveHeap {
    pub(crate) const fn new() -> Self {
        Self {
            live: AtomicIsize::new(0),
            peak: AtomicIsize::new(0),
            harness_seen: AtomicBool::new(false),
        }
    }

    /// What the counted allocations hold. A counted thread may free a block the harness's
    /// allocated, so the figure is a difference, not a size.
    pub(crate) fn live_bytes(&self) -> isize {
        self.live.load(Ordering::Relaxed)
    }

    /// The most the counted allocations held since [`Self::restart_peak`].
    pub(crate) fn peak_bytes(&self) -> isize {
        self.peak.load(Ordering::Relaxed)
    }

    /// Count the peak again from what the counted allocations hold now, which this returns.
    pub(crate) fn restart_peak(&self) -> isize {
        let live = self.live_bytes();
        self.peak.store(live, Ordering::Relaxed);
        live
    }

    /// Whether this thread's allocations are counted: every thread's but the harness's.
    pub(crate) fn counts_this_thread(&self) -> bool {
        // A thread whose locals are gone is counted: a count can over-read, never under-read.
        ROLE.try_with(|role| {
            if role.get() == Role::Unseen {
                role.set(if self.harness_seen.swap(true, Ordering::Relaxed) {
                    Role::Counted
                } else {
                    Role::Harness
                });
            }
            role.get() == Role::Counted
        })
        .unwrap_or(true)
    }

    /// Count `from` bytes becoming `to` on this thread: an allocation from 0, a free to 0.
    fn resized(&self, from: usize, to: usize) {
        if self.counts_this_thread() {
            let size = |bytes| isize::try_from(bytes).expect("an allocation fits in isize");
            let delta = size(to) - size(from);
            let live = self.live.fetch_add(delta, Ordering::Relaxed) + delta;
            self.peak.fetch_max(live, Ordering::Relaxed);
        }
    }
}

unsafe impl GlobalAlloc for LiveHeap {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        self.resized(0, layout.size());
        unsafe { System.alloc(layout) }
    }

    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        self.resized(0, layout.size());
        unsafe { System.alloc_zeroed(layout) }
    }

    unsafe fn dealloc(&self, pointer: *mut u8, layout: Layout) {
        self.resized(layout.size(), 0);
        unsafe { System.dealloc(pointer, layout) }
    }

    unsafe fn realloc(&self, pointer: *mut u8, layout: Layout, size: usize) -> *mut u8 {
        self.resized(layout.size(), size);
        unsafe { System.realloc(pointer, layout, size) }
    }
}
