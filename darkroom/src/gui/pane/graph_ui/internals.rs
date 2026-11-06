//! The canvas a test drives real input at: a [`Harness`] surface, the
//! [`GraphUi`] recorded onto it, and the document both read.
//!
//! One type rather than a closure per test file because the subject under test
//! is the same machinery in each case — a frame runs the phases `MainWindow`
//! runs, `prepass` → `draw`, and only the gesture driven through it differs. A
//! test that skipped a phase would be exercising a frame the app never performs.

use glam::{UVec2, Vec2};
use palantir::{App, Harness, InputOutcome, UpdateCtx, WindowToken};
use palantir::{Configure, DockOperation, Panel, Rect, Sizing, SystemScale, Ui, WidgetId};
use scenarium::NodeId;

use crate::core::document::Document;
use crate::core::document::internals::DocumentFixture;
use crate::core::document::tab_ref::TabRef;
use crate::core::edit::document_request::DocumentRequest;
use crate::core::edit::graph_intent::GraphIntent;
use crate::gui::frame::app_command::AppCommand;
use crate::gui::frame::requests::Requests;
use crate::gui::graph_ctx::internals::GraphCtxFixture;
use crate::gui::pane::graph_ui::GraphUi;
use crate::gui::pane::graph_ui::frame::canvas_geometry::CanvasGeometry;
use crate::gui::pane::graph_ui::wid;
use crate::gui::port_ref::PortRef;
use std::iter;
use std::mem;

impl GraphUi {
    /// The canvas's cross-frame geometry cache, for the tests that assert
    /// on what a record filled into it and what [`GraphUi::retain_nodes`]
    /// released — port centers, node sizes, cached world rects.
    ///
    /// Shared, not `&mut`: a test reads what the passes settled, it does
    /// not seed the cache by hand.
    pub(crate) fn geometry(&self) -> &CanvasGeometry {
        &self.geometry
    }
}

/// Surface every canvas test records at unless it is about size. Wide enough
/// that [`DocumentFixture`]'s row of nodes lands on screen uncropped.
const SURFACE: UVec2 = UVec2::new(1200, 800);

/// How far down from the anchor [`menu_rows`] sweeps, how far in from its
/// left edge, and how fine its step. The sweep has to clear the whole menu,
/// and the step has to be finer than its thinnest band (a separator) or a row
/// would read as two.
const PROBE_DEPTH: f32 = 320.0;
const PROBE_INSET: f32 = 20.0;
const PROBE_STEP: f32 = 2.0;

#[derive(Debug)]
pub(crate) struct CanvasHarness {
    /// The palantir side. `pub(crate)` so tests drive input and read geometry
    /// through it directly — `h.ui.press(.., PointerButton::Left)`, `h.ui.rect(..)`.
    pub(crate) ui: Harness,
    pub(crate) graph_ui: GraphUi,
    /// The document the canvas draws and the context it resolves through.
    /// `pub(crate)` so a test can edit the document between frames, the way an
    /// undo edits it under a running gesture.
    pub(crate) ctx: GraphCtxFixture,
    /// What the last frame's input stage answered.
    pub(crate) outcome: InputOutcome,
    /// The app commands the last [`Self::frame`] raised. Kept beside the
    /// intents that call returns rather than folded into them: the canvas
    /// raises both tiers, and a test is almost always about one of them.
    pub(crate) commands: Vec<AppCommand>,
    /// The view ops the last [`Self::frame`] raised. Rare — the canvas asks
    /// for one thing the dock owns, a viewer tab for a clicked preview card —
    /// but real, so they are collected rather than treated as impossible.
    pub(crate) view_ops: Vec<DockOperation<TabRef>>,
}

impl CanvasHarness {
    /// [`SURFACE`] with mono metrics — what a canvas test wants unless it is
    /// about measured text or about the surface itself.
    ///
    /// Takes a [`DocumentFixture`] or anything that converts into one — a
    /// [`TestGraph`](scenarium::internals::test_graph::TestGraph) goes straight in.
    pub(crate) fn new(fixture: impl Into<DocumentFixture>) -> Self {
        Self::over(Harness::builder(SURFACE).mono().build(), fixture)
    }

    /// A different surface, still mono: for the tests whose subject is what
    /// fits on one.
    pub(crate) fn sized(fixture: impl Into<DocumentFixture>, surface: UVec2) -> Self {
        Self::over(Harness::builder(surface).mono().build(), fixture)
    }

    /// [`SURFACE`], still mono, on a display at `scale`: for the tests about where the canvas
    /// lands on physical pixels.
    pub(crate) fn scaled(fixture: impl Into<DocumentFixture>, scale: SystemScale) -> Self {
        Self::over(
            Harness::builder(SURFACE).mono().system_scale(scale).build(),
            fixture,
        )
    }

