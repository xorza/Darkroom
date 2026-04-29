use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use tokio::sync::Notify;

use crate::ui_host::UiHost;

/// `UiHost` for the TUI. Both hooks fire the shared `wake` so the
/// `tokio::select!` in `MainTui::run` drops out of its current await
/// (whether that's stdin or `wake.notified()`) and re-enters the
/// drain → render → handle_output loop. `close_app` additionally
/// flips the shutdown flag, so the loop sees it and breaks.
#[derive(Debug)]
pub struct TuiUiHost {
    wake: Arc<Notify>,
    shutdown: Arc<AtomicBool>,
}

impl TuiUiHost {
    pub fn new(wake: Arc<Notify>, shutdown: Arc<AtomicBool>) -> Self {
        Self { wake, shutdown }
    }
}

impl UiHost for TuiUiHost {
    fn request_redraw(&self) {
        self.wake.notify_one();
    }

    fn close_app(&self) {
        self.shutdown.store(true, Ordering::SeqCst);
        self.wake.notify_one();
    }
}
