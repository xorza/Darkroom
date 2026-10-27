//! [`WindowLink`]: what a driver on another thread reaches the editor's desktop window through.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use palantir::HostHandle;

use crate::gui::app::App;

/// The window's host handle, and the flag the app raises while a native dialog holds the main
/// thread. Made by [`crate::run_driven`], once the app exists.
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

    /// Whether a native dialog is open, which blocks every frame until the person answers it.
    pub(crate) fn native_open(&self) -> bool {
        self.native_open.load(Ordering::Acquire)
    }
}
