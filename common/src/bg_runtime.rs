//! A dedicated background tokio runtime: the one route to building one outside `common`, as
//! `clippy.toml` holds.
//!
//! An owner builds one, `enter`s it just long enough to construct its tokio-spawning inner
//! value, then holds it for its `Drop` — dropping the runtime shuts down the threads that value's
//! tasks run on. The wrapper exists to make that drop order a property of the type rather than a
//! comment on a field: declared after the value, so it drops after it.

use tokio::runtime::{Builder, Runtime};

/// A dedicated background tokio runtime, held for its `Drop` after the value it hosts.
#[derive(Debug)]
pub struct BgRuntime {
    runtime: Runtime,
}

impl BgRuntime {
    /// Build a fresh multi-thread runtime with all drivers enabled, its threads named `name`.
    /// The OS failing to start it panics, naming it, as a thread that cannot start does.
    pub fn new(name: impl Into<String>) -> Self {
        let name = name.into();
        let runtime = Builder::new_multi_thread()
            .thread_name(name.clone())
            .enable_all()
            .build()
            .unwrap_or_else(|error| panic!("the OS starts the {name} runtime: {error}"));
        Self { runtime }
    }

    /// Run `f` with this runtime entered, so any `tokio::spawn` inside `f`
    /// lands on it.
    pub fn enter<T>(&self, f: impl FnOnce() -> T) -> T {
        let _guard = self.runtime.enter();
        f()
    }

    /// Drive shutdown work on the owned runtime before its threads are dropped.
    pub fn block_on<F: Future>(&self, future: F) -> F::Output {
        self.runtime.block_on(future)
    }
}

#[cfg(test)]
mod tests {
    use crate::BgRuntime;

    #[test]
    fn enter_spawns_on_owned_runtime_and_block_on_joins_the_task() {
        let rt = BgRuntime::new("bg-runtime-test");
        let task = rt.enter(|| tokio::spawn(async { 42 }));
        assert_eq!(rt.block_on(task).unwrap(), 42);
    }
}
