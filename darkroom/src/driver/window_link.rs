//! [`WindowLink`]: what a driver on another thread reaches the editor's desktop window through.

use palantir::HostHandle;

use crate::driver::error::DriverError;
use crate::gui::MAIN_WINDOW;
use crate::gui::app::App;
use crate::gui::dialogs::NativeDialogFlag;

/// The window's host handle, and the flag the app raises while a native dialog holds the main
/// thread. Made by [`crate::run_driven`], once the app exists.
#[derive(Debug, Clone)]
pub struct WindowLink {
    pub(crate) handle: HostHandle<App>,
    native_dialog: NativeDialogFlag,
}

impl WindowLink {
    pub(crate) const fn new(handle: HostHandle<App>, native_dialog: NativeDialogFlag) -> Self {
        Self {
            handle,
            native_dialog,
        }
    }

    /// Whether a native dialog holds the main thread now, as a remote's waits ask.
    pub(crate) fn native_probe(&self) -> impl Fn() -> bool + Send + 'static {
        let native_dialog = self.native_dialog.clone();
        move || native_dialog.is_raised()
    }

    /// Why the window runs no frame for now, or `None` while it runs: a native dialog holds every
    /// frame until the person answers it, and a minimized or occluded window until it shows.
    pub(crate) fn blocked(&self) -> Option<DriverError> {
        if self.native_dialog.is_raised() {
            Some(DriverError::NativeDialog)
        } else if self.handle.is_hidden(MAIN_WINDOW) {
            Some(DriverError::WindowHidden)
        } else {
            None
        }
    }
}
