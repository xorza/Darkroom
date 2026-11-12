//! The graph pane: the pan/zoom canvas, everything recorded on it, and the
//! toolbar pinned to its corner.
//!
//! Split by what each part does with a frame: [`scene`] is what a pass reads, [`gesture`]
//! turns input into intents, [`draw`] is what the record pass draws between the node bodies,
//! and [`node_ui`] is the body itself. [`canvas`] holds the two nested canvases' ids and the
//! coordinate space between them, which every one of those needs.
//!
//! This file is [`GraphUi`]: the state a graph pane keeps across frames, the two phases that
//! drive it ([`GraphUi::input`] and [`GraphUi::record`]), and the cache sweep
//! ([`GraphUi::retain_nodes`]) that runs outside them.

pub(crate) mod canvas;
pub(crate) mod canvas_bg;
pub(crate) mod canvas_ctx;
pub(crate) mod canvas_input;
pub(crate) mod draw;
pub(crate) mod draw_ctx;
pub(crate) mod gesture;
pub(crate) mod node_ui;
pub(crate) mod scene;
pub(crate) mod selection;
pub(crate) mod toolbar;
pub(crate) mod unsettled_ctx;
pub(crate) mod wid;

use palantir::{Chrome, Configure, Panel, Sense, Sizing, TranslateScale, Ui};

use crate::core::document::Document;
use crate::gui::frame::framing::Framing;
use crate::gui::frame::requests::Requests;
use crate::gui::frame::stage::Stage;
use crate::gui::graph_ctx::GraphCtx;
use crate::gui::pane::graph_ui::canvas_bg::CanvasBg;
use crate::gui::pane::graph_ui::canvas_ctx::CanvasCtx;
use crate::gui::pane::graph_ui::draw::wire::{WireEmphasis, WirePass, WireSet};
use crate::gui::pane::graph_ui::draw_ctx::DrawCtx;
use crate::gui::pane::graph_ui::gesture::gestures::Gestures;
use crate::gui::pane::graph_ui::gesture::shortcuts;
use crate::gui::pane::graph_ui::gesture::tie_ui::tie_widget::TiePass;
use crate::gui::pane::graph_ui::node_ui::{NodePass, NodeUi};
use crate::gui::pane::graph_ui::scene::canvas_geometry::CanvasGeometry;
use crate::gui::pane::graph_ui::scene::cull_region::CullRegion;
use crate::gui::pane::graph_ui::scene::tie_layout::{Span, TieLayout};
use crate::gui::pane::graph_ui::selection::Selection;
use crate::gui::pane::graph_ui::unsettled_ctx::UnsettledCtx;

