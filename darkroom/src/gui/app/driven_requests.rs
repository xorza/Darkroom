//! [`DrivenRequests`]: the work a driver on another thread hands the app, done in the app's next
//! frames, where palantir lends the `Ui`. `darkroom/AGENTS.md`, "The editor harness", "One
//! driver, two backends".

use std::fmt;
use std::mem;

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

/// The requests waiting for a frame, in the order they came.
#[derive(Default)]
pub(crate) struct DrivenRequests {
    update: Vec<UpdateRequest>,
    record: Vec<RecordRequest>,
    /// The buffer the update requests that wait for a later frame are gathered in.
    again: Vec<UpdateRequest>,
}

impl DrivenRequests {
    pub(crate) fn push_update(&mut self, request: UpdateRequest) {
        self.update.push(request);
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

    /// Lend the update requests out, so each can borrow all of the app; [`Self::restore`] takes
    /// them back.
    pub(crate) fn lend(&mut self) -> LentRequests {
        LentRequests {
            requests: mem::take(&mut self.update),
            again: mem::take(&mut self.again),
        }
    }

    /// Take back what [`Self::lend`] lent: the requests that wait for a later frame, ahead of any
    /// a served one queued.
    pub(crate) fn restore(&mut self, lent: LentRequests) {
        let LentRequests {
            requests,
            mut again,
        } = lent;
        debug_assert!(requests.is_empty(), "every lent request ran");
        again.append(&mut self.update);
        self.update = again;
        self.again = requests;
    }
}

/// The update requests [`DrivenRequests::lend`] lent.
pub(crate) struct LentRequests {
    requests: Vec<UpdateRequest>,
    again: Vec<UpdateRequest>,
}

impl LentRequests {
    /// Run each request in order against `app` and `ui`, keeping each that waits for a later
    /// frame.
    pub(crate) fn serve(&mut self, app: &mut App, ui: &Ui) {
        for mut request in self.requests.drain(..) {
            if request(app, ui) == Served::Again {
                self.again.push(request);
            }
        }
    }
}

impl fmt::Debug for LentRequests {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("LentRequests")
            .field("requests", &self.requests.len())
            .field("again", &self.again.len())
            .finish()
    }
}

impl fmt::Debug for DrivenRequests {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("DrivenRequests")
            .field("update", &self.update.len())
            .field("record", &self.record.len())
            .finish_non_exhaustive()
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::sync::mpsc;

    use glam::UVec2;
    use palantir::internals::ui_harness::UiHarness;

    use crate::core::io::preferences::Preferences;
    use crate::core::io::preferences::preferences_store::PreferencesStore;
    use crate::gui::app::app_host::AppHost;
    use crate::gui::app::driven_requests::Served;
    use crate::gui::app::offscreen_signals::OffscreenSignals;
    use crate::gui::app::{App, LaunchInputs};
    use crate::gui::dialogs::{DialogFallback, Dialogs};

    /// The next frame serves the queue in order: each update request once, against the app and the
    /// frame, and each record request in its first pass. A request that waits is asked again the
    /// frame after, and keeps the frames coming until it is done: the first frame asks a repaint,
    /// the second, with the queue empty, asks none.
    #[test]
    fn the_next_frame_serves_the_queue_and_a_waiting_request_the_frame_after() {
        let mut ui = UiHarness::with_text(UVec2::new(800, 600));
        let mut app = App::new(
            ui.ui(),
            AppHost::Offscreen(Arc::new(OffscreenSignals::default())),
            LaunchInputs {
                preferences: Ok(Preferences::default()),
                store: PreferencesStore::Memory,
                document: None,
                dialogs: Dialogs::new(DialogFallback::Refuse),
            },
        );
        ui.frame_app(&mut app);

        let (served, log) = mpsc::channel();
        let read = served.clone();
        app.driven_mut().push_update(Box::new(move |app, ui| {
            read.send(format!(
                "read {:?} at {}",
                app.session.open.path,
                ui.frame_id()
            ))
            .unwrap();
            Served::Done
        }));
        let wait = served.clone();
        let mut asked = 0;
        app.driven_mut().push_update(Box::new(move |_, _| {
            asked += 1;
            wait.send(format!("wait {asked}")).unwrap();
            if asked == 2 {
                Served::Done
            } else {
                Served::Again
            }
        }));
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
}
