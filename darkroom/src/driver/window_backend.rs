//! [`WindowBackend`]: the editor in the desktop window on the main thread, reached from the
//! driver's thread in real time. `darkroom/AGENTS.md`, "The editor harness", "One driver, two
//! backends".

use std::path::PathBuf;
use std::sync::mpsc::{self, TryRecvError};
use std::thread;
use std::time::{Duration, Instant};

use palantir::{FrameSummary, InputEvent, OwnedInputEvent, Ui};

use crate::driver::BACKGROUND_WAIT;
use crate::driver::error::DriverError;
use crate::driver::shot::{Region, Shot};
use crate::driver::window_link::WindowLink;
use crate::gui::MAIN_WINDOW;
use crate::gui::app::App;
use crate::gui::app::driven_requests::{Served, UpdateRequest};

/// How often a wait reads the window's last frame: a frame at 60 Hz is about 17 ms.
const POLL: Duration = Duration::from_millis(2);

/// The link to the window, and the input sent through it.
#[derive(Debug)]
pub(crate) struct WindowBackend {
    link: WindowLink,
    /// The events sent, which a frame that saw them all counts in its summary.
    sent: u64,
}

impl WindowBackend {
    pub(crate) const fn new(link: WindowLink) -> Self {
        Self { link, sent: 0 }
    }