/// The graph pane's UI state: the cross-frame geometry, the `NodeUi` that
/// renders graph nodes, the inspection panels, and the in-flight gesture
/// controllers.
///
/// The document has one graph tab, so one pane shows it and one instance
/// drives it: there is one pointer, so one drag, one rubber band, one open
/// popup. The canvas widget ids are fixed for the same reason.
///
/// Nothing here builds the [`GraphCtx`] it reads: `MainWindow` composes one per frame phase
/// and threads it down, and the "is the graph pane up" question is settled once at the tab
/// dispatch. [`Self::input`] turns this frame's input into intents before the record, and
/// [`Self::record`] records the pane from the dock's content closure.
///
/// **Bare-canvas gesture arbitration.**
/// [`CanvasGesture::classify`](canvas_input::CanvasGesture::classify) reads the outer canvas's
/// response ([`canvas::outer_wid`]) and modifiers and resolves which gesture latches this
/// frame into a single [`CanvasGesture`](canvas_input::CanvasGesture). `input` resolves it
/// once per frame, and each controller consumes only its own variant, so the precedence lives
/// in one match. Scroll and pinch aren't latch gestures (they coexist), so they stay inside
/// `CameraGesture::input` regardless of the classification.
///
/// Node panels and port circles live in the *inner* canvas and hit-test first, so a gesture
/// only reaches the bare canvas when it missed every node/port.
#[derive(Default, Debug)]
pub(crate) struct GraphUi {
    /// The *record* frame this canvas last ran [`Self::input`] on, or `None` before its first.
    ///
    /// The input stage runs only while a pane is showing the graph, so a gap here means the
    /// canvas was away, with nothing running on its behalf meanwhile. A stamp rather than a
    /// cached `bool`, so there is no second spelling of visibility to keep in step with the
    /// active tabs.
    ///
    /// [`Ui::record_frame_id`] and not [`Ui::render_frame_id`]: the latter counts painted
    /// frames too, and a paint-only frame (an idle caret blink) runs no record pass, so it
    /// would open a gap while the canvas never went anywhere and drop every in-flight gesture.
    last_input_frame: Option<u64>,
    bg: CanvasBg,
    /// Port centers, node sizes and world rects, cached across frames and rebuilt in
    /// [`Self::input`]. Outlives the scene on purpose (a culled node's ports still resolve off
    /// it), so it is swept by [`Self::retain_nodes`], not by absence from a projection.
    ///
    /// Private: every production reader reaches it through a [`CanvasCtx`] or a [`DrawCtx`]. Tests
    /// read it through `internals`' `GraphUi::geometry`.
    geometry: CanvasGeometry,
    /// This frame's wire ties: each band and each tied wire's pieces, rebuilt at the top of
    /// [`Self::record_canvas`].
    ties: TieLayout,
    /// The body of each node. No gesture: what it keeps is focus hysteresis and scratch.
    node_ui: NodeUi,
    /// The controllers in flight, each dropped by [`Self::open_pass`] on a tab switch.
    pub(crate) gestures: Gestures,
    /// A framing a surface outside the canvas asked for — the status bar's link, a menu —
    /// raised as a camera move in the next input stage, which has the settled geometry it needs.
    framing: Option<Framing>,
}

impl GraphUi {
    /// Raise `framing` as a camera move in the next input stage.
    pub(crate) const fn request_framing(&mut self, framing: Framing) {
        self.framing = Some(framing);
    }

    /// Evict this canvas's `NodeId`-keyed cache, the cross-frame geometry, down to the nodes the
    /// document still holds; see [`CanvasGeometry::retain_nodes`].
    pub(crate) fn retain_nodes(&mut self, document: &Document) {
        self.geometry
            .retain_nodes(|node_id| document.holds_node(node_id));
    }

    /// Whether this input stage is the canvas's first after being away, or its first ever: the
    /// edge that owes a relayout, since a canvas that has never recorded has no cached geometry
    /// to draw its first frame from, and a dock op raises no edit fact of its own.
    ///
    /// Two cases both mean *still here* and must not read as an appearance: the previous frame
    /// (the steady state) and *this* frame, as the input stage and a settle of one frame read
    /// the same [`Ui::record_frame_id`]. Only a gap wider than that is an absence.
    fn appearing(&mut self, frame: u64) -> bool {
        let appearing = self
            .last_input_frame
            .is_none_or(|last| last.saturating_add(1) < frame);
        self.last_input_frame = Some(frame);
        appearing
    }

    pub(crate) fn input(&mut self, ui: &mut Ui, stage: &mut Stage<'_, GraphCtx<'_>>) {
        let graph_ctx = stage.ctx;
        self.open_pass(ui.record_frame_id(), stage.out);
        // One classification for the whole frame. A controller with nothing in flight has nothing
        // to cancel, so each applies this to its own state slot unconditionally and lets its
        // existing "no gesture" path do the rest.
        let input = self.gestures.read_input(ui);
        let unsettled = UnsettledCtx::new(graph_ctx, input);
        self.gestures
            .before_geometry(ui, &mut stage.with(unsettled));
        // One walk, filling the geometry caches off the per-node and per-port responses.
        self.geometry.rebuild(ui, graph_ctx);
        self.gestures.input(
            ui,
            &mut stage.with(CanvasCtx::new(unsettled, &self.geometry)),
        );
        // Taking the table back `&mut` is what ends the stage above: nothing below may read it.
        self.finish_geometry(ui, stage);
        // The keyboard half of the same phase. Last, so a chord reads the
        // document the pointer gestures above were raised against.
        shortcuts::emit(
            ui,
            &mut stage.with(CanvasCtx::new(unsettled, &self.geometry)),
        );
    }

