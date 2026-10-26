//! [`AppHost`]: what runs the [`App`](crate::gui::app::App)'s frames, as the app reaches it.

use std::sync::Arc;

use palantir::HostHandle;

use crate::core::wake::Wake;
use crate::gui::MAIN_WINDOW;
use crate::gui::app::App;
#[cfg(feature = "internals")]
use crate::gui::app::offscreen_signals::OffscreenSignals;

/// What runs the app's frames: the desktop window's host, or the harness. The app asks it for
/// two things only, a wake when background work lands and a quit.
#[derive(Debug)]
pub(crate) enum AppHost {
    /// The winit host of the desktop window.
    Window(HostHandle<App>),
    /// The harness, which runs every frame itself and reads what the app asked of it.
    #[cfg(feature = "internals")]
    Offscreen(Arc<OffscreenSignals>),
}

impl AppHost {
    /// The callback the worker and the preview converter fire when they post a result, so the
    /// host runs a frame that drains it.
    pub(crate) fn wake(&self) -> Wake {
        match self {
            Self::Window(handle) => {
                let handle = handle.clone();
                Arc::new(move || handle.request_repaint(MAIN_WINDOW))
            }
            #[cfg(feature = "internals")]
            Self::Offscreen(signals) => {
                let signals = Arc::clone(signals);
                Arc::new(move || signals.note_wake())
            }
        }
    }

    /// End the session once this frame is over.
    pub(crate) fn quit(&self) {
        match self {
            Self::Window(handle) => handle.quit(),
            #[cfg(feature = "internals")]
            Self::Offscreen(signals) => signals.note_quit(),
        }
    }
}
