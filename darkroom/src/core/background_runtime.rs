//! RAII wrapper for [`WorkerBridge`](crate::core::worker::WorkerBridge)'s
//! dedicated background tokio runtime.
//!
//! The bridge builds one, `enter`s it just long enough to construct its
//! tokio-spawning inner value (`Worker`), then holds it for its `Drop` —
//! dropping the runtime shuts down the threads that value's tasks run on. The
//! wrapper exists to make that drop order a property of the type rather than a
//! comment on a field: declared after the `Worker`, so it drops after it.

use std::future::Future;

use tokio::runtime::{Builder, Runtime};

/// A dedicated background tokio runtime, held for its `Drop`. Build one with
/// [`BackgroundRuntime::new`], use [`BackgroundRuntime::enter`] to construct a
/// `tokio::spawn`-ing value inside its ambient context, then keep this
/// alongside that value — declared *after* it, so it drops after (runtime
/// shutdown happens once the tasks it hosts are gone).
#[derive(Debug)]
pub(crate) struct BackgroundRuntime {
    runtime: Runtime,
}

impl BackgroundRuntime {
    /// Build a fresh multi-thread runtime with all drivers enabled.
    pub(crate) fn new() -> std::io::Result<Self> {
        let runtime = Builder::new_multi_thread().enable_all().build()?;
        Ok(Self { runtime })
    }

    /// Run `f` with this runtime entered, so any `tokio::spawn` inside `f`
    /// lands on it.
    pub(crate) fn enter<T>(&self, f: impl FnOnce() -> T) -> T {
        let _guard = self.runtime.enter();
        f()
    }

    /// Drive shutdown work on the owned runtime before its threads are dropped.
    pub(crate) fn block_on<F: Future>(&self, future: F) -> F::Output {
        self.runtime.block_on(future)
    }
}

#[cfg(test)]
mod tests {
    use super::BackgroundRuntime;

    #[test]
    fn enter_spawns_on_owned_runtime_and_block_on_joins_the_task() {
        let rt = BackgroundRuntime::new().unwrap();
        let task = rt.enter(|| tokio::spawn(async { 42 }));
        assert_eq!(rt.block_on(task).unwrap(), 42);
    }
}
