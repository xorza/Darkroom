//! Shared RAII helper for a dedicated background tokio runtime.
//!
//! `WorkerBridge` and `ScriptHost` each spin up their own dedicated
//! multi-thread runtime, `enter()` it just long enough to construct their
//! tokio-spawning inner value (`Worker`, `ScriptExecutor`), then hold the
//! `Runtime` only for its `Drop` — dropping it shuts down the threads the
//! inner value's tasks run on. [`BackgroundRuntime`] captures that pattern
//! once so both owners just build one, `enter` it to construct their inner
//! value, and store it alongside for drop order.

use std::future::Future;

use tokio::runtime::{Builder, Runtime};

/// A dedicated background tokio runtime, held only for its `Drop`. Build
/// one with [`BackgroundRuntime::new`], use [`BackgroundRuntime::enter`] to
/// construct a `tokio::spawn`-ing value inside its ambient context, then
/// keep this alongside that value — declare it after, so it drops after
/// (runtime shutdown happens once the tasks it hosts are gone).
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