    /// Real text shaping: node headers and menu rows size to their labels, so
    /// mono metrics would put every one of them in the wrong place.
    pub(crate) fn shaping_text(fixture: impl Into<DocumentFixture>, surface: UVec2) -> Self {
        Self::over(Harness::new(surface), fixture)
    }

    fn over(ui: Harness, fixture: impl Into<DocumentFixture>) -> Self {
        Self {
            ui,
            graph_ui: GraphUi::default(),
            ctx: GraphCtxFixture::over(fixture),
            outcome: InputOutcome::Record,
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
            outcome,
            commands,
            view_ops,
        } = self;
        let mut frame = CanvasFrame::new(graph_ui, ctx, AppliesIntents::No);
        ui.frame_app(&mut frame);
        *outcome = frame.outcome;
        let mut out = frame.finish(ui);
        view_ops.clear();
        let intents: Vec<GraphIntent> = out
            .document()
            .drain()
            .filter_map(|request| match request {
                // A test reads what a gesture raised: a cancel by the move it falls back on.
                DocumentRequest::Graph(intent)
                | DocumentRequest::InGesture { intent, .. }
                | DocumentRequest::Cancel {
                    otherwise: intent, ..
                } => Some(intent),
                DocumentRequest::View(op) => {
                    view_ops.push(op);
                    None
                }
            })
            .collect();
        commands.clear();
        commands.extend(iter::from_fn(|| out.pop_app()));
        intents
    }

    /// One canvas frame as the window runs it: the prepass, its graph intents applied to the
    /// document, then the record, which reads the document they left, as the session drains
    /// them between the two. [`Self::frame`] hands its intents back unapplied instead, so a test
    /// reads exactly what a gesture raised.
    pub(crate) fn frame_applied(&mut self) {
        let Self {
            ui, graph_ui, ctx, ..
        } = self;
        let mut frame = CanvasFrame::new(graph_ui, ctx, AppliesIntents::Yes);
        ui.frame_app(&mut frame);
        let _ = frame.finish(ui);
    }

    /// `n` frames whose intents are discarded. Two is the minimum before
    /// reading geometry: one to lay out, one for the caches the passes read to
    /// resolve against a settled cascade.
    pub(crate) fn prime(&mut self, n: u32) {
        for _ in 0..n {
            let _ = self.frame();
        }
    }

    /// Let the double-click interval lapse, so the next press opens a fresh
    /// click rather than extending the last one. Records no frame: a press is
    /// stamped when it arrives, so the gap alone separates the runs.
    pub(crate) fn advance_past_double_click(&mut self) {
        self.ui.advance_past_double_click();
    }

    /// One frame that is allowed to be paint-only, reporting whether a record
    /// pass ran. The wake a focused `TextEdit`'s caret blink schedules is
    /// enough to produce one, and [`Self::frame`] panics on those.
    pub(crate) fn try_frame(&mut self) -> bool {
        let Self {
            ui, graph_ui, ctx, ..
        } = self;
        let mut frame = CanvasFrame::new(graph_ui, ctx, AppliesIntents::No);
        ui.frame_app(&mut frame);
        frame.recorded_at == Some(ui.ui().render_frame_id())
    }

    pub(crate) fn document(&self) -> &Document {
        self.ctx.open.document()
    }

    /// The document, to edit between frames — an undo moving a node out from
    /// under a live gesture is the case this exists for.
    pub(crate) fn document_mut(&mut self) -> &mut Document {
        self.ctx.open.document_mut()
    }

    /// The `i`th node in placement order.
    pub(crate) fn node(&self, i: usize) -> NodeId {
        self.ctx.node(i)
    }

    /// The clickable rows of the canvas menu open at `anchor`, top to bottom.
    ///
    /// [`MenuItem`](palantir::MenuItem) takes no caller-supplied id, so a test
    /// finds the rows the way a pointer does: sweep a vertical line down the open
    /// popup and keep every widget it lands on. The popup's own panel shows
    /// through the padding above the first row, the gaps between rows, and every
    /// separator, so it is the one widget the sweep meets repeatedly — and its
    /// first and last sightings bracket exactly the rows.
    pub(crate) fn menu_rows(&self, anchor: Vec2) -> Vec<WidgetId> {
        let mut runs: Vec<WidgetId> = Vec::new();
        let mut y = anchor.y;
        while y < anchor.y + PROBE_DEPTH {
            if let Some(id) = self.ui.hit_at(Vec2::new(anchor.x + PROBE_INSET, y))
                && runs.last() != Some(&id)
            {
                runs.push(id);
            }
            y += PROBE_STEP;
        }
        let sightings = |id: &WidgetId| runs.iter().filter(|other| *other == id).count();
        let panel = *runs
            .iter()
            .max_by_key(|id| sightings(id))
            .expect("the sweep crossed the open menu");
        let first = runs.iter().position(|id| *id == panel).unwrap();
        let last = runs.iter().rposition(|id| *id == panel).unwrap();
        runs[first..last]
            .iter()
            .copied()
            .filter(|id| *id != panel)
            .collect()
    }

