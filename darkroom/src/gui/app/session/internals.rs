//! Whole-editor test harness: drives [`Session::input`] and [`Session::record`] through palantir's
//! [`Harness`], so a test can feed a real pointer event and assert on
//! what the editor did with it.
//!
//! Two levels, one type. [`SessionHarness::apply`] / [`SessionHarness::drain`]
//! reach the edit path directly and record nothing — enough for the tests
//! about what an intent does to a document. [`SessionHarness::frame`] drives a
//! real record pass, which is the only way to exercise what sits between a
//! pointer event and an intent: hit-testing, response routing, pane scoping.
//!
//! **The record closure runs once per record pass, not once per frame.**
//! `Session::record` is called from `App::record`, so on a frame with
//! pending action input it runs *twice*, exactly as in production. That
//! is deliberate — it is the behaviour under test. What a caller must
//! not do is accumulate across frames on the assumption of one call per
//! frame; [`SessionHarness::frame`] returns the commands of every pass, in order.

use glam::UVec2;
use palantir::{App, ColorScheme, Harness, RecordPass, Ui, WindowToken};
use scenarium::Library;

use crate::core::document::internals::DocumentFixture;
use crate::core::edit::graph_intent::GraphIntent;
use crate::core::io::preferences::Preferences;
use crate::core::open_document::OpenDocument;
use crate::core::status_log::StatusMessage;
use crate::core::wake;
use crate::gui::app::session::Session;
use crate::gui::frame::app_command::AppCommand;
use crate::gui::frame::app_ctx::AppCtx;
use crate::gui::frame::requests::Requests;
use crate::gui::frame::stage::Stage;
use crate::gui::frame::window_parts::{StatusInputs, WindowParts};
use crate::gui::pane::graph_ui::scene::canvas_geometry::CanvasGeometry;
use crate::gui::state::run_state::RunState;
use crate::gui::theme::Theme;
use std::iter;

/// Surface every editor test frames at unless it resizes. Wide enough
/// that the dock strip lays its chips out unwrapped.
const SURFACE: UVec2 = UVec2::new(1200, 800);

/// One frame's run of the session, as `App` runs it. The commands are drained after the stage and
/// after each record, as `App` drains them, which includes the frames the harness runs first to deliver held input, where
/// a chord pressed before a click lands, and it spends the relayout owed as `App::record` does.
#[derive(Debug)]
struct SessionFrame<'a> {
    session: &'a mut Session,
    theme: &'a Theme,
    library: &'a Library,
    run_state: &'a RunState,
    preferences: &'a mut Preferences,
    process_memory: u64,
    status_message: Option<StatusMessage<'a>>,
    requests: &'a mut Requests,
    commands: Vec<AppCommand>,
}

impl App for SessionFrame<'_> {
    fn record(&mut self, _: WindowToken, ui: &mut Ui) {
        let ctx = AppCtx::new(self.theme, self.library, self.run_state);
        let mut stage = Stage {
            ctx,
            out: &mut *self.requests,
        };
        if ui.record_pass() == RecordPass::First {
            self.session.input(ui, &mut stage);
            self.commands
                .extend(iter::from_fn(|| self.requests.pop_app()));
        } else {
            self.session.settle(ui, &mut stage);
        }
        self.session.record(
            ui,
            &mut Stage {
                ctx,
                out: self.requests,
            },
            WindowParts {
                preferences: self.preferences,
                status: StatusInputs {
                    message: self.status_message,
                    process_memory: self.process_memory,
                },
            },
        );
        self.commands
            .extend(iter::from_fn(|| self.requests.pop_app()));
        self.requests.spend_relayout(ui);
    }
}

#[derive(Debug)]
pub(crate) struct SessionHarness {
    /// The palantir side. `pub(crate)` so tests drive input and read
    /// geometry through it directly — `h.ui.press(.., PointerButton::Left)`, `h.ui.rect(..)`.
    pub(crate) ui: Harness,
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
    /// The status bar's line, `App`'s status log in production.
    pub(crate) status_message: Option<StatusMessage<'static>>,
    /// The frame's request queue — `App`'s in production. `pub(crate)` so a
    /// test can seed it the way a widget does.
    pub(crate) requests: Requests,
}

impl Session {
    /// The canvas's geometry, for a test that reads which ports a pass could anchor a wire to.
    pub(crate) fn geometry(&self) -> &CanvasGeometry {
        self.main_window.graph_ui.geometry()
    }
}

impl SessionHarness {
    /// Real text shaping — the dock strip and node headers size to their
    /// labels, so mono metrics would put every chip in the wrong place.
    pub(crate) fn new(fixture: DocumentFixture) -> Self {
        let theme = Theme::default();
        let mut ui = Harness::new(SURFACE);
        // What `App::new` does before frame 1, and what every palantir
        // widget darkroom records reads its geometry from — the dock's
        // tab strips among them. Without it a geometry assertion here
        // measures palantir's stock theme rather than darkroom's.
        ui.ui_mut()
            .set_theme(ColorScheme::Dark, theme.palantir.clone());
        Self {
            ui,
            session: Session::new(OpenDocument::over(fixture.document)),
            library: fixture.library,
            theme,
            run_state: RunState::new(wake::internals::noop()),
            preferences: Preferences::default(),
            process_memory: 0,
            status_message: None,
            requests: Requests::default(),
        }
    }

    /// Push one intent through the real edit path, as a widget's does.
    pub(crate) fn apply(&mut self, intent: GraphIntent) {
        self.session
            .apply_edit(intent, &self.library, &mut self.requests);
    }

    /// Drain the queued intents into the document, as the frame's edit phase does.
    pub(crate) fn drain(&mut self) {
        self.session.drain(&self.library, &mut self.requests);
    }

    /// Take back the last undoable entry. Reports whether there was one.
    pub(crate) fn undo(&mut self) -> bool {
        let took = self.session.open.can_undo();
        self.session.undo(&mut self.requests);
        took
    }

    /// One editor frame, `App::sweep`'s work then its record passes. Returns the commands
    /// every pass raised, in order; a frame with pending action input records twice.
    pub(crate) fn frame(&mut self) -> Vec<AppCommand> {
        let Self {
            ui,
            session,
            library,
            theme,
            run_state,
            preferences,
            process_memory,
            status_message,
            requests,
        } = self;
        // `App::sweep`'s work, once a frame before its first pass: the preview store against
        // the document, as the run projection's sync ends, and the window's caches. The rest of
        // the sync drains a `RuntimeHost` this harness has no reason to build.
        //
        // Nothing here materializes a viewer's texture: the pane uploads its
        // own when it records, so an open tab draws at full resolution on the
        // frame it appears, in this harness exactly as in production.
        run_state.previews_mut().reconcile(session.open.document());
        session.reconcile_caches();
        let mut frame = SessionFrame {
            session,
            theme,
            library,
            run_state,
            preferences,
            process_memory: *process_memory,
            status_message: *status_message,
            requests,
            commands: Vec::new(),
        };
        ui.frame_app(&mut frame);
        frame.commands
    }

    /// `n` frames whose commands are discarded — the editor equivalent of
    /// `Harness::prime`. Two is the minimum before reading geometry:
    /// one to lay out, one for `response_for` to resolve against a
    /// settled cascade.
    pub(crate) fn prime(&mut self, n: u32) {
        for _ in 0..n {
            let _ = self.frame();
        }
    }
}