    /// Ready the canvas for a record pass that no input stage ran just before: the one past the
    /// first frame's warmup, and a second pass, each of which reads the rects of the pass before it.
    /// The geometry is built again, and a framing the last record raised lands here.
    /// Refused: a replay of the input stage, which ran each gesture again on a level read.
    pub(crate) fn settle(&mut self, ui: &Ui, stage: &mut Stage<'_, GraphCtx<'_>>) {
        self.open_pass(ui.record_frame_id(), stage.out);
        self.geometry.rebuild(ui, stage.ctx);
        self.finish_geometry(ui, stage);
    }

    /// What opens a pass on any path: a canvas back from being away drops the gesture state it left
    /// latched, since a drag still held would otherwise resume under a pointer that has long since
    /// moved on, and owes a relayout.
    fn open_pass(&mut self, frame: u64, out: &mut Requests) {
        if self.appearing(frame) {
            self.gestures.reset();
            self.node_ui.reset();
            out.owe_relayout();
        }
    }

    /// What follows a rebuilt geometry on any path: the snapped ports read as hovered, a pending
    /// framing lands on the rects it now holds, and a node with no measured size owes a relayout. It
    /// records hidden until its rects let its wires draw, and a cull keeps it recorded, so this ends
    /// with the next record.
    fn finish_geometry(&mut self, ui: &Ui, stage: &mut Stage<'_, GraphCtx<'_>>) {
        if self.geometry.has_unmeasured() {
            stage.out.owe_relayout();
        }
        // Both wire gestures have settled their snap targets, and the flags this writes are
        // document-unique, so the draw reads finished geometry.
        self.gestures.wire_ui.bake_snap_hover(&mut self.geometry);
        self.gestures
            .subscription_ui
            .bake_snap_hover(&mut self.geometry);
        if let Some(framing) = self.framing.take() {
            let moved = framing.intent(ui, stage.ctx, &self.geometry);
            stage.out.extend_graph(moved);
        }
    }

    /// Record one graph pane: its popups, the canvas itself, and the run/cancel toolbar over it.
    /// Called once per visible graph tab from the dock's content closure, so everything here is
    /// scoped to `graph_ctx` — the canvas widget ids included.
    ///
    /// Anything the pane asks for — graph edits, and the commands a chip or a
    /// menu pick means — lands on the stage's requests in the order it was raised.
    pub(crate) fn record(&mut self, ui: &mut Ui, stage: &mut Stage<'_, GraphCtx<'_>>) {
        self.record_popups(ui, stage);
        // The toolbar overlays the canvas's top-left corner, so the two share a stack. It
        // records *second*, which puts it above the canvas in the hit-test: a click on a chip
        // never starts a pan.
        Panel::zstack()
            .id_salt("graph_overlay")
            .size(Sizing::FILL)
            .show(ui, |ui| {
                self.record_canvas(ui, stage);
                toolbar::show(ui, stage);
            });
    }

    /// The palette and the menus, which read the clicks on their rows as they record.
    fn record_popups(&mut self, ui: &mut Ui, stage: &mut Stage<'_, GraphCtx<'_>>) {
        let gestures = &mut self.gestures;
        // A wire released over empty canvas opened the new-node popup, and a pick re-floats it.
        if let Some(source) = gestures.new_node_ui.record(ui, stage) {
            gestures.wire_ui.resume(source, stage.ctx);
        }
        // The menu records every frame whatever comes of it, as its popup's lifecycle depends on
        // it. Everything a pick means goes onto the requests, so a frame with both a menu pick
        // and a chip click raises both.
        gestures.node_menu.record(ui, stage);
        gestures.tie_menu.record(ui, stage, &self.ties);
    }

