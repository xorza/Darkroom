use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use crate::ui_host::UiHost;

/// `UiHost` for the stub TUI. `request_redraw` is a no-op (the loop is
/// blocked on stdin between commands, nothing to repaint). `close_app`
/// flips a shared flag the loop checks after each line; because the
/// read is blocking, a `shutdown()` from a remote script lands once
/// the user presses Enter (we print a notice so they know).
#[derive(Debug)]
pub struct TuiUiHost {
    shutdown: Arc<AtomicBool>,
}

impl TuiUiHost {
    pub fn new(shutdown: Arc<AtomicBool>) -> Self {
        Self { shutdown }
    }
}

impl UiHost for TuiUiHost {
    fn request_redraw(&self) {}

    fn close_app(&self) {
        self.shutdown.store(true, Ordering::SeqCst);
        eprintln!("\n[shutdown requested by script — press Enter to exit]");
    }
}
