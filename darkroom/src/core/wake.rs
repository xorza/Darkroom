//! The host-wake callback the evaluation worker fires.

use std::sync::Arc;

/// Opaque "wake the host loop" callback, fired from the worker thread after
/// it posts a result so the main loop re-drains. Wired to
/// [`palantir::HostHandle::request_repaint`], which keeps the worker module
/// free of any specific frontend type.
pub(crate) type Wake = Arc<dyn Fn() + Send + Sync>;

#[cfg(feature = "internals")]
pub(crate) mod internals {
    use std::sync::Arc;

    use crate::core::wake::Wake;

    /// A wake with no host loop behind it — for a fixture or a bench, which
    /// waits for background work itself instead of being woken for it.
    pub(crate) fn noop() -> Wake {
        Arc::new(|| {})
    }
}
