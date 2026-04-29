use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use tokio::sync::Notify;

use crate::ui_host::UiHost;

/// `UiHost` for headless mode. No window or terminal — we surface only
/// the two capabilities the core needs:
///
/// - `request_redraw` pings a `Notify` so the headless event loop wakes
///   and runs `Session::frame` (which drains worker + script inbounds).
/// - `close_app` flips the shutdown flag and pings the same `Notify`,
///   so the loop sees the bit on its next iteration and exits cleanly.
#[derive(Debug)]
pub struct HeadlessUiHost {
    wake: Arc<Notify>,
    shutdown: Arc<AtomicBool>,
}

impl HeadlessUiHost {
    pub fn new(wake: Arc<Notify>, shutdown: Arc<AtomicBool>) -> Self {
        Self { wake, shutdown }
    }
}

impl UiHost for HeadlessUiHost {
    fn request_redraw(&self) {
        self.wake.notify_one();
    }

    fn close_app(&self) {
        self.shutdown.store(true, Ordering::SeqCst);
        self.wake.notify_one();
    }
}