    /// The record pass's drawing half: the outer (pan-capture) canvas, the dotted backdrop,
    /// and, under the inner canvas's pan/zoom transform, the wires, node bodies and in-flight
    /// gesture previews.
    fn record_canvas(&mut self, ui: &mut Ui, stage: &mut Stage<'_, GraphCtx<'_>>) {
        let graph_ctx = stage.ctx;
        self.geometry.reposition(graph_ctx.view());
        let geometry = &self.geometry;
        self.ties
            .rebuild(graph_ctx.view(), |wire| Span::of(geometry, wire));
        let geometry = &self.geometry;
        let theme = graph_ctx.theme();
        let viewport = graph_ctx.camera();
        // The selection to paint: the live rubber-band preview while a band is in flight, else
        // the committed set. The preview stays out of the document, so no edit is recorded.
        let gestures = &mut self.gestures;
        let selected = gestures.selection_ui.preview().map_or_else(
            || Selection::Committed(graph_ctx.selected()),
            Selection::swept,
        );

        // Outer canvas: covers the whole pane, paints the canvas background, owns the input
        // routing for the bare canvas. Senses: `DRAG` is the middle-button pan (left-drag is
        // the rubber band), `SCROLL` is pan (zoom about the cursor under the command key),
        // `PINCH` is zoom about the cursor. Node panels hit-test first, in the inner canvas.
        //
        // `.clip_rect()` pins the inner-canvas subtree's `paint_rect`s to the outer rect even
        // when the inner transform zooms them past the viewport: without it, at high zoom one
        // off-screen node panel's screen rect can dwarf the surface and trip `Damage::Full`
        // every pan/zoom tick.
        Panel::canvas()
            .id(canvas::outer_wid())
            .size(Sizing::FILL)
            .sense(Sense::CLICK | Sense::DRAG | Sense::SCROLL | Sense::PINCH)
            .clip_rect()
            .chrome(Chrome::new(theme.canvas.bg))
            .show(ui, |ui| {
                // Dotted backdrop in screen space, beneath the inner canvas.
                self.bg.draw(ui, theme, viewport);
                Panel::canvas()
                    .id(canvas::inner_wid())
                    .size(Sizing::FILL)
                    .transform(TranslateScale::new(viewport.pan, viewport.zoom))
                    .show(ui, |ui| {
                        let cull = CullRegion::from_size(
                            ui.response_for(canvas::outer_wid())
                                .layout_rect
                                .map(|rect| rect.size),
                            &viewport,
                        );
                        // Built outside the probe scope so both passes read the same refs.
                        let draw_ctx = DrawCtx {
                            graph_ctx,
                            geometry,
                            selected,
                            cull,
                        };
                        // Painted first, beneath the wires and node bodies.
                        gestures.selection_ui.record(ui, draw_ctx);
                        {
                            let mut probe = gestures.breaker_ui.probe();
                            let mut tie_line = gestures.tie_ui.probe();
                            // One emphasis resolution for both wire families: any wire gesture
                            // (either drag controller, a breaker scribble or a tie line) fades
                            // the standing set.
                            let set = if gestures.wire_ui.in_flight()
                                || gestures.subscription_ui.in_flight()
                                || probe.is_active()
                                || tie_line.is_active()
                            {
                                WireSet::Faded
                            } else {
                                WireSet::Standing
                            };
                            let emphasis = WireEmphasis::resolve(theme.canvas.bg, set);
                            // Both wire renderers share these inputs. Subscription wires sit
                            // under the node bodies like data wires and share the breaker
                            // probe, so they are all cuttable.
                            let mut wires = WirePass {
                                draw_ctx,
                                probe: &mut probe,
                                tie_line: &mut tie_line,
                                lit: gestures.tie_menu.lit(),
                                emphasis: &emphasis,
                                ties: &self.ties,
                            };
                            wires.draw_data_wires(ui);
                            wires.draw_event_wires(ui);
                            // Over the wires they gather, under the node
                            // bodies, which cover a band as they cover wires
                            // and win a press over it.
                            TiePass {
                                draw_ctx,
                                ties: &self.ties,
                                line: &mut tie_line,
                            }
                            .record(ui);
                            // Back-to-front by each placement's depth (`GraphView::paint_order`),
                            // so a clicked node raises above its neighbours.
                            self.node_ui.draw_all(
                                ui,
                                &mut NodePass {
                                    stage: stage.with(draw_ctx),
                                    probe: &mut probe,
                                },
                            );
                        }
                        gestures.breaker_ui.record(ui, draw_ctx);
                        gestures.tie_ui.record(ui, draw_ctx);
                        gestures.wire_ui.record(ui, draw_ctx);
                        gestures.subscription_ui.record(ui, draw_ctx);
                    });
            });
    }
}

