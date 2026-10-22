//! Blocking work from async code: the one route to tokio's blocking pool outside `common`, as
//! `clippy.toml` holds. A task on an async runtime that already runs is not a thread, and stays
//! `tokio::spawn`.

use tokio::task::{self, JoinHandle};

/// `body` on the blocking pool of the runtime this is called inside, which grows a thread for it
/// when every one it has is busy.
pub fn spawn_blocking<T: Send + 'static>(
    body: impl FnOnce() -> T + Send + 'static,
) -> JoinHandle<T> {
    task::spawn_blocking(body)
}
