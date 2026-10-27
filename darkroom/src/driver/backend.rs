//! [`Backend`]: what a driver runs the editor through. `darkroom/AGENTS.md`, "The editor
//! harness", "One driver, two backends".

use std::path::PathBuf;
use std::thread;
use std::time::Duration;

use palantir::{InputEvent, Ui};

use crate::driver::error::DriverError;
use crate::driver::offscreen::Offscreen;
use crate::driver::shot::{Region, Shot};
use crate::driver::window_backend::WindowBackend;
use crate::gui::app::App;

/// The two places the editor runs: offscreen on the driver's thread, or in the desktop window on
/// the main thread.
#[derive(Debug)]
pub(crate) enum Backend {
    /// Boxed: it holds a whole app and its window, kilobytes the enum need not carry inline.
    Offscreen(Box<Offscreen>),
    Window(WindowBackend),
}

impl Backend {
    /// `f` against the app and the frame it last ran.
    pub(crate) fn read<R: Send + 'static>(
        &mut self,
        f: impl FnOnce(&mut App, &Ui) -> R + Send + 'static,
    ) -> Result<R, DriverError> {
        match self {
            Self::Offscreen(offscreen) => Ok(offscreen.read(f)),
            Self::Window(window) => window.read(f),
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

    pub(crate) fn settle(&mut self, step: &str) -> Result<u32, DriverError> {
        match self {
            Self::Offscreen(offscreen) => offscreen.settle(step),
            Self::Window(window) => window.settle(step),
        }
    }

    /// Open `document`, or a new document for none, in place of the open one.
    pub(crate) fn open(&mut self, document: Option<PathBuf>) -> Result<(), DriverError> {
        match self {
            Self::Offscreen(offscreen) => offscreen.relaunch(document),
            Self::Window(window) => window.open(document),
        }
    }

    pub(crate) fn shot(
        &mut self,
        region: &Region,
        max_side: u32,
        path: PathBuf,
    ) -> Result<Shot, DriverError> {
        match self {
            Self::Offscreen(offscreen) => offscreen.shot(region, max_side, path),
            Self::Window(window) => window.shot(region, max_side, path),
        }
    }
}
