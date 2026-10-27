//! [`DrivenRequests`]: the work a driver on another thread hands the app, done in the app's next
//! frames, where palantir lends the `Ui`. `darkroom/AGENTS.md`, "The editor harness", "One
//! driver, two backends".

use std::any::Any;
use std::fmt;
use std::mem;
use std::panic::{self, AssertUnwindSafe};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use palantir::Ui;

use crate::gui::app::App;

/// Read or change the app against the last frame, in `update`, before the frame changes anything;
/// asked again each frame until it answers [`Served::Done`].
pub(crate) type UpdateRequest = Box<dyn FnMut(&mut App, &Ui) -> Served + Send>;
/// Ask the frame for what only a record pass asks, as a screenshot: run in its first pass.
pub(crate) type RecordRequest = Box<dyn FnOnce(&mut Ui) + Send>;

/// Whether an update request is done, or waits for a later frame, as one waiting for a screenshot
/// to land does.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Served {
    Done,
    Again,
}

/// The driver's hold on an update request it queued: it withdraws the request when it stops
/// waiting, so a late frame never runs it, and it reads the message of a panic in it. A panic
/// stays in the request: the window stays for the person at it. `darkroom/AGENTS.md`, "The
/// editor harness", "The process is the session".
#[derive(Clone, Debug, Default)]
pub(crate) struct RequestHandle(Arc<Mutex<Shared>>);

#[derive(Debug, Default)]
struct Shared {
    stage: Stage,
    panic: Option<String>,
}

/// How far a queued request got.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum Stage {
    #[default]
    Queued,
    /// A frame ran it at least once.
    Begun,
    /// The driver stopped waiting; no frame runs it again.
    Withdrawn(Withdrawn),
}

/// What a withdrawn request had done.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Withdrawn {
    /// No frame ran it: it never takes effect.
    Unbegun,
    /// A frame ran it: what it did stays.
    Begun,
}

impl RequestHandle {
    /// Stop the request, so no later frame runs it; whether a frame had run it.
    pub(crate) fn withdraw(&self) -> Withdrawn {
        let mut shared = self.lock();
        let withdrawn = match shared.stage {
            Stage::Queued => Withdrawn::Unbegun,
            Stage::Begun => Withdrawn::Begun,
            Stage::Withdrawn(withdrawn) => withdrawn,
        };
        shared.stage = Stage::Withdrawn(withdrawn);
        withdrawn
    }

    /// The message of the panic the request raised, once it raised one.
    pub(crate) fn panic(&self) -> Option<String> {
        self.lock().panic.clone()
    }

    /// Whether a frame may run the request, marking it begun when it may.
    fn begin(&self) -> bool {
        let mut shared = self.lock();
        match shared.stage {
            Stage::Withdrawn(_) => false,
            Stage::Queued | Stage::Begun => {
                shared.stage = Stage::Begun;
                true
            }
        }
    }

    fn lock(&self) -> MutexGuard<'_, Shared> {
        self.0.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

/// An update request and the handle its driver holds.
struct QueuedUpdate {
    request: UpdateRequest,
    handle: RequestHandle,
}

impl QueuedUpdate {
    /// Run the request against `app` and `ui` unless it was withdrawn; whether it waits for a
    /// later frame. A panic in it ends it, its message on the handle.
    fn serve(&mut self, app: &mut App, ui: &Ui) -> bool {
        if !self.handle.begin() {
            return false;
        }
        match panic::catch_unwind(AssertUnwindSafe(|| (self.request)(app, ui))) {
            Ok(served) => served == Served::Again,
            Err(payload) => {
                self.handle.lock().panic = Some(message(&*payload));
                false
            }
        }
    }
}

/// A panic's message, as its payload holds it.
fn message(payload: &(dyn Any + Send)) -> String {
    payload
        .downcast_ref::<&str>()
        .map(|text| (*text).to_string())
        .or_else(|| payload.downcast_ref::<String>().cloned())
        .unwrap_or_else(|| "a panic with no message".to_string())
}

/// The requests waiting for a frame, in the order they came.
#[derive(Default)]
pub(crate) struct DrivenRequests {
    update: Vec<QueuedUpdate>,
    record: Vec<RecordRequest>,
}

impl DrivenRequests {
    pub(crate) fn push_update(&mut self, request: UpdateRequest, handle: RequestHandle) {
        self.update.push(QueuedUpdate { request, handle });
    }

    pub(crate) fn push_record(&mut self, request: RecordRequest) {
        self.record.push(request);
    }

    /// Whether a request waits for a frame, which the app then asks for.
    pub(crate) fn pending(&self) -> bool {
        !self.update.is_empty() || !self.record.is_empty()
    }

