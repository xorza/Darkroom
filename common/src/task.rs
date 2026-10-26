//! Blocking work from async code: the one route to tokio's blocking pool outside `common`, as
//! `clippy.toml` holds. A task on an async runtime that already runs is not a thread, and stays
//! `tokio::spawn`.

use std::panic;

use tokio::task;

/// The runtime shut down before a blocking task ran: tokio drops a queued task then.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("the runtime shut down before the blocking task ran")]
pub struct Shutdown;

/// `body` on the blocking pool of the runtime this is called inside, which grows a thread for it
/// when every one it has is busy, awaited. A panic in `body` resumes here, as if `body` ran on
/// the awaiting task: the release build aborts on a panic anyway, so turning one into an error
/// would only make a debug build differ from it.
///
/// # Errors
///
/// [`Shutdown`] when the runtime shut down before `body` ran.
pub async fn run_blocking<T: Send + 'static>(
    body: impl FnOnce() -> T + Send + 'static,
) -> Result<T, Shutdown> {
    match task::spawn_blocking(body).await {
        Ok(value) => Ok(value),
        Err(error) => match error.try_into_panic() {
            Ok(payload) => panic::resume_unwind(payload),
            Err(_) => Err(Shutdown),
        },
    }
}

#[cfg(test)]
mod tests {
    use std::panic::{self, AssertUnwindSafe};

    use tokio::runtime::Builder;

    use crate::task::run_blocking;

    /// A body's value comes back, and its panic resumes on the awaiting task with its payload.
    #[test]
    fn a_blocking_body_returns_its_value_or_resumes_its_panic() {
        let runtime = Builder::new_current_thread().build().unwrap();
        assert_eq!(runtime.block_on(run_blocking(|| 7)), Ok(7));
        let payload = panic::catch_unwind(AssertUnwindSafe(|| {
            runtime.block_on(run_blocking(|| -> u8 { panic!("in the body") }))
        }))
        .unwrap_err();
        assert_eq!(payload.downcast_ref::<&str>(), Some(&"in the body"));
    }
}
