//! [`WindowBackend`]: the editor in the desktop window on the main thread, reached from the
//! driver's thread in real time through palantir's [`Remote`]. `darkroom/AGENTS.md`, "The editor
//! harness", "One driver, two backends".

use std::path::PathBuf;

use palantir::{InspectedWidget, PlayError, Remote, Ui};

use crate::driver::action::Play;
use crate::driver::error::DriverError;
use crate::driver::shot::{Capture, Region};
use crate::driver::window_link::WindowLink;
use crate::gui::MAIN_WINDOW;
use crate::gui::app::{App, UnsavedEdits};

/// The link to the window, and the remote that plays and reads it.
#[derive(Debug)]
pub(crate) struct WindowBackend {
    link: WindowLink,
    remote: Remote<App>,
}

impl WindowBackend {
    pub(crate) fn new(link: WindowLink) -> Self {
        let remote = link
            .handle
            .remote(MAIN_WINDOW)
            .with_blocked(link.native_probe());
        Self { link, remote }
    }

    /// `f` run on the main thread between two of the window's frames, against the last.
    ///
    /// # Panics
    ///
    /// When `f` panics there, with its message: the window stays, and the session ends.
    pub(crate) fn with_app<R: Send + 'static>(
        &self,
        f: impl FnOnce(&mut App, &Ui) -> R + Send + 'static,
    ) -> Result<R, DriverError> {
        self.remote
            .query(move |app, ui| f(app, ui))
            .map_err(|error| self.error(error))
    }

    /// Play `action` as palantir's remote plays it, settling after each step it takes, the
    /// previews converting included ([`App::is_busy`](palantir::App::is_busy)).
    pub(crate) fn play(&self, action: &Play) -> Result<u32, DriverError> {
        self.remote
            .play(std::slice::from_ref(action))
            .map_err(|failed| self.error(failed.error))
    }

    pub(crate) fn widgets(
        &self,
        filter: Option<&str>,
    ) -> Result<Vec<InspectedWidget>, DriverError> {
        self.remote
            .widgets(filter)
            .map_err(|error| self.error(error))
    }

    /// Refuse to go on once the window closed: the session ends with it.
    pub(crate) fn running(&self) -> Result<(), DriverError> {
        match self.link.handle.last_frame(MAIN_WINDOW) {
            Some(_) => Ok(()),
            None => Err(DriverError::WindowClosed),
        }
    }

    /// Open `document` in the window, or a new document for none, as File ▸ Open and File ▸ New
    /// do, refusing to discard unsaved edits.
    pub(crate) fn open(&self, document: Option<PathBuf>) -> Result<(), DriverError> {
        let opened = self.with_app(move |app, _| match &document {
            Some(document) => app.open_driven(document, UnsavedEdits::Refuse),
            None => app.new_driven(UnsavedEdits::Refuse),
        })?;
        Ok(opened?)
    }

    /// A shot of `region` as a later frame paints it: the region resolved against the last
    /// frame, the image taken by the frame it lands in, the fit made here, off the person's
    /// thread, and the legend read against the frame after.
    pub(crate) fn shot(&self, region: &Region, max_side: u32) -> Result<Capture, DriverError> {
        let asked = region.clone();
        let rect = self.with_app(move |app, ui| asked.rect(app, ui))??;
        let screenshot = self.remote.shot(rect).map_err(|error| match error {
            PlayError::EmptyRegion => DriverError::EmptyRegion {
                region: format!("{region:?}"),
            },
            error => self.error(error),
        })?;
        let picture = screenshot.image().fitted(max_side);
        self.with_app(move |app, ui| Capture::of(&screenshot, picture, app, ui))
    }

    /// palantir's settle of the window: a frame that saw every sent event, asks no repaint and
    /// finds no preview converting, bounded in real time.
    pub(crate) fn settle(&self, step: &str) -> Result<u32, DriverError> {
        self.running()?;
        self.remote.settle().map_err(|error| {
            self.link
                .blocked()
                .unwrap_or_else(|| DriverError::from_settle(step, error))
        })
    }

    /// What `error` of the remote means: a native dialog or a hidden window holding the frames
    /// is said to be the reason, as the remote gave up on it at once.
    fn error(&self, error: PlayError) -> DriverError {
        self.link.blocked().unwrap_or_else(|| error.into())
    }
}