    /// Where `node_id`'s body last painted — post-transform, so it is the rect
    /// a pointer aimed at the node has to land in.
    pub(crate) fn node_rect(&self, node_id: NodeId) -> Rect {
        self.ui
            .rect(wid::body(node_id))
            .unwrap_or_else(|| panic!("{node_id:?} recorded no body — was the frame primed?"))
    }

    /// The point a press grabs `node_id`'s body at: the middle of its footer, past the run status
    /// at its left, where no widget takes the press. The body's own center can land on a port
    /// row's field.
    pub(crate) fn node_center(&self, node_id: NodeId) -> Vec2 {
        self.ui
            .rect(wid::footer(node_id))
            .expect("the widget shows")
            .center()
    }

    /// The point a wire drag leaves `port` from, or lands on — the snap scan
    /// tests the post-transform rect, so this is the one to aim at.
    pub(crate) fn port_center(&self, port: PortRef) -> Vec2 {
        self.ui
            .rect(wid::port_circle(port))
            .expect("the widget shows")
            .center()
    }

    /// The cached world rect wires anchor to and the cull reads, or `None`
    /// once the node's entry has been evicted. Unlike [`Self::node_rect`] this
    /// survives the node being culled — that is the whole point of the cache.
    pub(crate) fn node_world_rect(&self, node_id: NodeId) -> Option<Rect> {
        let Self { graph_ui, ctx, .. } = self;
        let node = ctx
            .graph_ctx()
            .node(node_id)
            .expect("the document still holds that node");
        graph_ui.geometry().node_world_rect(node)
    }
}

/// Whether a frame applies the graph intents it raises to the document, as the session's drains do.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum AppliesIntents {
    No,
    Yes,
}

/// One canvas frame as the window runs it. It keeps what the input stage and the first record pass
/// raised, as a later pass has no edges to raise anything from.
#[derive(Debug)]
struct CanvasFrame<'a> {
    graph_ui: &'a mut GraphUi,
    ctx: &'a mut GraphCtxFixture,
    applies: AppliesIntents,
    out: Requests,
    /// What the last input stage answered.
    outcome: InputOutcome,
    /// An input stage ran since the last record pass, as `Session` holds it.
    input_ran: bool,
    /// The last frame that recorded.
    recorded_at: Option<u64>,
}

impl<'a> CanvasFrame<'a> {
    fn new(
        graph_ui: &'a mut GraphUi,
        ctx: &'a mut GraphCtxFixture,
        applies: AppliesIntents,
    ) -> Self {
        Self {
            graph_ui,
            ctx,
            applies,
            out: Requests::default(),
            outcome: InputOutcome::Record,
            input_ran: false,
            recorded_at: None,
        }
    }

    /// What the last frame raised. A frame that ran no record raised nothing a test can read.
    fn finish(self, ui: &Harness) -> Requests {
        assert_eq!(
            self.recorded_at,
            Some(ui.ui().render_frame_id()),
            "the last frame ran no record pass",
        );
        self.out
    }

    fn apply(&mut self) {
        if self.applies == AppliesIntents::Yes {
            let facts = self
                .ctx
                .open
                .drain_requests(self.out.document(), &self.ctx.library);
            self.out.owe_relayout_after(facts);
        }
    }
}

impl App for CanvasFrame<'_> {
    fn update(&mut self, _: WindowToken, mut update_ctx: UpdateCtx<'_>) -> InputOutcome {
        self.out = Requests::default();
        let graph_ctx = self.ctx.graph_ctx_on(update_ctx.display());
        self.outcome = self
            .graph_ui
            .prepass(&mut update_ctx, graph_ctx, &mut self.out);
        self.apply();
        self.input_ran = true;
        self.outcome
    }

    fn record(&mut self, _: WindowToken, recorder: &mut Ui) {
        let first_pass = mem::take(&mut self.input_ran);
        let mut later_pass = Requests::default();
        if !first_pass {
            let graph_ctx = self.ctx.graph_ctx_on(recorder.display());
            self.graph_ui.settle(recorder, graph_ctx, &mut later_pass);
        }
        let out = if first_pass {
            &mut self.out
        } else {
            &mut later_pass
        };
        draw(self.graph_ui, self.ctx, recorder, out);
        out.spend_relayout(recorder);
        self.recorded_at = Some(recorder.render_frame_id());
    }
}

/// The canvas's record half, inside the pane a dock gives it.
fn draw(graph_ui: &mut GraphUi, ctx: &GraphCtxFixture, recorder: &mut Ui, out: &mut Requests) {
    Panel::vstack()
        .id_salt("pane")
        .size(Sizing::FILL)
        .show(recorder, |ui| {
            graph_ui.draw(ui, ctx.graph_ctx_on(ui.display()), out);
        });
}
