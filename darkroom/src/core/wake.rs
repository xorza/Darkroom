//! The host-wake callback shared by the evaluation worker and the script
//! executor, plus its non-GUI constructor.

use std::sync::Arc;

use tokio::sync::Notify;

/// Opaque "wake the host loop" callback, fired from a background
/// worker/script thread after it posts a result so the main loop
/// re-drains. The GUI wires it to [`aperture::HostHandle::request_repaint`];
/// the headless / TUI drivers wire it to a `tokio::sync::Notify` (see
/// [`from_notify`]). Keeps the worker + script modules free of any
/// specific frontend type.
pub(crate) type Wake = Arc<dyn Fn() + Send + Sync>;

/// Build a [`Wake`] that pings `notify` — the headless/TUI wiring, where
/// the driver loop `await`s `notify.notified()` to re-drain.
pub(crate) fn from_notify(notify: Arc<Notify>) -> Wake {
    Arc::new(move || notify.notify_one())
}
