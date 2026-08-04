//! The canvas a test drives real input at: a [`UiHarness`] surface, the
//! [`GraphUI`] recorded onto it, and the document both read.
//!
//! One type rather than a closure per test file because the subject under test
//! is the same machinery in each case — [`Self::frame`] runs the sequence
//! `MainWindow` runs, `prepass` → `draw`, and only the gesture driven
//! through it differs. A test that skipped a phase would be exercising a
//! frame the app never performs.

use glam::{UVec2, Vec2};
use palantir::internals::UiHarness;
use palantir::{Configure, Panel, Rect, Sizing, Ui};
use scenarium::NodeId;

use crate::core::document::dock::DockOp;
use crate::core::document::harness::DocFixture;
use crate::core::document::{Document, PortRef};
use crate::core::edit::intent::types::GraphIntent;
use crate::gui::app::commands::AppCommand;
use crate::gui::graph_ctx::harness::GraphCtxFixture;
use crate::gui::pane::graph::GraphUI;
use crate::gui::pane::graph::node::port_row::port_circle_wid;
use crate::gui::pane::graph::node::wid;
use crate::gui::requests::{DocumentRequest, Requests};

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
    /// The app commands the last [`Self::frame`] raised. Kept beside the
    /// intents that call returns rather than folded into them: the canvas
    /// raises both tiers, and a test is almost always about one of them.
    pub(crate) commands: Vec<AppCommand>,
    /// The view ops the last [`Self::frame`] raised. Rare — the canvas asks
    /// for one thing the dock owns, a viewer tab for a clicked preview card —
    /// but real, so they are collected rather than treated as impossible.
    pub(crate) view_ops: Vec<DockOp>,
}

impl CanvasHarness {
    /// [`SURFACE`] with mono metrics — what a canvas test wants unless it is
    /// about measured text or about the surface itself.
    ///
    /// Takes a [`DocFixture`] or anything that converts into one — a
    /// [`TestGraph`](scenarium::testing::graph::TestGraph) goes straight in.
    pub(crate) fn new(fixture: impl Into<DocFixture>) -> Self {
        Self::over(UiHarness::new(SURFACE), fixture)
    }

    /// A different surface, still mono: for the tests whose subject is what
    /// fits on one.
    pub(crate) fn sized(fixture: impl Into<DocFixture>, surface: UVec2) -> Self {
        Self::over(UiHarness::new(surface), fixture)
    }

    /// Real text shaping: node headers and menu rows size to their labels, so
    /// mono metrics would put every one of them in the wrong place.
    pub(crate) fn shaping_text(fixture: impl Into<DocFixture>, surface: UVec2) -> Self {
        Self::over(UiHarness::with_text(surface), fixture)
    }

    fn over(ui: UiHarness, fixture: impl Into<DocFixture>) -> Self {
        Self {
            ui,
            graph_ui: GraphUI::default(),
            ctx: GraphCtxFixture::over(fixture),
            commands: Vec::new(),
            view_ops: Vec::new(),
        }
    }

    /// One canvas frame, in the order `MainWindow` runs its phases, returning
    /// the graph intents it raised. App commands land in [`Self::commands`],
    /// view ops in [`Self::view_ops`] — one tier per sink, so a test reads the
    /// one it is about without matching the others away.
    pub(crate) fn frame(&mut self) -> Vec<GraphIntent> {
        let Self {
            ui,
            graph_ui,
            ctx,
            commands,
            view_ops,
        } = self;
        let mut out = ui.frame_value(|recorder: &mut Ui| Self::record(graph_ui, ctx, recorder));
        view_ops.clear();
        let intents: Vec<GraphIntent> = out
            .drain_document()
            .filter_map(|request| match request {
                DocumentRequest::Graph(intent) => Some(intent),
                DocumentRequest::View(op) => {
                    view_ops.push(op);
                    None
                }
            })
            .collect();
        commands.clear();
        commands.extend(std::iter::from_fn(|| out.pop_app()));
        intents
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
        let Self {
            ui, graph_ui, ctx, ..
        } = self;
        ui.advance_past_double_click(|recorder: &mut Ui| {
            Self::record(graph_ui, ctx, recorder);
        });
    }

    /// One frame that is allowed to be paint-only, reporting whether a record
    /// pass ran. The wake a focused `TextEdit`'s caret blink schedules is
    /// enough to produce one, and [`Self::frame`] panics on those.
    pub(crate) fn try_frame(&mut self) -> bool {
        let Self {
            ui, graph_ui, ctx, ..
        } = self;
        ui.try_frame_value(|recorder: &mut Ui| {
            let _ = Self::record(graph_ui, ctx, recorder);
        })
        .is_some()
    }

    /// The record body both frame drivers run.
    fn record(graph_ui: &mut GraphUI, ctx: &mut GraphCtxFixture, recorder: &mut Ui) -> Requests {
        let mut out = Requests::default();
        let graph_ctx = ctx.graph_ctx();
        graph_ui.prepass(recorder, graph_ctx, &mut out);
        Panel::vstack()
            .id_salt("pane")
            .size((Sizing::FILL, Sizing::FILL))
            .show(recorder, |ui| {
                graph_ui.draw(ui, graph_ctx, &mut out);
            });
        out
    }

    pub(crate) fn doc(&self) -> &Document {
        &self.ctx.open.document
    }

    /// The document, to edit between frames — an undo moving a node out from
    /// under a live gesture is the case this exists for.
    pub(crate) fn doc_mut(&mut self) -> &mut Document {
        &mut self.ctx.open.document
    }

    /// The `i`th node in placement order.
    pub(crate) fn node(&self, i: usize) -> NodeId {
        self.ctx.node(i)
    }

    /// Where `node_id`'s body last painted — post-transform, so it is the rect
    /// a pointer aimed at the node has to land in.
    pub(crate) fn node_rect(&self, node_id: NodeId) -> Rect {
        self.ui
            .rect(wid::body(node_id))
            .unwrap_or_else(|| panic!("{node_id:?} recorded no body — was the frame primed?"))
    }

    /// The point a press grabs `node_id`'s body at.
    pub(crate) fn node_center(&self, node_id: NodeId) -> Vec2 {
        self.ui.center_of(wid::body(node_id))
    }

    /// The point a wire drag leaves `port` from, or lands on — the snap scan
    /// tests the post-transform rect, so this is the one to aim at.
    pub(crate) fn port_center(&self, port: PortRef) -> Vec2 {
        self.ui.center_of(port_circle_wid(port))
    }

    /// The cached world rect wires anchor to and the cull reads, or `None`
    /// once the node's entry has been evicted. Unlike [`Self::node_rect`] this
    /// survives the node being culled — that is the whole point of the cache.
    pub(crate) fn node_world_rect(&mut self, node_id: NodeId) -> Option<Rect> {
        let Self { graph_ui, ctx, .. } = self;
        let node = ctx
            .graph_ctx()
            .node(node_id)
            .expect("the document still holds that node");
        graph_ui.geometry().node_world_rect(node)
    }
}