    /// Run every record request against `ui`.
    pub(crate) fn serve_record(&mut self, ui: &mut Ui) {
        for request in self.record.drain(..) {
            request(ui);
        }
    }
}

impl App {
    /// Run each update request in order against the app and `ui`, keeping each that waits for a
    /// later frame ahead of any a served one queued. The queue is lent out, so each request can
    /// borrow all of the app; its buffer comes back, so a frame allocates nothing.
    pub(crate) fn serve_driven(&mut self, ui: &Ui) {
        let mut queued = mem::take(&mut self.driven.update);
        queued.retain_mut(|request| request.serve(self, ui));
        queued.append(&mut self.driven.update);
        self.driven.update = queued;
    }
}

impl fmt::Debug for DrivenRequests {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("DrivenRequests")
            .field("update", &self.update.len())
            .field("record", &self.record.len())
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use std::sync::mpsc;

    use glam::UVec2;
    use palantir::internals::ui_harness::UiHarness;

    use crate::gui::app::App;
    use crate::gui::app::driven_requests::{RequestHandle, Served, Withdrawn};

    /// The next frame serves the queue in order: each update request once, against the app and the
    /// frame, and each record request in its first pass. A request that waits is asked again the
    /// frame after, and keeps the frames coming until it is done: the first frame asks a repaint,
    /// the second, with the queue empty, asks none.
    #[test]
    fn the_next_frame_serves_the_queue_and_a_waiting_request_the_frame_after() {
        let mut ui = UiHarness::with_text(UVec2::new(800, 600));
        let mut app = App::offscreen(ui.ui());
        ui.frame_app(&mut app);

        let (served, log) = mpsc::channel();
        let read = served.clone();
        app.driven_mut().push_update(
            Box::new(move |app, ui| {
                read.send(format!(
                    "read {:?} at {}",
                    app.session.open.path,
                    ui.frame_id()
                ))
                .unwrap();
                Served::Done
            }),
            RequestHandle::default(),
        );
        let wait = served.clone();
        let mut asked = 0;
        app.driven_mut().push_update(
            Box::new(move |_, _| {
                asked += 1;
                wait.send(format!("wait {asked}")).unwrap();
                if asked == 2 {
                    Served::Done
                } else {
                    Served::Again
                }
            }),
            RequestHandle::default(),
        );
        app.driven_mut().push_record(Box::new(move |ui| {
            served.send(format!("record at {}", ui.frame_id())).unwrap();
        }));
        assert!(app.driven_mut().pending());

        let frame = ui.ui().frame_id();
        let first = ui.frame_app(&mut app);
        assert_eq!(
            log.try_iter().collect::<Vec<_>>(),
            [
                format!("read None at {frame}"),
                "wait 1".to_string(),
                format!("record at {frame}"),
            ],
            "in order, in the one frame"
        );
        assert!(first.repaint_requested, "a request waits");

        let second = ui.frame_app(&mut app);
        assert_eq!(log.try_iter().collect::<Vec<_>>(), ["wait 2"]);
        assert!(!app.driven_mut().pending());
        assert!(!second.repaint_requested, "the queue is empty");
    }

    /// A withdrawn request never runs, and one withdrawn after its first frame runs no second; a
    /// panic in a request ends it alone, its message on its handle, and the requests after it
    /// still run.
    #[test]
    fn a_withdrawn_request_stops_and_a_panic_ends_its_request_alone() {
        let mut ui = UiHarness::with_text(UVec2::new(800, 600));
        let mut app = App::offscreen(ui.ui());
        ui.frame_app(&mut app);
        let (served, log) = mpsc::channel();
        let mut queue = |name: &'static str, served_as: Served| {
            let served = served.clone();
            let handle = RequestHandle::default();
            app.driven_mut().push_update(
                Box::new(move |_, _| {
                    served.send(name).unwrap();
                    assert_ne!(name, "panics", "boom");
                    served_as
                }),
                handle.clone(),
            );
            handle
        };
        let withdrawn = queue("withdrawn", Served::Done);
        let waits = queue("waits", Served::Again);
        let panics = queue("panics", Served::Done);
        let after = queue("after", Served::Done);
        assert_eq!(withdrawn.withdraw(), Withdrawn::Unbegun);

        ui.frame_app(&mut app);
        assert_eq!(
            log.try_iter().collect::<Vec<_>>(),
            ["waits", "panics", "after"]
        );
        let panic = panics.panic().unwrap();
        assert!(
            panic.starts_with("assertion `left != right` failed: boom"),
            "{panic}"
        );
        assert_eq!((waits.panic(), after.panic()), (None, None));

        assert_eq!(waits.withdraw(), Withdrawn::Begun);
        ui.frame_app(&mut app);
        assert_eq!(
            log.try_iter().count(),
            0,
            "the waiting request ran no second frame"
        );
        assert!(!app.driven_mut().pending());
    }
}