#[cfg(test)]
pub(crate) mod internals;

#[cfg(test)]
mod tests {

    use glam::UVec2;
    use palantir::Harness;

    use super::*;
    use crate::core::edit::graph_intent::GraphIntent;

    /// What a frame's input stage takes: a body's click selects, its right click opens the menu, a
    /// bare click selects nothing, a right click there opens the palette, and a click past an open
    /// menu closes it.
    #[test]
    fn the_stage_acts_on_a_click_by_where_it_lands() {
        use glam::Vec2;
        use palantir::PointerButton;
        use scenarium::NodeId;

        use crate::core::document::internals::DocumentFixture;
        use crate::gui::frame::shortcuts;
        use crate::gui::pane::graph_ui::internals::CanvasHarness;
        use crate::gui::port_ref::PortRef;

        const BARE: Vec2 = Vec2::new(600.0, 600.0);

        // The release frame of a click at `at`, past the press frame, which answers `Record`.
        fn click(
            h: &mut CanvasHarness,
            at: impl Into<palantir::Locator>,
            button: PointerButton,
        ) -> Vec<GraphIntent> {
            h.advance_past_double_click();
            h.ui.press(at, button);
            h.frame();
            h.ui.release(button);
            h.frame()
        }
        let fresh = || {
            let mut h = CanvasHarness::new(DocumentFixture::probes(2));
            h.prime(2);
            let node: NodeId = h.node(0);
            (h, node)
        };

        let (mut h, node) = fresh();
        let intents = click(&mut h, wid::body(node), PointerButton::Left);
        assert!(
            matches!(intents[0], GraphIntent::SetSelection { .. }),
            "{intents:?}"
        );

        let (mut h, node) = fresh();
        click(&mut h, wid::body(node), PointerButton::Right);
        assert!(
            h.graph_ui.gestures.node_menu.in_flight(),
            "the menu latched"
        );

        let (mut h, _) = fresh();
        let intents = click(&mut h, BARE, PointerButton::Left);
        assert!(intents.is_empty(), "nothing selected, nothing to clear");

        let (mut h, _) = fresh();
        click(&mut h, BARE, PointerButton::Right);
        assert!(h.graph_ui.gestures.new_node_ui.in_flight());

        let (mut h, node) = fresh();
        click(&mut h, wid::rename(node), PointerButton::Left);

        let (mut h, node) = fresh();
        h.ui.press(wid::body(node), PointerButton::Left);
        h.frame();
        h.ui.release(PointerButton::Left);
        h.ui.key(shortcuts::DELETE);
        h.frame();

        // The press would end a floating wire, so the wire floats from past the press.
        let (mut h, node) = fresh();
        h.ui.press(wid::body(node), PointerButton::Left);
        h.frame();
        let start = PortRef::output(h.node(1), 0);
        let CanvasHarness { graph_ui, ctx, .. } = &mut h;
        graph_ui.gestures.wire_ui.resume(start, ctx.graph_ctx());
        h.ui.release(PointerButton::Left);
        h.frame();

        let (mut h, node) = fresh();
        let from = h.node_center(node);
        h.ui.press(from, PointerButton::Left);
        h.frame();
        h.ui.drag_to(from + Vec2::new(40.0, 0.0));
        h.frame();
        h.ui.release(PointerButton::Left);
        h.frame();

        let (mut h, _) = fresh();
        h.ui.press(BARE, PointerButton::Left);
        h.frame();
        h.ui.drag_to(BARE + Vec2::new(40.0, 0.0));
        h.frame();
        h.ui.release(PointerButton::Left);
        h.frame();

        let (mut h, node) = fresh();
        click(&mut h, wid::body(node), PointerButton::Right);
        click(&mut h, BARE, PointerButton::Left);
        assert!(
            !h.graph_ui.gestures.node_menu.in_flight(),
            "the menu closed"
        );
    }

