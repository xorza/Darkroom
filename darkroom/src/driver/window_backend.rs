//! [`WindowBackend`]: the editor in the desktop window on the main thread, reached from the
//! driver's thread in real time. `darkroom/AGENTS.md`, "The editor harness", "One driver, two
//! backends".

use std::path::PathBuf;
use std::sync::mpsc::{self, Receiver, TryRecvError};
use std::sync::{Arc, Mutex, PoisonError};
use std::thread;
use std::time::{Duration, Instant};

use palantir::{FrameSummary, InputEvent, OwnedInputEvent, Ui};

use crate::driver::BACKGROUND_WAIT;
use crate::driver::error::DriverError;
use crate::driver::shot::{Capture, Region};
use crate::driver::window_link::WindowLink;
use crate::gui::MAIN_WINDOW;
use crate::gui::app::driven_requests::{RequestHandle, Served, UpdateRequest, Withdrawn};
use crate::gui::app::screenshot_ask::{Landed, ScreenshotAsk};
use crate::gui::app::{App, UnsavedEdits};

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
    ///
    /// # Panics
    ///
    /// When `f` panics there, with its message: the window stays, and the session ends.
    pub(crate) fn with_app<R: Send + 'static>(
        &self,
        f: impl FnOnce(&mut App, &Ui) -> R + Send + 'static,
    ) -> Result<R, DriverError> {
        let (request, answer) = with_app_request(f);
        self.answer(request, &answer)
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
        let deadline = Instant::now() + BACKGROUND_WAIT;
        loop {
            match self.link.handle.last_frame(MAIN_WINDOW) {
                Some(summary) if summary.sent_inputs >= sent => return Ok(()),
                Some(_) => {}
                None => return Err(DriverError::WindowClosed),
            }
            if let Some(blocked) = self.link.blocked() {
                return Err(blocked);
            }
            if Instant::now() > deadline {
                return Err(DriverError::NoFrame {
                    seconds: BACKGROUND_WAIT.as_secs_f32(),
                });
            }
            thread::sleep(POLL);
        }
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

    /// A shot of `region` as a later frame paints it: the region is resolved, the screenshot
    /// asked, and the legend read, each in the frame it belongs to.
    pub(crate) fn shot(&self, region: &Region, max_side: u32) -> Result<Capture, DriverError> {
        let (request, answer) = shot_request(region.clone(), max_side);
        self.answer(request, &answer)?
    }

    /// Wait for a frame that saw every event sent, asks no repaint, and has no preview converting
    /// and no view capture waiting, bounded in real time. The frames it counts are the window's,
    /// which run in real time whatever the driver does.
    pub(crate) fn settle(&self, step: &str) -> Result<u32, DriverError> {
        let first = self.running().map(|()| self.frame_id())?;
        let deadline = Instant::now() + BACKGROUND_WAIT;
        loop {
            // Read from the frame's flag, not a request, which would run a frame of its own.
            if settled(self.link.handle.last_frame(MAIN_WINDOW), self.sent)
                && !self.link.pending_work()
            {
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

    /// Queue `request` for the app's next frame, which the queue asks for, and wait for what it
    /// answers on `answer`.
    fn answer<T>(&self, request: UpdateRequest, answer: &Receiver<T>) -> Result<T, DriverError> {
        let handle = RequestHandle::default();
        let queued = handle.clone();
        self.link
            .handle
            .run_on_main(move |app| {
                app.driven_mut().push_update(request, queued);
                true
            })
            .map_err(|_| DriverError::WindowClosed)?;
        wait_answer(&handle, answer, BACKGROUND_WAIT, || self.link.blocked())
    }
}

/// A request that runs `f` once, and the receiver of what it returns.
fn with_app_request<R: Send + 'static>(
    f: impl FnOnce(&mut App, &Ui) -> R + Send + 'static,
) -> (UpdateRequest, Receiver<R>) {
    let (reply, answer) = mpsc::sync_channel(1);
    let mut f = Some(f);
    let request: UpdateRequest = Box::new(move |app, ui| {
        let f = f.take().expect("a request runs once");
        // The driver gone: no one waits for the answer.
        reply.send(f(app, ui)).ok();
        Served::Done
    });
    (request, answer)
}

/// A request for a shot of `region`, and the receiver of the shot: the first frame resolves the
/// region and asks the screenshot in its record pass, and the frame its image lands in reads the
/// legend.
fn shot_request(
    region: Region,
    max_side: u32,
) -> (UpdateRequest, Receiver<Result<Capture, DriverError>>) {
    let (reply, answer) = mpsc::sync_channel(1);
    let mut pushed = false;
    // Written by the record pass that asks, read by the update passes after it.
    let asked: Arc<Mutex<ScreenshotAsk>> = Arc::default();
    let request: UpdateRequest = Box::new(move |app, ui| {
        if !pushed {
            match region.rect(app, ui) {
                Ok(rect) => {
                    let ask = Arc::clone(&asked);
                    app.driven_mut().push_record(Box::new(move |ui| {
                        ask.lock()
                            .unwrap_or_else(PoisonError::into_inner)
                            .ask(ui, rect);
                    }));
                    pushed = true;
                    return Served::Again;
                }
                Err(error) => {
                    reply.send(Err(error)).ok();
                    return Served::Done;
                }
            }
        }
        let landed = asked
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .landed(ui);
        let screenshot = match landed {
            None => return Served::Again,
            Some(Landed::Shot(screenshot)) => screenshot,
            Some(Landed::Empty) => {
                reply
                    .send(Err(DriverError::EmptyRegion {
                        region: format!("{region:?}"),
                    }))
                    .ok();
                return Served::Done;
            }
        };
        // The copy and the legend alone: the fit and the encode, tens of milliseconds, run on
        // the driver's thread rather than stall the person's window.
        reply
            .send(Ok(Capture::of(screenshot, max_side, app, ui)))
            .ok();
        Served::Done
    });
    (request, answer)
}

/// Wait until the request `handle` holds answers on `answer`, for at most `wait`. A window that
/// `blocked` says holds its frames — a native dialog the person must answer, or a hidden window —
/// is said to be the reason rather than waited out; either way the request is withdrawn, so a
/// late frame never runs it.
///
/// # Panics
///
/// When the request panicked, with its message.
fn wait_answer<T>(
    handle: &RequestHandle,
    answer: &Receiver<T>,
    wait: Duration,
    blocked: impl Fn() -> Option<DriverError>,
) -> Result<T, DriverError> {
    let deadline = Instant::now() + wait;
    loop {
        match answer.try_recv() {
            Ok(value) => return Ok(value),
            Err(TryRecvError::Empty) => {}
            // The request dropped unanswered: it panicked, or the window closed and dropped it.
            Err(TryRecvError::Disconnected) => match handle.panic() {
                Some(message) => panic!("a request on the window's thread panicked: {message}"),
                None => return Err(DriverError::WindowClosed),
            },
        }
        let error = if let Some(blocked) = blocked() {
            blocked
        } else if Instant::now() > deadline {
            DriverError::NoAnswer {
                seconds: wait.as_secs_f32(),
            }
        } else {
            thread::sleep(POLL);
            continue;
        };
        return Err(match handle.withdraw() {
            Withdrawn::Unbegun => error,
            Withdrawn::Begun => DriverError::Unfinished {
                cause: Box::new(error),
            },
        });
    }
}

/// Whether `last`, the window's last frame, counts as settled once `sent` events were sent: it saw
/// every one of them and asks no repaint. A timer wake, as a caret blink asks, does not hold it.
fn settled(last: Option<FrameSummary>, sent: u64) -> bool {
    last.is_some_and(|summary| summary.sent_inputs >= sent && !summary.repaint_requested)
}

#[cfg(test)]
mod tests {
    use std::panic::{self, AssertUnwindSafe};
    use std::sync::mpsc;
    use std::time::Duration;

    use glam::UVec2;
    use palantir::FrameSummary;
    use palantir::internals::ui_harness::UiHarness;

    use crate::driver::error::DriverError;
    use crate::driver::shot::Region;
    use crate::driver::window_backend::{settled, shot_request, wait_answer, with_app_request};
    use crate::gui::app::App;
    use crate::gui::app::driven_requests::{RequestHandle, Served, Withdrawn};

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

    /// An answer sent is the wait's; a request dropped unanswered is a closed window, or its
    /// panic, raised again with its message; a native dialog, a hidden window and the deadline
    /// give up at once
    /// and withdraw the request, saying so when a frame had begun it.
    #[test]
    fn a_wait_takes_the_answer_or_says_why_there_is_none() {
        let no_dialog = || None;
        let (reply, answer) = mpsc::sync_channel(1);
        reply.send(7).unwrap();
        let handle = RequestHandle::default();
        assert_eq!(
            wait_answer(&handle, &answer, Duration::ZERO, no_dialog).unwrap(),
            7
        );
        drop(reply);
        assert!(matches!(
            wait_answer(&handle, &answer, Duration::ZERO, no_dialog),
            Err(DriverError::WindowClosed)
        ));

        let (_reply, answer) = mpsc::sync_channel::<u32>(1);
        let dialog = RequestHandle::default();
        assert!(matches!(
            wait_answer(&dialog, &answer, Duration::from_secs(30), || {
                Some(DriverError::NativeDialog)
            }),
            Err(DriverError::NativeDialog)
        ));
        let hidden = RequestHandle::default();
        assert!(matches!(
            wait_answer(&hidden, &answer, Duration::from_secs(30), || {
                Some(DriverError::WindowHidden)
            }),
            Err(DriverError::WindowHidden)
        ));
        assert_eq!(dialog.withdraw(), Withdrawn::Unbegun, "withdrawn unbegun");
        let late = RequestHandle::default();
        assert!(matches!(
            wait_answer(&late, &answer, Duration::ZERO, no_dialog),
            Err(DriverError::NoAnswer { seconds: 0.0 })
        ));

        let mut ui = UiHarness::with_text(UVec2::new(800, 600));
        let mut app = App::offscreen(ui.ui());
        let begun = RequestHandle::default();
        app.driven_mut()
            .push_update(Box::new(|_, _| Served::Again), begun.clone());
        ui.frame_app(&mut app);
        match wait_answer(&begun, &answer, Duration::ZERO, no_dialog) {
            Err(DriverError::Unfinished { cause }) => {
                assert!(matches!(*cause, DriverError::NoAnswer { .. }), "{cause}");
            }
            other => panic!("{other:?}"),
        }

        let (request, answer) = with_app_request(|_, _| -> u32 { panic!("boom") });
        let panicked = RequestHandle::default();
        app.driven_mut().push_update(request, panicked.clone());
        ui.frame_app(&mut app);
        let raised = panic::catch_unwind(AssertUnwindSafe(|| {
            wait_answer(&panicked, &answer, Duration::ZERO, no_dialog)
        }))
        .unwrap_err();
        assert_eq!(
            raised.downcast_ref::<String>().map(String::as_str),
            Some("a request on the window's thread panicked: boom")
        );
    }

    /// A read runs once against the app and the frame. A shot asks its screenshot in the first
    /// frame's record pass and waits for it to land; a region that resolves to nothing answers
    /// its error at once.
    #[test]
    fn the_requests_answer_in_the_frames_they_belong_to() {
        let mut ui = UiHarness::with_text(UVec2::new(800, 600));
        let mut app = App::offscreen(ui.ui());
        ui.frame_app(&mut app);

        let (mut read, answer) =
            with_app_request(|app, ui| (app.open_document().path.clone(), ui.frame_id()));
        let frame = ui.ui().frame_id();
        assert_eq!(read(&mut app, ui.ui()), Served::Done);
        assert_eq!(answer.try_recv().unwrap(), (None, frame));

        let (mut shot, answer) = shot_request(Region::Window, 100);
        assert_eq!(shot(&mut app, ui.ui()), Served::Again);
        assert!(
            app.driven_mut().pending(),
            "the screenshot is asked in the record pass"
        );
        assert_eq!(
            shot(&mut app, ui.ui()),
            Served::Again,
            "no screenshot landed"
        );
        assert!(answer.try_recv().is_err());

        let (mut missing, answer) = shot_request(Region::Node("missing".to_string()), 100);
        assert_eq!(missing(&mut app, ui.ui()), Served::Done);
        assert!(matches!(
            answer.try_recv().unwrap(),
            Err(DriverError::NotFound { .. })
        ));
    }
}
