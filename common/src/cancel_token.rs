use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

/// A shared, poll-only cooperative cancellation token.
///
/// It carries its own "no cancellation" case ([`CancelToken::never`], the `Default`), so an
/// operation takes a plain `CancelToken` rather than an `Option<CancelToken>`; a never-token costs
/// no allocation.
///
/// A live token's flag is shared by all clones, so a holder on any thread can
/// [`cancel`](Self::cancel) work another thread polls. There is no async wait: cooperative code
/// polls [`is_cancelled`](Self::is_cancelled) at safe points, which a rayon hot loop can afford.
/// For an awaitable cancel, use `tokio_util`'s token.
///
/// A live token stays cancelled until [`reset`](Self::reset), which rearms every clone and is safe
/// only once all work from the previous operation has joined. Ordering is `Relaxed`: the flag
/// publishes no other data.
#[derive(Clone, Debug, Default)]
pub struct CancelToken(State);

#[derive(Clone, Debug, Default)]
enum State {
    /// Never cancels.
    #[default]
    Never,
    /// A live flag; clones share it.
    Live(Arc<AtomicBool>),
}

impl CancelToken {
    /// How many items a cancellable walk covers between polls of its token. Not tuned to any one
    /// walk: small enough that a cancelled run stops promptly, large enough that the poll is lost
    /// in the work.
    pub const POLL_CHUNK: usize = 16_384;

    /// A fresh, live token that can be cancelled.
    pub fn new() -> Self {
        Self(State::Live(Arc::new(AtomicBool::new(false))))
    }

    /// A token that never cancels; same as `default()`.
    pub const fn never() -> Self {
        Self(State::Never)
    }

    /// Request cancellation. Idempotent; visible to every clone. A no-op on a
    /// never-token.
    pub fn cancel(&self) {
        if let State::Live(flag) = &self.0 {
            flag.store(true, Ordering::Relaxed);
        }
    }

    /// Rearm a live token and all of its clones for a new operation.
    pub fn reset(&self) {
        if let State::Live(flag) = &self.0 {
            flag.store(false, Ordering::Relaxed);
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
    fn cancellation_and_reset_are_shared_across_clones() {
        let token = CancelToken::new();
        let clone = token.clone();
        assert!(!token.is_cancelled());
        assert!(!clone.is_cancelled());

        token.cancel();
        assert!(token.is_cancelled());
        assert!(clone.is_cancelled());

        clone.reset();
        assert!(!token.is_cancelled());
        assert!(!clone.is_cancelled());
    }

    #[test]
    fn never_token_ignores_cancel() {
        let token = CancelToken::never();
        token.cancel();
        token.reset();
        assert!(!token.is_cancelled(), "a never-token can't be cancelled");
        assert!(!CancelToken::default().is_cancelled());
    }
}
