//! [`WindowBackend`]: the editor in the desktop window on the main thread, reached from the
//! driver's thread in real time through palantir's [`Remote`]. `darkroom/AGENTS.md`, "The editor
//! harness", "One driver, two backends".

use std::path::PathBuf;

use palantir::{AgentWindow, InspectedWidget, Remote, Ui};

use crate::driver::action::Play;
use crate::driver::error::DriverError;
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

    /// The remote that plays and reads the window, as palantir's driver verbs take it.
    pub(crate) const fn remote(&mut self) -> &mut Remote<App> {
        &mut self.remote
    }

    /// `f` run on the main thread between two of the window's frames, against the last.
    ///
    /// # Panics
    ///
    /// When `f` panics there, with its message: the window stays, and the session ends.
    pub(crate) fn with_app<R: Send + 'static>(
        &mut self,
        f: impl FnOnce(&mut App, &Ui) -> R + Send + 'static,
    ) -> Result<R, DriverError> {
        self.remote
            .query(move |app, ui| f(app, ui))
            .map_err(|error| DriverError::played("the query", error))
    }

    /// Play `action` as palantir's remote plays it, settling after each step it takes, the
    /// previews converting included ([`App::is_busy`](palantir::App::is_busy)).
    pub(crate) fn play(&mut self, action: &Play) -> Result<u32, DriverError> {
        self.remote
            .play(std::slice::from_ref(action))
            .map_err(|failed| DriverError::played(&format!("{action:?}"), failed.error))
    }

    pub(crate) fn widgets(
        &mut self,
        filter: Option<&str>,
    ) -> Result<Vec<InspectedWidget>, DriverError> {
        self.remote
            .widgets(filter)
            .map_err(|error| DriverError::played("the widget list", error))
    }

    /// Refuse to go on once the window closed: the session ends with it.
    pub(crate) fn running(&self) -> Result<(), DriverError> {
        if self.link.handle.is_open(MAIN_WINDOW) {
            Ok(())
        } else {
            Err(DriverError::WindowClosed)
        }
    }

    /// Open `document` in the window, or a new document for none, as File ▸ Open and File ▸ New
    /// do, refusing to discard unsaved edits.
    pub(crate) fn open(&mut self, document: Option<PathBuf>) -> Result<(), DriverError> {
        let opened = self.with_app(move |app, _| match &document {
            Some(document) => app.open_driven(document, UnsavedEdits::Refuse),
            None => app.new_driven(UnsavedEdits::Refuse),
        })?;
        Ok(opened?)
    }

    /// palantir's settle of the window: a frame that saw every sent event, asks no repaint and
    /// finds no preview converting, bounded in real time.
    pub(crate) fn settle(&mut self, step: &str) -> Result<u32, DriverError> {
        self.running()?;
        self.remote
            .settle()
            .map_err(|error| DriverError::played(step, error))
    }
}
