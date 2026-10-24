//! [`WorkerPool`]: a pool of a set size, for a test or a bench that compares thread counts.

use std::cell::Cell;

use rayon::{ThreadPool, ThreadPoolBuilder};

thread_local! {
    /// Whether this thread runs a [`WorkerPool::run`] body: top-level code, on a worker.
    static TOP_LEVEL: Cell<bool> = const { Cell::new(false) };
}

/// A rayon pool of `threads` named threads, which runs a body as top-level code.
///
/// Shipped code runs on the global pool, whose workers only ever run loop bodies. A body
/// [`run`](Self::run) installs runs on one of this pool's workers instead, so it marks that
/// worker's thread as top-level for the [`par`](crate::par) check. The worker also runs bodies of
/// its own loops while it waits in them, and those pass the check too: nesting is caught on the
/// pool's other workers only.
#[derive(Debug)]
pub struct WorkerPool {
    pool: ThreadPool,
}

impl WorkerPool {
    /// A pool of `threads` threads named `{name}-{idx}`.
    pub fn new(name: impl Into<String>, threads: usize) -> Self {
        let name = name.into();
        let prefix = name.clone();
        let pool = ThreadPoolBuilder::new()
            .num_threads(threads)
            .thread_name(move |idx| format!("{prefix}-{idx}"))
            .build()
            .unwrap_or_else(|error| panic!("the OS starts the {name} pool: {error}"));
        Self { pool }
    }

    /// `body` on this pool, as top-level code; the caller blocks until it returns.
    ///
    /// # Panics
    /// On a worker of any rayon pool: a worker that waits on another pool keeps running its own
    /// pool's jobs.
    pub fn run<R: Send>(&self, body: impl FnOnce() -> R + Send) -> R {
        assert!(
            rayon::current_thread_index().is_none(),
            "a worker pool runs from outside every pool, never from a parallel loop's body"
        );
        self.pool.install(|| {
            let outer = TOP_LEVEL.replace(true);
            let _restore = Restore(outer);
            body()
        })
    }

    pub(super) fn is_top_level() -> bool {
        TOP_LEVEL.get()
    }
}

/// Puts back the worker's mark when a [`WorkerPool::run`] body returns or unwinds.
#[derive(Debug)]
struct Restore(bool);

impl Drop for Restore {
    fn drop(&mut self) {
        TOP_LEVEL.set(self.0);
    }
}
