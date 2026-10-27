//! [`Backend`]: what a driver runs the editor through. `darkroom/AGENTS.md`, "The editor
//! harness", "One driver, two backends".

use std::path::PathBuf;
use std::thread;
use std::time::{Duration, Instant};

use palantir::{InputEvent, Ui};

use crate::driver::error::DriverError;
use crate::driver::offscreen::Offscreen;
use crate::driver::shot::{Capture, Region};
use crate::driver::window_backend::WindowBackend;
use crate::gui::app::App;

/// How long a wait on the window's background work sleeps between two looks at the app, which
/// each run a frame of it.
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
    /// `f`, which reads or changes the app, against the app and the frame it last ran.
    pub(crate) fn with_app<R: Send + 'static>(
        &mut self,
        f: impl FnOnce(&mut App, &Ui) -> R + Send + 'static,
    ) -> Result<R, DriverError> {
        match self {
            Self::Offscreen(offscreen) => Ok(offscreen.with_app(f)),
            Self::Window(window) => window.with_app(f),
        }
    }

    pub(crate) fn input(&mut self, event: InputEvent<'_>) {
        match self {
            Self::Offscreen(offscreen) => offscreen.input(event),
            Self::Window(window) => window.input(event),
        }
    }

    /// One frame that sees every event sent so far.
    pub(crate) fn frame(&mut self) -> Result<(), DriverError> {
        match self {
            Self::Offscreen(offscreen) => {
                offscreen.frame();
                Ok(())
            }
            Self::Window(window) => window.frame(),
        }
    }

    /// Let `dt` pass with no frame, as a pause between two clicks does.
    pub(crate) fn advance(&mut self, dt: Duration) {
        match self {
            Self::Offscreen(offscreen) => offscreen.advance(dt),
            // The window runs on real time: pause as a person's hand does between two clicks.
            Self::Window(_) => thread::sleep(dt),
        }
    }

    pub(crate) fn running(&self) -> Result<(), DriverError> {
        match self {
            Self::Offscreen(offscreen) => offscreen.running(),
            Self::Window(window) => window.running(),
        }
    }

    /// Whether background work announced a result no frame has taken yet. The window's host runs
    /// that frame itself.
    pub(crate) fn wake_pending(&self) -> bool {
        match self {
            Self::Offscreen(offscreen) => offscreen.wake_pending(),
            Self::Window(_) => false,
        }
    }

    /// Sleep until background work may have posted a result, or `deadline` passes: offscreen,
    /// until the app's wake; the window's frames run themselves, so a moment of its real time.
    pub(crate) fn wait_wake(&self, deadline: Instant) {
        match self {
            Self::Offscreen(offscreen) => offscreen.wait_wake(deadline),
            Self::Window(_) => {
                thread::sleep(WINDOW_POLL.min(deadline.saturating_duration_since(Instant::now())));
            }
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

    pub(crate) fn shot(&mut self, region: &Region, max_side: u32) -> Result<Capture, DriverError> {
        match self {
            Self::Offscreen(offscreen) => offscreen.shot(region, max_side),
            Self::Window(window) => window.shot(region, max_side),
        }
    }
}
