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
use crate::core::edit::graph_intent::GraphIntent;
use crate::core::edit::relayout::Relayout;
use crate::core::io::preferences::Preferences;
use crate::gui::app::commands::AppCommand;
use crate::gui::app::ctx::{AppCtx, StatusInputs};
use crate::gui::app::session::Session;
use crate::gui::requests::Requests;
use crate::gui::state::run_state::RunState;
use crate::gui::theme::Theme;
use std::iter;
use std::sync::Arc;

/// Surface every editor test frames at unless it resizes. Wide enough
/// that the dock strip lays its chips out unwrapped.
const SURFACE: UVec2 = UVec2::new(1200, 800);

#[derive(Debug)]
pub(crate) struct SessionHarness {
    /// The palantir side. `pub(crate)` so tests drive input and read
    /// geometry through it directly — `h.ui.press_at(..)`, `h.ui.rect(..)`.
    pub(crate) ui: UiHarness,
    pub(crate) session: Session,
    pub(crate) library: Arc<Library>,
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
        let theme = Theme::default();
        let mut ui = UiHarness::with_text(SURFACE);
        // What `App::new` does before frame 1, and what every palantir
        // widget darkroom records reads its geometry from — the dock's
        // tab strips among them. Without it a geometry assertion here
        // measures palantir's stock theme rather than darkroom's.
        ui.ui().set_theme(theme.palantir.clone());
        Self {
            ui,
            session: Session::new(OpenDocument::over(fixture.doc)),
            library: Arc::new(fixture.library),
            theme,
            run_state: RunState::default(),
            preferences: Preferences::default(),
            process_memory: 0,
            requests: Requests::default(),
        }
    }

    /// Push one intent through the real edit path, as a widget's does.
    /// Reports whether it stranded the canvas's cached geometry.
    pub(crate) fn apply(&mut self, intent: GraphIntent) -> Relayout {
        self.session.open.apply_edit(intent, &self.library)
    }

    /// Drain the queued intents into the document, as the frame's edit phase
    /// does. Reports whether the batch stranded the canvas's cached geometry.
    pub(crate) fn drain(&mut self) -> Relayout {
        self.session
            .open
            .drain_requests(self.requests.document(), &self.library)
    }

    /// Take back the last undoable entry. Reports whether there was one.
    pub(crate) fn undo(&mut self) -> bool {
        let took = self.session.open.can_undo();
        let _relayout = self.session.open.undo();
        took
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
        // The part of `App::update` a frame's *content* depends on: it sweeps
        // the preview store against the document, releasing what no node
        // retains any more. The rest of `update` — the run projection, the
        // cache sweeps — needs a `RuntimeHost` this harness has no reason to
        // build, and no test reads what it produces.
        //
        // Nothing here materializes a viewer's texture: the pane uploads its
        // own when it records, so an open tab draws at full resolution on the
        // frame it appears, in this harness exactly as in production.
        run_state.previews.reconcile(&session.open.document);
        let ctx = AppCtx::new(
            theme,
            library,
            run_state,
            StatusInputs {
                error: None,
                process_memory: *process_memory,
            },
        );
        // Drained *inside* every pass, as `App::record` does — it is the
        // per-pass entry point in production, and executes what each pass
        // raised. That includes the frames the harness runs first to deliver
        // held input, which is where a chord pressed before a click lands.
        let mut commands = Vec::new();
        ui.frame(|recorder: &mut Ui| {
            // Deliberately dropped: production hands this to `App::frame`,
            // which owns the app's one `request_relayout`. This harness
            // asserts on commands and documents, not on layout passes.
            let _needs_relayout = session.frame(recorder, ctx, preferences, requests);
            commands.extend(iter::from_fn(|| requests.pop_app()));
        });
        commands
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
