//! [`Backend`]: what a driver runs the editor through. `darkroom/AGENTS.md`, "The editor
//! harness", "One driver, two backends".

use std::path::PathBuf;
use std::thread;
use std::time::Duration;

use palantir::{AgentWindow, InspectedWidget, Ui};

use crate::driver::action::Play;
use crate::driver::error::DriverError;
use crate::driver::offscreen::Offscreen;
use crate::driver::shot::{Capture, Region};
use crate::driver::window_backend::WindowBackend;
use crate::gui::app::App;

/// How long a wait on the window's background work sleeps between two looks at the app.
const WINDOW_POLL: Duration = Duration::from_millis(5);

/// The two places the editor runs: offscreen on the driver's thread, or in the desktop window on
/// the main thread.
#[derive(Debug)]
pub(crate) enum Backend {
    /// Boxed: it holds a whole app and its window, kilobytes the enum need not carry inline.
    Offscreen(Box<Offscreen>),
    Window(WindowBackend),
}

impl Backend {
    /// `f`, which reads the app, against the app and the frame it last ran. It asks no frame, so a
    /// hidden window answers and its next settle ends as it would have.
    pub(crate) fn read<R: Send + 'static>(
        &mut self,
        f: impl FnOnce(&App, &Ui) -> R + Send + 'static,
    ) -> Result<R, DriverError> {
        match self {
            Self::Offscreen(offscreen) => offscreen.session().read(f),
            Self::Window(window) => window.remote().read(f),
        }
        .map_err(|error| DriverError::played("the read", error))
    }

    /// `f`, which changes the app, against the app and the frame it last ran.
    pub(crate) fn with_app<R: Send + 'static>(
        &mut self,
        f: impl FnOnce(&mut App, &Ui) -> R + Send + 'static,
    ) -> Result<R, DriverError> {
        match self {
            Self::Offscreen(offscreen) => Ok(offscreen.with_app(f)),
            Self::Window(window) => window.with_app(f),
        }
    }

    /// Play `action` and settle, the editor's own work included; the frames it recorded.
    pub(crate) fn play(&mut self, action: &Play) -> Result<u32, DriverError> {
        match self {
            Self::Offscreen(offscreen) => offscreen.play(action),
            Self::Window(window) => window.play(action),
        }
    }

    /// The visible interactive widgets of the last frame whose text contains `filter`, ignoring
    /// case, in paint order, as palantir lists them.
    pub(crate) fn widgets(
        &mut self,
        filter: Option<&str>,
    ) -> Result<Vec<InspectedWidget>, DriverError> {
        match self {
            Self::Offscreen(offscreen) => Ok(offscreen.widgets(filter)),
            Self::Window(window) => window.widgets(filter),
        }
    }

    pub(crate) fn running(&self) -> Result<(), DriverError> {
        match self {
            Self::Offscreen(offscreen) => offscreen.running(),
            Self::Window(window) => window.running(),
        }
    }

    /// Whether a wake waits for the next frame: work the worker posted since the last settle.
    /// The window runs that frame at once, so none waits there.
    pub(crate) fn wake_pending(&self) -> bool {
        match self {
            Self::Offscreen(offscreen) => offscreen.wake_pending(),
            Self::Window(_) => false,
        }
    }

    /// Sleep until background work may have posted a result, or `timeout` passes: offscreen,
    /// until the app's wake; the window's frames run themselves, so a moment of its real time.
    pub(crate) fn wait_wake(&self, timeout: Duration) {
        match self {
            Self::Offscreen(offscreen) => offscreen.wait_wake(timeout),
            Self::Window(_) => thread::sleep(WINDOW_POLL.min(timeout)),
        }
    }

    /// Lose the device the editor paints with; the next frame paints on a new one. In the window
    /// every window of the app loses it.
    pub(crate) fn lose_device(&mut self) -> Result<(), DriverError> {
        match self {
            Self::Offscreen(offscreen) => offscreen.lose_device(),
            Self::Window(window) => window.lose_device(),
        }
    }

    pub(crate) fn settle(&mut self, step: &str) -> Result<u32, DriverError> {
        match self {
            Self::Offscreen(offscreen) => offscreen.settle(step),
            Self::Window(window) => window.settle(step),
        }
    }

    /// Open `document`, or a new document for none, in place of the open one.
    pub(crate) fn open(&mut self, document: Option<PathBuf>) -> Result<(), DriverError> {
        match self {
            Self::Offscreen(offscreen) => offscreen.open(document),
            Self::Window(window) => window.open(document),
        }
    }

    /// A capture of `region`, as a later frame paints it.
    pub(crate) fn shot(&mut self, region: &Region, max_side: u32) -> Result<Capture, DriverError> {
        match self {
            Self::Offscreen(offscreen) => {
                Capture::take(&mut offscreen.painting_session()?, region, max_side)
            }
            Self::Window(window) => Capture::take(window.remote(), region, max_side),
        }
    }
}
