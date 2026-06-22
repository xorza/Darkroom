use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

/// A shared, poll-only cooperative cancellation flag.
///
/// Clones share one underlying flag, so a holder on any thread can
/// [`cancel`](Self::cancel) work that another thread observes via
/// [`is_cancelled`](Self::is_cancelled). It carries no async wait: cooperative
/// code checks it at safe points (a loop top, between work chunks) and bails —
/// which is exactly what a `spawn_blocking` / rayon hot loop can afford. For an
/// awaitable cancel, reach for `tokio_util`'s token instead.
///
/// **Reusable across operations.** [`reset`](Self::reset) clears the flag so one
/// long-lived token can gate a *sequence* of operations: the owner clears it at
/// each operation's start, so a cancel only affects the operation in flight when
/// it was requested (a cancel issued while idle is cleared by the next start).
///
/// Ordering is `Relaxed`: the flag is standalone (it publishes no other data),
/// so it needs no synchronization beyond the atomic itself.
#[derive(Clone, Debug, Default)]
pub struct CancelToken(Arc<AtomicBool>);

impl CancelToken {
    /// A fresh, un-cancelled token.
    pub fn new() -> Self {
        Self::default()
    }

    /// Request cancellation. Idempotent; visible to every clone.
    pub fn cancel(&self) {
        self.0.store(true, Ordering::Relaxed);
    }

    /// Whether cancellation has been requested.
    pub fn is_cancelled(&self) -> bool {
        self.0.load(Ordering::Relaxed)
    }

    /// Clear the flag so the token can gate a fresh operation.
    pub fn reset(&self) {
        self.0.store(false, Ordering::Relaxed);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cancel_is_observable_through_clones_and_resettable() {
        let token = CancelToken::new();
        let clone = token.clone();
        assert!(!token.is_cancelled());
        assert!(!clone.is_cancelled());

        // A cancel on one handle is seen by the other (shared flag).
        token.cancel();
        assert!(token.is_cancelled());
        assert!(clone.is_cancelled());

        // Reset clears it for reuse, again visible through clones.
        token.reset();
        assert!(!token.is_cancelled());
        assert!(!clone.is_cancelled());
    }
}
