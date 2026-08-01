//! Whole-editor test harness: drives [`Editor::frame`] through palantir's
//! [`UiHarness`], so a test can feed a real pointer event and assert on
//! what the editor did with it.
//!
//! Two levels, one type. [`SessionHarness::apply`] / [`SessionHarness::drain`]
//! reach the edit path directly and record nothing — enough for the tests
//! about what an intent does to a document. [`SessionHarness::frame`] drives a
//! real record pass, which is the only way to exercise what sits between a
//! pointer event and an intent: hit-testing, response routing, pane scoping.
//!
//! **The record closure runs once per record pass, not once per frame.**
//! `Editor::frame` is called from `App::record`, so on a frame with
//! pending action input it runs *twice*, exactly as in production. That
//! is deliberate — it is the behaviour under test. What a caller must
//! not do is accumulate across frames on the assumption of one call per
//! frame; [`Self::frame`] returns the first pass's command for the same
//! reason `UiHarness::frame_value` returns pass A.

use glam::UVec2;
use palantir::Ui;
use palantir::internals::UiHarness;
use scenarium::Library;

use crate::core::document::harness::DocFixture;
use crate::core::document::open_document::OpenDocument;
use crate::core::edit::intent::types::GraphIntent;
use crate::core::io::preferences::Preferences;
use crate::gui::app::commands::AppCommand;
use crate::gui::app::ctx::{AppCtx, StatusInputs};
use crate::gui::app::session::Session;
use crate::gui::requests::Requests;
use crate::gui::state::run_state::RunState;
use crate::gui::theme::Theme;

/// Surface every editor test frames at unless it resizes. Wide enough
/// that the dock strip lays its chips out unwrapped.
const SURFACE: UVec2 = UVec2::new(1200, 800);

#[derive(Debug)]
pub(crate) struct SessionHarness {
    /// The palantir side. `pub(crate)` so tests drive input and read
    /// geometry through it directly — `h.ui.press_at(..)`, `h.ui.rect(..)`.
    pub(crate) ui: UiHarness,
    pub(crate) session: Session,
    pub(crate) library: Library,
    pub(crate) theme: Theme,
    /// The run projections the frame reads — `App`'s in production, so a
    /// test that wants a node to look executed writes it here.
    pub(crate) run_state: RunState,
    pub(crate) preferences: Preferences,
    /// Footprint handed to the status bar. `0` — the default — is the
    /// no-reading path, so a test asserting on geometry isn't reading a
    /// figure that moves between runs; set it to pin the `MEM` clause.
    pub(crate) process_memory: u64,
    /// The frame's request queue — `App`'s in production. `pub(crate)` so a
    /// test can seed it the way a widget does.
    pub(crate) requests: Requests,
}

impl SessionHarness {
    /// Real text shaping — the dock strip and node headers size to their
    /// labels, so mono metrics would put every chip in the wrong place.
    pub(crate) fn new(fixture: DocFixture) -> Self {
        Self {
            ui: UiHarness::with_text(SURFACE),
            session: Session::new(OpenDocument::over(fixture.doc)),
            library: fixture.library,
            theme: Theme::default(),
            run_state: RunState::default(),
            preferences: Preferences::default(),
            process_memory: 0,
            requests: Requests::default(),
        }
    }

    /// Push one intent through the real edit path, as a widget's does.
    /// Reports whether it stranded the canvas's cached geometry.
    pub(crate) fn apply(&mut self, intent: GraphIntent) -> bool {
        self.session.open.apply_edit(intent)
    }

    /// Drain the queued intents into the document, as the frame's edit phase
    /// does. Reports whether the batch stranded the canvas's cached geometry.
    pub(crate) fn drain(&mut self) -> bool {
        self.session.open.drain_requests(&mut self.requests)
    }

    /// Take back the last undoable entry. Reports whether there was one.
    pub(crate) fn undo(&mut self) -> bool {
        self.session.open.undo().took
    }

    /// One editor frame. Returns the commands the **first** record pass
    /// produced; a frame with pending action input records twice and the
    /// second pass no longer sees the one-frame edges that raise most
    /// commands.
    pub(crate) fn frame(&mut self) -> Vec<AppCommand> {
        let Self {
            ui,
            session,
            library,
            theme,
            run_state,
            preferences,
            process_memory,
            requests,
        } = self;
        let ctx = AppCtx::new(
            theme,
            library,
            run_state,
            StatusInputs {
                error: None,
                process_memory: *process_memory,
            },
        );
        // Drained *inside* the pass, as `App::record` does — it is the
        // per-pass entry point in production. Draining after `frame_value`
        // would read the queue pass B left, and pass B clears what pass A
        // raised.
        ui.frame_value(|recorder: &mut Ui| {
            // Deliberately dropped: production hands this to `App::frame`,
            // which owns the app's one `request_relayout`. This harness
            // asserts on commands and documents, not on layout passes.
            let _needs_relayout = session.frame(recorder, ctx, preferences, requests);
            std::iter::from_fn(|| requests.pop_app()).collect()
        })
    }

    /// `n` frames whose commands are discarded — the editor equivalent of
    /// `UiHarness::prime`. Two is the minimum before reading geometry:
    /// one to lay out, one for `response_for` to resolve against a
    /// settled cascade.
    pub(crate) fn prime(&mut self, n: u32) {
        for _ in 0..n {
            let _ = self.frame();
        }
    }
}

#[cfg(test)]
mod tests {

    use super::SessionHarness;

    use crate::core::document::harness::DocFixture;

    use crate::gui::window::status_bar::status_bar_id;

    /// The bar used to collapse when it had nothing to say; the process
    /// footprint gives it something on every frame, so it is recorded on
    /// an untouched document — and stays recorded when no reading is
    /// available, rather than reappearing as the figure lands.
    #[test]
    fn status_bar_is_recorded_on_an_idle_document_with_or_without_a_reading() {
        let mut h = SessionHarness::new(DocFixture::default());
        h.prime(2);
        let without =
            h.ui.rect(status_bar_id())
                .expect("status bar records with no reading and an empty cache");

        h.process_memory = 3 * 1024 * 1024;
        h.prime(2);
        let with = h.ui.rect(status_bar_id()).expect("status bar records");

        // The strip is a real row either way — a collapsed one would
        // arrange to zero height and read as "no bar".
        for (rect, what) in [(without, "no reading"), (with, "3 MB reading")] {
            assert!(rect.size.h > 0.0, "{what}: bar arranged to zero height");
            assert!(rect.size.w > 0.0, "{what}: bar arranged to zero width");
        }
        // With a reading the bar hugs a line of text; without one it is
        // padding alone, so it is strictly shorter. Both are still rows.
        assert!(
            with.size.h > without.size.h,
            "a reading adds its label's line to the bar: {} vs {}",
            with.size.h,
            without.size.h,
        );
    }
}
