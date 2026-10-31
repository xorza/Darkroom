//! [`WindowLink`]: what a driver on another thread reaches the editor's desktop window through.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use palantir::HostHandle;

use crate::driver::error::DriverError;
use crate::gui::MAIN_WINDOW;
use crate::gui::app::App;

/// The window's host handle, and the flags the app raises while a native dialog holds the main
/// thread and while a frame's work goes on past it. Made by [`crate::run_driven`], once the app
/// exists.
#[derive(Debug, Clone)]
pub struct WindowLink {
    pub(crate) handle: HostHandle<App>,
    native_open: Arc<AtomicBool>,
}

impl WindowLink {
    pub(crate) const fn new(handle: HostHandle<App>, native_open: Arc<AtomicBool>) -> Self {
        Self {
            handle,
            native_open,
        }
    }

    /// Whether a native dialog holds the main thread now, as a remote's waits ask.
    pub(crate) fn native_probe(&self) -> impl Fn() -> bool + Send + 'static {
        let native_open = Arc::clone(&self.native_open);
        move || native_open.load(Ordering::Acquire)
    }

    pub(crate) fn blocked(&self) -> Option<DriverError> {
        if self.native_open.load(Ordering::Acquire) {
            Some(DriverError::NativeDialog)
        } else if self.handle.is_hidden(MAIN_WINDOW) {
            Some(DriverError::WindowHidden)
        } else {
            None
        }
    }
}
