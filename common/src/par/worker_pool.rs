//! [`WorkerPool`]: the threads parallel work runs on, and the one source of a [`Par`].

use std::sync::Arc;

use rayon::{ThreadPool, ThreadPoolBuilder};

use crate::par::Par;

/// A rayon pool of named threads, shared by clones. Its owner decides how many workers the
/// program's parallel work spreads over; [`run`](Self::run) is how that work starts.
#[derive(Debug, Clone)]
pub struct WorkerPool {
    pool: Arc<ThreadPool>,
}

impl WorkerPool {
    /// A pool of `threads` threads named `{name}-{idx}`. The OS failing to start one is no case a
    /// caller handles: it panics, naming the pool, as `std::thread::spawn` does.
    pub fn new(name: impl Into<String>, threads: usize) -> Self {
        let name = name.into();
        let prefix = name.clone();
        let pool = ThreadPoolBuilder::new()
            .num_threads(threads)
            .thread_name(move |idx| format!("{prefix}-{idx}"))
            .build()
            .unwrap_or_else(|error| panic!("the OS starts the {name} pool: {error}"));
        Self {
            pool: Arc::new(pool),
        }
    }

    /// The workers [`run`](Self::run) spreads over.
    pub fn threads(&self) -> usize {
        self.pool.current_num_threads()
    }

    /// `body` on this pool, with the [`Par`] its parallel work starts from; the caller blocks
    /// until it returns.
    ///
    /// # Panics
    /// On a worker of any rayon pool. A worker that waits on another pool keeps running its own
    /// pool's jobs, so a body that reached a pool would nest work the types otherwise refuse; this
    /// is the one path they leave open, and a cold one, so it is checked at run time.
    pub fn run<R: Send>(&self, body: impl FnOnce(&Par<'_>) -> R + Send) -> R {
        assert!(
            rayon::current_thread_index().is_none(),
            "a worker pool runs from outside every pool, never from a parallel loop's body"
        );
        self.pool.install(|| body(&Par::new(&self.pool)))
    }
}