    /// A settle lands a framing the record raised: a reset from zoom 2 moves the camera, once.
    #[test]
    fn a_settle_lands_a_framing_once() {
        use crate::core::document::internals::DocumentFixture;
        use crate::core::edit::document_request::DocumentRequest;
        use crate::gui::frame::framing::Framing;
        use crate::gui::pane::graph_ui::internals::CanvasHarness;

        let mut h = CanvasHarness::new(DocumentFixture::probes(1));
        h.document_mut().main_view.viewport.zoom = 2.0;
        h.prime(2);
        h.graph_ui.request_framing(Framing::Reset);

        let settle = |h: &mut CanvasHarness| {
            let mut out = Requests::default();
            let ui = h.ui.ui_mut();
            let ctx = h.ctx.graph_ctx_on(ui.display());
            h.graph_ui.settle(ui, &mut Stage { ctx, out: &mut out });
            assert!(!out.owes_relayout());
            out
        };
        let mut out = settle(&mut h);
        let requests: Vec<_> = out.document().drain().collect();
        assert!(matches!(
            requests[..],
            [DocumentRequest::Graph(GraphIntent::SetViewport { .. })],
        ));
        let mut again = settle(&mut h);
        assert_eq!(
            again.document().drain().count(),
            0,
            "the framing lands once"
        );
    }

    /// The input stage runs only while the pane shows the graph, so the frame
    /// stamp — not a visibility flag — is what tells the canvas it was away.
    ///
    /// Three cases must read as *still here*, and only a gap as an
    /// appearance. The repeat is the trap: a frame that owes a relayout
    /// records twice under one frame id, and a second call reading as an
    /// appearance would reset a drag mid-gesture.
    #[test]
    fn appearing_is_a_frame_gap_not_a_repeat_or_a_step() {
        let mut h = Harness::builder(UVec2::ONE).mono().build();
        let mut graph_ui = GraphUi::default();

        assert!(
            graph_ui.appearing(h.ui().record_frame_id()),
            "a canvas that has never recorded is appearing"
        );
        assert!(
            !graph_ui.appearing(h.ui().record_frame_id()),
            "a second pass of the same frame is the same appearance, not a new one"
        );

        h.frame(|_| {});
        assert!(
            !graph_ui.appearing(h.ui().record_frame_id()),
            "the next consecutive frame is the steady state"
        );

        // Two frames the canvas sat out — the dock showed another tab.
        h.frame(|_| {});
        h.frame(|_| {});
        assert!(
            graph_ui.appearing(h.ui().record_frame_id()),
            "a gap means it was away and is back"
        );
        assert!(
            !graph_ui.appearing(h.ui().record_frame_id()),
            "and the reappearance is reported once, not per pass"
        );
    }
}
