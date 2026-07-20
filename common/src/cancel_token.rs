use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

/// A shared, poll-only cooperative cancellation token.
///
/// It carries its own "no cancellation" case ([`CancelToken::never`], the
/// `Default`), so an operation takes a plain `CancelToken` rather than an
/// `Option<CancelToken>` — a caller that doesn't want cancellation passes a
/// never-token (zero-cost, no allocation) and everything downstream just polls
/// [`is_cancelled`](Self::is_cancelled).
///
/// A live token's flag is shared by all clones, so a holder on any thread can
/// [`cancel`](Self::cancel) work another thread polls. There's no async wait:
/// cooperative code checks it at safe points (a loop top, between work chunks)
/// and bails — exactly what a `spawn_blocking` / rayon hot loop can afford. For
/// an awaitable cancel, reach for `tokio_util`'s token instead.
///
/// A live token is one-shot: once cancelled, it and all of its clones remain
/// cancelled. Each operation that needs independent cancellation must create a
/// fresh token. Ordering is `Relaxed`: the flag is standalone (it publishes no
/// other data), so it needs no synchronization beyond the atomic.
#[derive(Clone, Debug, Default)]
pub struct CancelToken(State);

#[derive(Clone, Debug, Default)]
enum State {
    /// Never cancels — the zero-cost stand-in for "no cancellation wired".
    #[default]
    Never,
    /// A live flag; clones share it.
    Live(Arc<AtomicBool>),
}

impl CancelToken {
    /// A fresh, live token that can be cancelled.
    pub fn new() -> Self {
        Self(State::Live(Arc::new(AtomicBool::new(false))))
    }

    /// A token that never cancels (zero-cost opt-out; same as `default()`).
    pub fn never() -> Self {
        Self(State::Never)
    }

    /// Request cancellation. Idempotent; visible to every clone. A no-op on a
    /// never-token.
    pub fn cancel(&self) {
        if let State::Live(flag) = &self.0 {
            flag.store(true, Ordering::Relaxed);
        }
    }

    /// Whether cancellation has been requested. Always `false` for a never-token.
    pub fn is_cancelled(&self) -> bool {
        matches!(&self.0, State::Live(flag) if flag.load(Ordering::Relaxed))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cancellation_is_permanent_across_clones() {
        let token = CancelToken::new();
        let clone = token.clone();
        assert!(!token.is_cancelled());
        assert!(!clone.is_cancelled());

        token.cancel();
        assert!(token.is_cancelled());
        assert!(clone.is_cancelled());

        let fresh = CancelToken::new();
        assert!(!fresh.is_cancelled());
        assert!(token.is_cancelled());
        assert!(clone.is_cancelled());
    }

    #[test]
    fn never_token_ignores_cancel() {
        let token = CancelToken::never();
        token.cancel();
        assert!(!token.is_cancelled(), "a never-token can't be cancelled");
        // Default is a never-token.
        assert!(!CancelToken::default().is_cancelled());
    }
}
