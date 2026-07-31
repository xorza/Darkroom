//! The canvas a test drives real input at: a [`UiHarness`] surface, the
//! [`GraphUI`] recorded onto it, and the document both read.
//!
//! One type rather than a closure per test file because the subject under test
//! is the same machinery in each case — [`Self::frame`] runs the sequence
//! `MainWindow` runs, `scan_hits` → `prepass` → `draw`, and only the gesture
//! driven through it differs. A test that skipped a phase would be exercising a
//! frame the app never performs.

use glam::UVec2;
use palantir::internals::UiHarness;
use palantir::{Configure, Panel, Sizing, Ui};
use scenarium::NodeId;

use crate::core::document::Document;
use crate::core::document::harness::DocFixture;
use crate::core::edit::intent::sink::{Intents, Queued};
use crate::core::edit::intent::types::GraphIntent;
use crate::gui::graph_ctx::harness::GraphCtxFixture;
use crate::gui::pane::graph::GraphUI;

/// Surface every canvas test records at unless it is about size. Wide enough
/// that [`DocFixture`]'s row of nodes lands on screen uncropped.
const SURFACE: UVec2 = UVec2::new(1200, 800);

#[derive(Debug)]
pub(crate) struct CanvasHarness {
    /// The palantir side. `pub(crate)` so tests drive input and read geometry
    /// through it directly — `h.ui.press_at(..)`, `h.ui.rect(..)`.
    pub(crate) ui: UiHarness,
    pub(crate) graph_ui: GraphUI,
    /// The document the canvas draws and the context it resolves through.
    /// `pub(crate)` so a test can edit the document between frames, the way an
    /// undo edits it under a running gesture.
    pub(crate) ctx: GraphCtxFixture,
}

impl CanvasHarness {
    /// [`SURFACE`] with mono metrics — what a canvas test wants unless it is
    /// about measured text or about the surface itself.
    pub(crate) fn new(fixture: DocFixture) -> Self {
        Self::over(UiHarness::new(SURFACE), fixture)
    }

    /// A different surface, still mono: for the tests whose subject is what
    /// fits on one.
    pub(crate) fn sized(fixture: DocFixture, surface: UVec2) -> Self {
        Self::over(UiHarness::new(surface), fixture)
    }

    /// Real text shaping: node headers and menu rows size to their labels, so
    /// mono metrics would put every one of them in the wrong place.
    pub(crate) fn shaping_text(fixture: DocFixture, surface: UVec2) -> Self {
        Self::over(UiHarness::with_text(surface), fixture)
    }

    fn over(ui: UiHarness, fixture: DocFixture) -> Self {
        Self {
            ui,
            graph_ui: GraphUI::default(),
            ctx: GraphCtxFixture::over(fixture),
        }
    }

    /// One canvas frame, in the order `MainWindow` runs its phases, returning
    /// the graph intents it raised.
    ///
    /// Panics if a canvas widget raised a dock intent: none of them can reach
    /// the dock, so one that did is our own bug rather than a case to assert
    /// on.
    pub(crate) fn frame(&mut self) -> Vec<GraphIntent> {
        let Self { ui, graph_ui, ctx } = self;
        ui.frame_value(|recorder: &mut Ui| Self::record(graph_ui, ctx, recorder))
    }

    /// `n` frames whose intents are discarded. Two is the minimum before
    /// reading geometry: one to lay out, one for the caches the passes read to
    /// resolve against a settled cascade.
    pub(crate) fn prime(&mut self, n: u32) {
        for _ in 0..n {
            let _ = self.frame();
        }
    }

    /// Let the double-click window lapse, recording throughout, so the next
    /// press opens a fresh click rather than extending the last one.
    pub(crate) fn advance_past_double_click(&mut self) {
        let Self { ui, graph_ui, ctx } = self;
        ui.advance_past_double_click(|recorder: &mut Ui| {
            Self::record(graph_ui, ctx, recorder);
        });
    }

    /// The record body both frame drivers run.
    fn record(
        graph_ui: &mut GraphUI,
        ctx: &mut GraphCtxFixture,
        recorder: &mut Ui,
    ) -> Vec<GraphIntent> {
        let mut intents = Intents::default();
        let graph_ctx = ctx.graph_ctx();
        graph_ui.scan_hits(recorder, Some(graph_ctx));
        graph_ui.prepass(recorder, graph_ctx, &mut intents);
        Panel::vstack()
            .id_salt("pane")
            .size((Sizing::FILL, Sizing::FILL))
            .show(recorder, |ui| {
                graph_ui.draw(ui, graph_ctx, &mut intents);
            });
        intents
            .drain()
            .map(|item| match item {
                Queued::Graph(intent) => intent,
                Queued::Dock(intent) => panic!("a canvas widget raised {intent:?}"),
            })
            .collect()
    }

    pub(crate) fn doc(&self) -> &Document {
        &self.ctx.fixture.doc
    }

    /// The document, to edit between frames — an undo moving a node out from
    /// under a live gesture is the case this exists for.
    pub(crate) fn doc_mut(&mut self) -> &mut Document {
        &mut self.ctx.fixture.doc
    }

    /// The `i`th node in placement order.
    pub(crate) fn node(&self, i: usize) -> NodeId {
        self.ctx.fixture.node(i)
    }
}
