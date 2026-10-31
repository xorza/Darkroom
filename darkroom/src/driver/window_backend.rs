//! [`WindowBackend`]: the editor in the desktop window on the main thread, reached from the
//! driver's thread in real time through palantir's [`Remote`]. `darkroom/AGENTS.md`, "The editor
//! harness", "One driver, two backends".

use std::any::Any;
use std::panic::{self, AssertUnwindSafe};
use std::path::PathBuf;
use std::thread;
use std::time::{Duration, Instant};

use palantir::{FrameSummary, InspectedWidget, PlayError, Remote, Ui};

use crate::driver::BACKGROUND_WAIT;
use crate::driver::action::Play;
use crate::driver::error::DriverError;
use crate::driver::shot::{Capture, Region};
use crate::driver::window_link::WindowLink;
use crate::gui::MAIN_WINDOW;
use crate::gui::app::{App, UnsavedEdits};

/// How often a wait reads the window's last frame: a frame at 60 Hz is about 17 ms.
const POLL: Duration = Duration::from_millis(2);

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
        let answer = self
            .remote
            .query(move |app, ui| panic::catch_unwind(AssertUnwindSafe(|| f(app, ui))))
            .map_err(|error| self.error(error))?;
        Ok(answer.unwrap_or_else(|payload| {
            panic!(
                "a request on the window's thread panicked: {}",
                message(&*payload)
            )
        }))
    }

    /// Play `action` as palantir's remote plays it, then wait for the editor's own work.
    pub(crate) fn play(&self, action: &Play, step: &str) -> Result<u32, DriverError> {
        let frames = self
            .remote
            .play(std::slice::from_ref(action))
            .map_err(|failed| self.error(failed.error))?;
        Ok(frames + self.settle(step)?)
    }

    pub(crate) fn widgets(
        &self,
        filter: Option<&str>,
    ) -> Result<Vec<InspectedWidget>, DriverError> {
        self.remote
            .widgets(filter.map(str::to_owned))
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

    /// Wait for a frame that asks no repaint and has no preview converting and no view capture
    /// waiting, bounded in real time. The frames it counts are the window's, which run in real
    /// time whatever the driver does.
    pub(crate) fn settle(&self, step: &str) -> Result<u32, DriverError> {
        let first = self.running().map(|()| self.frame_id())?;
        let deadline = Instant::now() + BACKGROUND_WAIT;
        loop {
            // Read from the frame's flag, not a query, which would run a frame of its own.
            if settled(self.link.handle.last_frame(MAIN_WINDOW)) && !self.link.pending_work() {
                let frames = self.frame_id().saturating_sub(first);
                return Ok(u32::try_from(frames).unwrap_or(u32::MAX));
            }
            self.running()?;
            if let Some(blocked) = self.link.blocked() {
                return Err(blocked);
            }
            if Instant::now() > deadline {
                return Err(DriverError::UnsettledInTime {
                    step: step.to_string(),
                    seconds: BACKGROUND_WAIT.as_secs_f32(),
                });
            }
            thread::sleep(POLL);
        }
    }

    fn frame_id(&self) -> u64 {
        self.link
            .handle
            .last_frame(MAIN_WINDOW)
            .map_or(0, |summary| summary.frame_id)
    }

    /// What `error` of the remote means: a native dialog or a hidden window holding the frames
    /// is said to be the reason, as the remote gave up on it at once.
    fn error(&self, error: PlayError) -> DriverError {
        self.link.blocked().unwrap_or_else(|| error.into())
    }
}

/// The message a panic carried, as `panic!` and `assert!` give it.
fn message(payload: &(dyn Any + Send)) -> &str {
    payload
        .downcast_ref::<String>()
        .map(String::as_str)
        .or_else(|| payload.downcast_ref::<&str>().copied())
        .unwrap_or("a panic with no message")
}

/// Whether `last`, the window's last frame, counts as settled: it asks no repaint. A timer wake,
/// as a caret blink asks, does not hold it.
fn settled(last: Option<FrameSummary>) -> bool {
    last.is_some_and(|summary| !summary.repaint_requested)
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use palantir::FrameSummary;

    use crate::driver::window_backend::{message, settled};

    /// No frame yet is unsettled; a frame that asks a repaint holds it, one that asks none
    /// settles, and a timer wake does not hold it.
    #[test]
    fn a_frame_settles_once_it_asks_no_repaint() {
        let frame = |frame_id, repaint_requested| FrameSummary {
            frame_id,
            sent_inputs: 0,
            repaint_requested,
            repaint_after: None,
        };
        assert!(!settled(None), "no frame yet");
        assert!(!settled(Some(frame(8, true))));
        assert!(settled(Some(frame(9, false))));
        assert!(settled(Some(FrameSummary {
            repaint_after: Some(Duration::from_millis(530)),
            ..frame(10, false)
        })));
    }

    /// A panic's message reads whether it was formatted or a literal.
    #[test]
    fn a_panic_message_reads_either_form() {
        let formatted: Box<dyn std::any::Any + Send> = Box::new(format!("boom {}", 7));
        let literal: Box<dyn std::any::Any + Send> = Box::new("bang");
        let other: Box<dyn std::any::Any + Send> = Box::new(7_u8);
        assert_eq!(message(&*formatted), "boom 7");
        assert_eq!(message(&*literal), "bang");
        assert_eq!(message(&*other), "a panic with no message");
    }
}
