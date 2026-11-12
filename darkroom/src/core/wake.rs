//! The host-wake callback the evaluation worker fires.

use std::sync::Arc;

/// The "wake the host loop" callback the worker fires after it posts a result, so the main loop
/// re-drains. Wired to [`palantir::Waker::wake`], which keeps `core` free of a frontend type.
/// Refused: `palantir::Waker` itself, which saves one indirect call a wake and puts a GUI type
/// in `core`.
pub(crate) type Wake = Arc<dyn Fn() + Send + Sync>;

#[cfg(feature = "internals")]
pub(crate) mod internals {
    use std::sync::Arc;

    use crate::core::wake::Wake;

    /// A wake with no host loop behind it, for a fixture or bench that waits for work itself.
    pub(crate) fn noop() -> Wake {
        Arc::new(|| {})
    }
}
