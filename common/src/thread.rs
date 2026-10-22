//! Threads: the one route to spawning one outside `common`, as `clippy.toml` holds. Every thread
//! has a name, so a debugger, a profiler and a panic message say whose it is. The OS failing to
//! start a thread is no case a caller handles: it panics, naming the thread, as
//! `std::thread::spawn` does.

use std::thread::{self, Builder, JoinHandle, ScopedJoinHandle};

use rayon::{ThreadPool, ThreadPoolBuilder};

/// A rayon pool of `threads` threads named `{name}-{idx}`, for work that must run on a set count of
/// threads rather than on the global pool.
pub fn pool(name: impl Into<String>, threads: usize) -> ThreadPool {
    let name = name.into();
    let prefix = name.clone();
    ThreadPoolBuilder::new()
        .num_threads(threads)
        .thread_name(move |idx| format!("{prefix}-{idx}"))
        .build()
        .unwrap_or_else(|error| panic!("the OS starts the {name} pool: {error}"))
}

pub fn spawn<T: Send + 'static>(
    name: impl Into<String>,
    body: impl FnOnce() -> T + Send + 'static,
) -> JoinHandle<T> {
    let name = name.into();
    Builder::new()
        .name(name.clone())
        .spawn(body)
        .unwrap_or_else(|error| panic!("the OS starts thread {name}: {error}"))
}

/// `body` with a [`Scope`] its threads borrow from; every thread spawned in it is joined before
/// this returns, as `std::thread::scope` joins them.
pub fn scope<'env, T>(body: impl for<'scope> FnOnce(&Scope<'scope, 'env>) -> T) -> T {
    thread::scope(|inner| body(&Scope { inner }))
}

/// The threads a [`scope`] joins.
#[derive(Debug)]
pub struct Scope<'scope, 'env> {
    inner: &'scope thread::Scope<'scope, 'env>,
}

impl<'scope> Scope<'scope, '_> {
    pub fn spawn<T: Send + 'scope>(
        &self,
        name: impl Into<String>,
        body: impl FnOnce() -> T + Send + 'scope,
    ) -> ScopedJoinHandle<'scope, T> {
        let name = name.into();
        Builder::new()
            .name(name.clone())
            .spawn_scoped(self.inner, body)
            .unwrap_or_else(|error| panic!("the OS starts thread {name}: {error}"))
    }
}