    /// `f` run on the main thread in the app's next frame, against the frame before it.
    pub(crate) fn read<R: Send + 'static>(
        &self,
        f: impl FnOnce(&mut App, &Ui) -> R + Send + 'static,
    ) -> Result<R, DriverError> {
        let (reply, answer) = mpsc::sync_channel(1);
        let mut f = Some(f);
        let request: UpdateRequest = Box::new(move |app, ui| {
            let f = f.take().expect("a read runs once");
            // The driver gone: no one waits for the answer.
            reply.send(f(app, ui)).ok();
            Served::Done
        });
        self.ask(request)?;
        self.wait(|| match answer.try_recv() {
            Ok(value) => Some(Ok(value)),
            Err(TryRecvError::Empty) => None,
            // The app dropped the request unserved: the window closed.
            Err(TryRecvError::Disconnected) => Some(Err(DriverError::Quit)),
        })
    }

    pub(crate) fn input(&mut self, event: InputEvent<'_>) {
        self.link
            .handle
            .send_input(MAIN_WINDOW, OwnedInputEvent::from(event));
        self.sent += 1;
    }

    /// Wait for a frame that saw every event sent so far.
    pub(crate) fn frame(&self) -> Result<(), DriverError> {
        let sent = self.sent;
        self.wait(|| match self.link.handle.last_frame(MAIN_WINDOW) {
            Some(summary) if summary.sent_inputs >= sent => Some(Ok(())),
            Some(_) => None,
            None => Some(Err(DriverError::Quit)),
        })
    }

    /// Refuse to go on once the window closed: the editor quit, and the process ends with it.
    pub(crate) fn running(&self) -> Result<(), DriverError> {
        match self.link.handle.last_frame(MAIN_WINDOW) {
            Some(_) => Ok(()),
            None => Err(DriverError::Quit),
        }
    }

    /// Open `document` in the window, or a new document for none, as File ▸ Open and File ▸ New
    /// do, refusing to discard unsaved edits.
    pub(crate) fn open(&self, document: Option<PathBuf>) -> Result<(), DriverError> {
        self.read(move |app, _| match &document {
            Some(document) => app.open_driven(document),
            None => app.new_driven(),
        })?
    }

    /// A shot of `region` as a later frame paints it: the region is resolved, the screenshot
    /// asked, and the legend read, each in the frame it belongs to.
    pub(crate) fn shot(
        &self,
        region: &Region,
        max_side: u32,
        path: PathBuf,
    ) -> Result<Shot, DriverError> {
        let (reply, answer) = mpsc::sync_channel(1);
        let region = region.clone();
        let mut path = Some(path);
        let mut asked = false;
        let request: UpdateRequest = Box::new(move |app, ui| {
            if !asked {
                match region.rect(app, ui) {
                    Ok(rect) => {
                        app.driven_mut()
                            .push_record(Box::new(move |ui| ui.request_screenshot(rect)));
                        asked = true;
                        return Served::Again;
                    }
                    Err(error) => {
                        reply.send(Err(error)).ok();
                        return Served::Done;
                    }
                }
            }
            let Some(screenshot) = ui.screenshot_taken() else {
                return Served::Again;
            };
            let path = path.take().expect("a shot lands once");
            reply
                .send(Shot::of(screenshot, max_side, app, ui, path))
                .ok();
            Served::Done
        });
        self.ask(request)?;
        self.wait(|| match answer.try_recv() {
            Ok(shot) => Some(shot),
            Err(TryRecvError::Empty) => None,
            Err(TryRecvError::Disconnected) => Some(Err(DriverError::Quit)),
        })
    }

    /// Wait for a frame that saw every event sent, asks no repaint, and has no preview converting
    /// and no view capture waiting, bounded in real time. The frames it counts are the window's,
    /// which run in real time whatever the driver does.
    pub(crate) fn settle(&self, step: &str) -> Result<u32, DriverError> {
        let first = self.running().map(|()| self.frame_id())?;
        let deadline = Instant::now() + BACKGROUND_WAIT;
        loop {
            if settled(self.link.handle.last_frame(MAIN_WINDOW), self.sent)
                && !self.read(|app, _| app.run_state().previews.in_flight() || app.capturing())?
            {
                let frames = self.frame_id().saturating_sub(first);
                return Ok(u32::try_from(frames).unwrap_or(u32::MAX));
            }
            self.running()?;
            if self.link.native_open() {
                return Err(DriverError::NativeDialog);
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

    /// Queue `request` for the app's next frame, which the queue asks for.
    fn ask(&self, request: UpdateRequest) -> Result<(), DriverError> {
        self.link
            .handle
            .run_on_main(move |app| {
                app.driven_mut().push_update(request);
                true
            })
            .map_err(|_| DriverError::Quit)
    }

    /// Wait until `ready` answers, bounded in real time; a native dialog, which holds the main
    /// thread until the person answers it, is said to be the reason rather than waited out.
    fn wait<R>(
        &self,
        mut ready: impl FnMut() -> Option<Result<R, DriverError>>,
    ) -> Result<R, DriverError> {
        let deadline = Instant::now() + BACKGROUND_WAIT;
        loop {
            if let Some(answer) = ready() {
                return answer;
            }
            if self.link.native_open() {
                return Err(DriverError::NativeDialog);
            }
            if Instant::now() > deadline {
                return Err(DriverError::NoAnswer {
                    seconds: BACKGROUND_WAIT.as_secs_f32(),
                });
            }
            thread::sleep(POLL);
        }
    }
}

/// Whether `last`, the window's last frame, counts as settled once `sent` events were sent: it saw
/// every one of them and asks no repaint. A timer wake, as a caret blink asks, does not hold it.
fn settled(last: Option<FrameSummary>, sent: u64) -> bool {
    last.is_some_and(|summary| summary.sent_inputs >= sent && !summary.repaint_requested)
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use palantir::FrameSummary;

    use crate::driver::window_backend::settled;

    /// Over a run of frames after three sent events: no frame yet, a frame that saw two, one that
    /// saw all three but asks a repaint, and one that asks none, settle only at the last; a timer
    /// wake does not hold it, and a later event unsettles it again.
    #[test]
    fn a_frame_settles_once_it_saw_every_event_and_asks_no_repaint() {
        let frame = |frame_id, sent_inputs, repaint_requested| FrameSummary {
            frame_id,
            sent_inputs,
            repaint_requested,
            repaint_after: None,
        };
        assert!(!settled(None, 0), "no frame yet");
        let run = [
            (frame(7, 2, false), false),
            (frame(8, 3, true), false),
            (frame(9, 3, false), true),
            (
                FrameSummary {
                    repaint_after: Some(Duration::from_millis(530)),
                    ..frame(10, 3, false)
                },
                true,
            ),
        ];
        for (summary, expected) in run {
            assert_eq!(settled(Some(summary), 3), expected, "{summary:?}");
        }
        assert!(
            !settled(Some(frame(10, 3, false)), 4),
            "a fourth event sent"
        );
    }
}
