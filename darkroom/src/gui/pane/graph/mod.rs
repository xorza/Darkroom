//! The graph pane: the pan/zoom canvas, everything recorded on it, and the
//! toolbar pinned to its corner.
//!
//! Split by what each part does with a frame rather than by what it draws:
//! [`frame`] is what a pass reads, [`gesture`] is what turns input into
//! intents, [`paint`] is what the record pass draws between the node bodies,
//! and [`node`] is the body itself. [`canvas`] holds the two nested canvases'
//! ids and the coordinate space between them — the one thing every one of
//! those needs.
//!
//! This file is [`GraphUI`] and nothing else: the state a graph pane keeps
//! across frames, the two phases that drive it ([`GraphUI::prepass`] and
//! [`GraphUI::draw`]), and the cache sweep ([`GraphUI::retain_nodes`]) that
//! runs outside them.

pub(crate) mod background;
pub(crate) mod canvas;
pub(crate) mod ctx;
pub(crate) mod frame;
pub(crate) mod gesture;
pub(crate) mod node;
pub(crate) mod paint;
pub(crate) mod toolbar;

use glam::Vec2;
use palantir::{Background, Configure, Panel, Sense, Sizing, TranslateScale, Ui};

use crate::core::document::Document;
use crate::core::edit::intent::types::GraphIntent;
use crate::gui::graph_ctx::GraphCtx;
use crate::gui::pane::graph::background::CanvasBackground;
use crate::gui::pane::graph::canvas::{inner_canvas_widget_id, outer_canvas_widget_id};
use crate::gui::pane::graph::ctx::{CanvasCtx, DrawCtx, Selection};
use crate::gui::pane::graph::frame::cull::CullRegion;
use crate::gui::pane::graph::frame::geometry::CanvasGeometry;
use crate::gui::pane::graph::gesture::breaker::BreakerUI;
use crate::gui::pane::graph::gesture::canvas_gesture::{CanvasGesture, classify_canvas_gesture};
use crate::gui::pane::graph::gesture::connection::ConnectionUI;
use crate::gui::pane::graph::gesture::new_node::NewNodeUi;
use crate::gui::pane::graph::gesture::node_menu::NodeMenuUi;
use crate::gui::pane::graph::gesture::preview_drag::PreviewDrag;
use crate::gui::pane::graph::gesture::selection::SelectionUI;
use crate::gui::pane::graph::gesture::slot::GestureSlot;
use crate::gui::pane::graph::gesture::subscription::SubscriptionUI;
use crate::gui::pane::graph::gesture::{connection, pan_zoom, shortcuts, subscription};
use crate::gui::pane::graph::node::{NodeDrawOutcome, NodeUI};
use crate::gui::pane::graph::paint::inspector::Inspectors;
use crate::gui::pane::graph::paint::wire::{WireEmphasis, WirePass};
use crate::gui::relayout::Relayout;
use crate::gui::requests::Requests;

/// Canvas-level UI state, shared by **every** graph pane on screen: the
/// port-widget-id cache, the `NodeUI` that renders graph nodes, the
/// inspection panels, and the in-flight gesture controllers.
///
/// One instance drives N canvases rather than one per pane, because
/// everything here is either keyed by a document-unique id (geometry,
/// inspectors) or inherently singular (there is one pointer, so one drag,
/// one rubber band, one open popup). What *is* per-pane — the canvas
/// widget ids, the viewport, the paint stack — comes from the [`GraphCtx`]
/// each entry point is handed.
///
/// Nothing here builds that context: `MainWindow` composes one per frame phase
/// and threads it down, so this type never needs to name a theme, a library
/// or a run state — the context answers for all three — and the "is a graph
/// pane up" question is settled once at the tab dispatch rather than
/// re-derived by every pass.
///
/// The frame splits accordingly:
/// - [`Self::prepass`] runs **once** over the whole scene, with a small
///   per-pane loop inside for the viewport-dependent parts.
/// - [`Self::draw`] runs **once per visible graph pane**, from the dock's
///   content closure.
///
/// **Bare-canvas gesture arbitration.** [`classify_canvas_gesture`] reads
/// one pane's outer-canvas response
/// ([`outer_canvas_widget_id`](canvas::outer_canvas_widget_id)) +
/// modifiers and resolves which gesture latches this frame into a single
/// [`CanvasGesture`]. `prepass` resolves it once per frame and parks the
/// winner (with its pane) in [`Self::gesture`]; each sub-controller is
/// handed that classification and consumes only its own variant, so
/// there's no hand-kept disjointness across files — the precedence lives
/// in one match. Wheel/pinch zoom isn't a latch gesture (it coexists), so
/// it stays inside `emit_pan_zoom` regardless of the classification.
///
/// Node panels and port circles live in the *inner* canvas and hit-test
/// first, so a gesture only reaches the bare canvas (and this
/// classification) when it missed every node/port.
#[derive(Default, Debug)]
pub(crate) struct GraphUI {
    /// The frame this canvas last ran [`Self::prepass`] on, or `None` before
    /// its first ever one.
    ///
    /// The prepass runs only while a pane is showing the graph, so a *gap*
    /// here means the canvas was away — which is how it notices its own
    /// reappearance without anything having to run on its behalf while it was
    /// gone. A stamp rather than a cached `bool` copy of
    /// [`Document::shows_graph`], so there is no second spelling of visibility
    /// to keep in step with the first.
    last_prepass_frame: Option<u64>,
    background: CanvasBackground,
    /// Port centers, node sizes and world rects, cached across frames and
    /// rebuilt in [`Self::prepass`]. Outlives the scene on purpose — a culled
    /// node's ports still resolve off it — so it is swept by
    /// [`Self::retain_nodes`] rather than by absence from a projection.
    ///
    /// Private: every production reader reaches it through a [`CanvasCtx`],
    /// which is the only thing that pairs it with the gesture it was settled
    /// beside. Tests read it through [`internals::geometry`].
    geometry: CanvasGeometry,
    /// Open inspection panels, keyed by node. Outside the gesture group
    /// so pinned panels survive a tab switch; panels only paint for nodes
    /// in the active scene, so off-tab ones hide and reappear.
    inspectors: Inspectors,
    /// This frame's bare-canvas gesture and the pane it latched on,
    /// resolved once in [`Self::prepass`] and read back by [`Self::draw`].
    /// At most one pane can own a press, so one slot is enough.
    gesture: Option<CanvasGesture>,
    /// Whether this frame cancels whatever gesture is in flight (Esc).
    ///
    /// Resolved once in [`Self::prepass`], beside the classification —
    /// the counterpart to [`classify_canvas_gesture`], which says which
    /// gesture *starts*. Read there rather than per controller because
    /// the escape is one fact about the frame, and four controllers
    /// polling it separately is how they grew three different guards
    /// around it.
    cancelled: bool,
    // The in-flight gesture controllers, each dropped by
    // [`Self::reset_gestures`] on a tab switch. Flat rather than grouped
    // behind one field: replacing a group wholesale threw away buffers
    // several of them deliberately grow (the rubber band's swept set, the
    // breaker's point and broken-target vectors, the palette's folded search),
    // and each controller knows which of its own state is a buffer and which
    // is the gesture.
    node_ui: NodeUI,
    breaker_ui: BreakerUI,
    connection_ui: ConnectionUI,
    preview_drag: PreviewDrag,
    subscription_ui: SubscriptionUI,
    new_node_ui: NewNodeUi,
    node_menu: NodeMenuUi,
    selection_ui: SelectionUI,
    /// Viewport pan snapshot captured at the frame the active pan-drag
    /// latched, keyed by the pane that latched it. While the drag is
    /// active, that pane's `viewport.pan = anchor + drag_delta`. Input
    /// bookkeeping (lifetime = one gesture), not viewport state — and
    /// keyed because `emit_pan_zoom` runs once per visible pane, so the
    /// idle ones must not consume the live one's release edge.
    pan_anchor: GestureSlot<Vec2>,
}

impl GraphUI {
    /// Evict this canvas's two `NodeId`-keyed caches down to the nodes the
    /// document still holds — the cross-frame geometry and the open inspection
    /// panels.
    ///
    /// Both deliberately outlive the scene, so absence from it is no grounds
    /// on its own and neither can do this itself; see
    /// [`CanvasGeometry::retain_nodes`]. Swept together because `GraphUI` owns
    /// both, so a new one lands here rather than in a fourth call site.
    pub(crate) fn retain_nodes(&mut self, document: &Document) {
        self.geometry
            .retain_nodes(|node_id| document.holds_node(node_id));
        self.inspectors
            .retain_nodes(|node_id| document.holds_node(node_id));
    }

    /// Drop every in-flight gesture, keeping the buffers the controllers grow.
    ///
    /// Destructured rather than assigned wholesale so the compiler makes the
    /// call: a field added to [`GraphUI`] does not compile until it is either
    /// reset here or explicitly named as surviving. The `_`-bound arms below
    /// are the survivors — cross-frame caches and per-frame facts that the
    /// next frame overwrites anyway.
    fn reset_gestures(&mut self) {
        let Self {
            node_ui,
            breaker_ui,
            connection_ui,
            preview_drag,
            subscription_ui,
            new_node_ui,
            node_menu,
            selection_ui,
            pan_anchor,
            // Survivors: caches that outlive the scene on purpose, and the
            // frame-local facts `prepass` rewrites before anything reads them.
            background: _,
            geometry: _,
            inspectors: _,
            last_prepass_frame: _,
            gesture: _,
            cancelled: _,
        } = self;
        node_ui.reset();
        breaker_ui.reset();
        connection_ui.reset();
        preview_drag.reset();
        subscription_ui.reset();
        new_node_ui.reset();
        node_menu.reset();
        selection_ui.reset();
        pan_anchor.clear();
    }

    /// Take note of whether a pane is showing this canvas, and report whether
    /// that *changed* since last frame.
    ///
    /// Crossing the edge drops all in-flight gesture state and closes the
    /// transient inspection panels — both are tab-local, and a drag left
    /// latched while the canvas was away would otherwise resume when it
    /// comes back. Cross-frame caches survive, notably
    /// [`CanvasGeometry`]'s port-offset table, so connections still anchor on
    /// the first frame after a switch.
    ///
    /// Whether this prepass is the canvas's first after being away — the
    /// edge that owes a relayout, since a canvas that has never recorded has
    /// no cached geometry to draw its first frame from, and a dock op raises
    /// no geometry signal of its own
    /// (`UndoStep::invalidates_cached_geometry` is `false` for one).
    ///
    /// Reads the frame stamp rather than a visibility flag, so nothing has to
    /// run while the canvas is off screen for it to notice coming back. Two
    /// cases both mean *still here* and must not read as an appearance: the
    /// previous frame (the ordinary steady state) and *this* frame — a split
    /// view showing the graph in two panes runs this once per pane, since
    /// `DockLayout::active_tabs` yields one tab per group and never dedupes.
    /// Only a gap wider than that is an absence.
    fn appearing(&mut self, ui: &Ui) -> bool {
        let frame = ui.frame_id();
        let appearing = self
            .last_prepass_frame
            .is_none_or(|last| last.saturating_add(1) < frame);
        self.last_prepass_frame = Some(frame);
        appearing
    }

    pub(crate) fn prepass(
        &mut self,
        ui: &mut Ui,
        graph_ctx: GraphCtx<'_>,
        out: &mut Requests,
    ) -> Relayout {
        debug_assert!(
            graph_ctx.is_visible(),
            "the prepass is reached from an active Graph tab"
        );
        // First: a canvas back from being away drops the transient state it
        // left latched, since a drag still held would otherwise resume under
        // a pointer that has long since moved on.
        let appearing = self.appearing(ui);
        if appearing {
            self.reset_gestures();
            self.inspectors.close_unpinned();
        }
        // Resolve the frame's bare-canvas gesture and park it for `draw` to
        // read back — the classification is one response poll, and both
        // phases must agree on the winner.
        //
        // One read for the whole frame. A controller with nothing in
        // flight has nothing to cancel, so each applies this to its own
        // state slot unconditionally and lets its existing "no gesture"
        // path do the rest.
        self.cancelled = ui.escape_pressed();
        let gesture = classify_canvas_gesture(ui);
        self.gesture = gesture;
        pan_zoom::emit_pan_zoom(&mut self.pan_anchor, ui, graph_ctx, gesture, out);
        self.node_ui.prepass(ui, graph_ctx, out);
        // One walk, filling the geometry caches and the whole hit digest off
        // the same per-node and per-port responses.
        self.geometry.rebuild(ui, graph_ctx);
        // Everything below reads the settled geometry, so from here the canvas
        // has a context of its own.
        let cx = CanvasCtx::new(graph_ctx, &self.geometry, gesture, self.cancelled);
        // Both port-drag claimants sit *after* the rebuild so they read this
        // frame's drag edges and centers, and `preview_drag_modifier` keeps
        // them disjoint: the preview spawn takes the output column under the
        // chord, the wire gesture takes it otherwise.
        self.preview_drag.apply(ui, cx, out);
        // A node picked from a drop-spawned palette last frame re-floats its
        // wire so the user clicks the exact port to land it.
        let resume = self.new_node_ui.take_resume_floating();
        self.connection_ui.apply(ui, cx, resume, out);
        // Subscription wires (emitter → subscriber) latch/commit here, for
        // the same pre-record reasons as the connection gesture above; an
        // emitter glyph and a data port can't both latch (different widget-id
        // spaces).
        self.subscription_ui.apply(ui, cx, out);
        // Last, once: both wire gestures have settled their snap targets
        // above, and the flags this writes are document-unique, so the draw
        // reads finished geometry. Taking the table back `&mut` is what ends
        // `cx` — nothing below may read one.
        self.connection_ui.bake_snap_hover(&mut self.geometry);
        self.subscription_ui.bake_snap_hover(&mut self.geometry);
        // The keyboard half of the same phase. Last, so a chord reads the
        // document the pointer gestures above were raised against.
        shortcuts::emit(ui, graph_ctx, out);
        Relayout::needed_if(appearing)
    }

    /// Record one graph pane: its gestures' record-phase halves, the canvas
    /// itself, and the run/cancel toolbar over it. Called once per visible
    /// graph tab from the dock's content closure, so everything here is scoped
    /// to `graph_ctx` — the canvas widget ids included.
    ///
    /// Anything the pane asks for — graph edits, and the commands a chip or a
    /// menu pick means — lands on `out` in the order it was raised.
    pub(crate) fn draw(&mut self, ui: &mut Ui, graph_ctx: GraphCtx<'_>, out: &mut Requests) {
        debug_assert!(
            graph_ctx.is_visible(),
            "the record is reached from an active Graph tab"
        );
        // Pan/zoom was already folded into the document in `prepass`, and the
        // contexts built below read it straight off there, so the transform
        // sees the up-to-date viewport with nothing to re-sync. The gesture
        // and the Esc were resolved in `prepass` too, and every half below
        // composes the same context off them — so no two passes can disagree
        // about the frame they are drawing.
        self.resolve_gestures(ui, graph_ctx, out);
        // The toolbar overlays the canvas's top-left corner rather than
        // sitting beside it, so the two share a stack. It records *second*,
        // which is what puts it above the canvas in the hit-test: a click on a
        // chip never starts a pan. The stacking is the pane's own business —
        // the dock's content closure hands this method a plain `ui` and learns
        // nothing about the overlay.
        Panel::zstack()
            .id_salt("graph_overlay")
            .size((Sizing::FILL, Sizing::FILL))
            .show(ui, |ui| {
                self.record_canvas(ui, graph_ctx, out);
                // Composed here rather than shared with the halves above and
                // below: both drive controllers and so hold all of `self`
                // mutably, while a context borrows the geometry shared. The
                // toolbar only reads, so it takes one built once
                // `record_canvas` has given the fields back.
                let cx = CanvasCtx::new(graph_ctx, &self.geometry, self.gesture, self.cancelled);
                toolbar::show(ui, cx, out);
            });
    }

    /// The record pass's non-drawing half: each controller's record-phase
    /// `apply`, then the chip clicks that mean an [`AppCommand`]. Everything
    /// here reads last frame's responses and raises requests; nothing draws.
    fn resolve_gestures(&mut self, ui: &mut Ui, graph_ctx: GraphCtx<'_>, out: &mut Requests) {
        let cx = CanvasCtx::new(graph_ctx, &self.geometry, self.gesture, self.cancelled);
        // Click on bare canvas (node panels hit-test first, so this
        // only fires when the click missed every node) clears the
        // selection. Skip when nothing is selected so we don't pollute
        // the undo stack with no-op `SetSelection` entries every time
        // the user clicks the empty canvas. A *drag* on bare canvas is
        // the rubber band (classified as `Select`), not a `Deselect`.
        if cx.gesture() == Some(CanvasGesture::Deselect) && !cx.graph_ctx().selected().is_empty() {
            out.push_graph(GraphIntent::clear_selection());
        }
        // `CanvasGeometry` was already rebuilt in `prepass` against every
        // visible graph's scene — `App` rebuilds the scene *before* prepass
        // on the frame a tab becomes active, so prepass never sees a stale
        // graph, and the offset cache fills in port centers for nodes that
        // hadn't recorded yet. `cx` carries that same table; no second
        // rebuild needed.
        self.selection_ui.apply(ui, cx, out);
        self.breaker_ui.apply(ui, cx, out);
        // A connection released over empty canvas (detected in `prepass`)
        // opens the new-node popup; picking a node re-floats the wire. Only
        // the pane holding the dropped wire's source claims it.
        let pending_connection = self.connection_ui.take_pending_connection();
        // A right-click that just ended a floating wire shouldn't also open
        // the palette — suppress the `NewNode` gesture for this frame, by
        // handing the popup a context whose gesture slot is empty.
        let popup_cx = if self.connection_ui.ended_on_secondary() {
            cx.without_gesture()
        } else {
            cx
        };
        self.new_node_ui
            .apply(ui, popup_cx, pending_connection, out);
        // The menu records every frame whatever comes of it — its popup owns a
        // lifecycle that depends on it — and everything a pick means goes onto
        // `out`, so there is no precedence to settle here: a frame in which
        // both a menu pick and a chip click landed raises both.
        self.node_menu.apply(ui, graph_ctx, out);
    }

    /// The record pass's drawing half: the outer (pan-capture) canvas, the
    /// dotted backdrop, and — under the inner canvas's pan/zoom transform —
    /// the wires, node bodies, inspection panels, and in-flight
    /// gesture previews.
    fn record_canvas(&mut self, ui: &mut Ui, graph_ctx: GraphCtx<'_>, out: &mut Requests) {
        let cx = CanvasCtx::new(graph_ctx, &self.geometry, self.gesture, self.cancelled);
        let theme = cx.theme();
        let viewport = graph_ctx.viewport();
        let (pan_val, zoom_val) = (viewport.pan, viewport.zoom);
        // Effective selection to paint: the live rubber-band preview while
        // a band is in flight over *this* pane, else its committed set.
        // The preview is kept out of the document, so a band in flight
        // changes what paints without recording an edit.
        let selected = self
            .selection_ui
            .preview()
            .map_or(Selection::Committed(graph_ctx.selected()), Selection::swept);

        // What the node draw sees but cannot act on: the inspect chip and the
        // body clicks, both of which drive `Inspectors` — held shared below so
        // the panels can paint, and taken `&mut` once the draw is over.
        let mut outcome = NodeDrawOutcome::default();

        // Outer canvas: covers the whole pane, paints the canvas
        // background, owns the input routing for empty-canvas
        //  Senses:
        // - `DRAG`: middle-button canvas pan (graph-editor
        //   convention; left-drag is reserved for rubber-band
        //   selection once that lands). Pulled via
        //   `Ui::drag_delta_by(.., PointerButton::Middle)`, since the
        //   left-only `ResponseState::drag_delta` doesn't carry middle.
        // - `SCROLL`: mouse wheel / touchpad swipe = zoom-about-cursor.
        // - `PINCH`: touchpad pinch = zoom-about-cursor.
        // Node panels (descendants of the *inner* canvas, which
        // carries the pan/zoom transform) hit-test first; only bare
        // canvas falls through to the outer's senses.
        //
        // `.clip_rect()` pins the inner-canvas subtree's `paint_rect`s
        // to the outer rect even when the inner transform zooms them
        // way past the viewport. Without it, at high zoom a single
        // off-screen node panel's screen rect can dwarf the surface,
        // damage threshold sees ratio ≫ 1 and trips `Damage::Full`
        // every pan/zoom tick.
        Panel::canvas()
            .id(outer_canvas_widget_id())
            .size((Sizing::FILL, Sizing::FILL))
            .sense(Sense::CLICK | Sense::DRAG | Sense::SCROLL | Sense::PINCH)
            .clip_rect()
            .background(Background::fill(theme.canvas.bg))
            .show(ui, |ui| {
                // Dotted backdrop in screen space, beneath the inner
                // (transformed) canvas — so it paints under everything.
                self.background.draw(ui, theme, pan_val, zoom_val);
                Panel::canvas()
                    .id(inner_canvas_widget_id())
                    .size((Sizing::FILL, Sizing::FILL))
                    .transform(TranslateScale::new(pan_val, zoom_val))
                    .show(ui, |ui| {
                        // Inner canvas's pre-transform origin. Shapes
                        // and child node panels recorded inside this
                        // closure share the inner canvas's transform
                        // (palantir's `Panel::transform` applies to
                        // the body: child subtrees AND direct
                        // shapes), so port `layout_rect`s and bezier
                        // endpoints stay aligned at every zoom.
                        let canvas_origin = ui
                            .response_for(inner_canvas_widget_id())
                            .layout_rect
                            .map(|r| r.min)
                            .unwrap_or(Vec2::ZERO);
                        let cull = CullRegion::from_canvas(
                            ui.response_for(outer_canvas_widget_id()).layout_rect,
                            canvas_origin,
                            &viewport,
                        );
                        // Painted first so it sits beneath the
                        // connections and node bodies.
                        self.selection_ui.draw(ui, theme);
                        // One bundle for everything this pane records: the
                        // node bodies below, and the inspection
                        // panels after them. Built out here rather than inside
                        // the probe scope so both passes read the same refs.
                        let dcx = DrawCtx::new(cx, selected, &self.inspectors, cull);
                        {
                            let mut probe = self.breaker_ui.probe();
                            // One emphasis resolution for both wire families:
                            // any wire gesture — either drag controller or an
                            // active breaker scribble — fades the standing set.
                            // All three are scoped to this pane, so a gesture
                            // running on a neighbouring canvas leaves these
                            // wires at full strength.
                            let fading = self.connection_ui.is_dragging()
                                || self.subscription_ui.is_dragging()
                                || probe.is_active();
                            let emphasis = WireEmphasis::resolve(theme.canvas.bg, fading);
                            // Both wire renderers share these inputs, so
                            // they're bundled once and reborrowed into each.
                            // Subscription wires sit under the node bodies
                            // like data wires (drawn before `draw_all`), and
                            // share the breaker probe so they're all
                            // cuttable — one passing behind an unrelated node
                            // goes under it rather than drawing on top.
                            let mut wires = WirePass {
                                dcx,
                                probe: &mut probe,
                                emphasis: &emphasis,
                            };
                            connection::draw(ui, &mut wires);
                            subscription::draw(ui, &mut wires);
                            // Node bodies paint back-to-front by each
                            // placement's depth (`GraphView::paint_order`), so
                            // a clicked node raises above its neighbours.
                            outcome = self.node_ui.draw_all(ui, dcx, &mut probe, out);
                        }
                        // Inspection panels paint after the node bodies so
                        // they sit on top and win clicks over the nodes
                        // beneath; positioned in world coords, so they ride
                        // the inner-canvas transform.
                        self.inspectors.draw_panels(ui, dcx);
                        self.breaker_ui.draw(ui, theme);
                        self.connection_ui.draw_in_flight(ui, cx, canvas_origin);
                        self.subscription_ui.draw_in_flight(ui, cx, canvas_origin);
                    });
            });
        // The draw is over, so the panels are free to take `&mut`: cycle the
        // node whose chip was clicked, and close the unpinned ones if the
        // action landed anywhere but on a panel.
        self.inspectors.apply(ui, &outcome);
        if let Some(node) = outcome.menu_opened {
            self.node_menu.open_on(ui, graph_ctx, node, out);
        }
    }
}

#[cfg(test)]
pub(crate) mod internals {
    use super::*;

    impl GraphUI {
        /// The canvas's cross-frame geometry cache, for the tests that assert
        /// on what a record filled into it and what [`GraphUI::retain_nodes`]
        /// released — port centers, node sizes, cached world rects.
        ///
        /// Shared, not `&mut`: a test reads what the passes settled, it does
        /// not seed the cache by hand.
        pub(crate) fn geometry(&self) -> &CanvasGeometry {
            &self.geometry
        }
    }
}

#[cfg(test)]
pub(crate) mod harness;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::document::TabRef;
    use crate::core::document::dock::DockOp;
    use crate::core::document::harness::DocFixture;
    use crate::core::preview::preview_func;
    use crate::gui::pane::graph::harness::CanvasHarness;
    use crate::gui::pane::graph::node::preview_row::preview_image_wid;
    use crate::gui::state::preview_store::internals::opaque_image_value;

    /// Clicking a preview card's image asks the dock for that node's viewer
    /// tab — the canvas's one view-tier request, raised from the prepass off
    /// the hit digest the geometry rebuild fills.
    #[test]
    fn clicking_a_preview_card_asks_for_its_viewer_tab() {
        let mut fixture = DocFixture::default();
        let node = fixture.add(&preview_func(Default::default()));
        let mut h = CanvasHarness::new(fixture);
        // The run projection `App` would have filled from a completed run:
        // without a value the card records `Sense::NONE` and swallows the click.
        h.ctx
            .run_state
            .previews
            .ingest_preview(h.ui.ui(), node, opaque_image_value());
        h.prime(2);
        assert!(h.view_ops.is_empty(), "nothing asked for before the click");

        h.ui.click_on(preview_image_wid(node));
        let intents = h.frame();

        assert!(
            matches!(
                h.view_ops[..],
                [DockOp::OpenTab {
                    tab: TabRef::ImageViewer(clicked)
                }] if clicked == node
            ),
            "expected one OpenTab for {node:?}, got {:?}",
            h.view_ops
        );
        assert!(
            intents.is_empty(),
            "opening a viewer is navigation, not a graph edit: {intents:?}"
        );
    }

    /// The prepass runs once per pane showing the graph, so the frame stamp —
    /// not a visibility flag — is what tells the canvas it was away.
    ///
    /// Three cases must read as *still here*, and only a gap as an
    /// appearance. The split-view one is the trap: `DockLayout::active_tabs`
    /// yields one tab per group with no dedup, so two panes on the graph run
    /// this twice in a single frame, and a second call reading as an
    /// appearance would reset a drag mid-gesture.
    #[test]
    fn appearing_is_a_frame_gap_not_a_repeat_or_a_step() {
        let mut h = CanvasHarness::new(DocFixture::probes(1));
        let mut graph_ui = GraphUI::default();

        assert!(
            graph_ui.appearing(h.ui.ui()),
            "a canvas that has never recorded is appearing"
        );
        assert!(
            !graph_ui.appearing(h.ui.ui()),
            "a second pane on the same frame is the same appearance, not a new one"
        );

        h.ui.frame(|_| {});
        assert!(
            !graph_ui.appearing(h.ui.ui()),
            "the next consecutive frame is the steady state"
        );

        // Two frames the canvas sat out — the pane was on another tab.
        h.ui.frame(|_| {});
        h.ui.frame(|_| {});
        assert!(
            graph_ui.appearing(h.ui.ui()),
            "a gap means it was away and is back"
        );
        assert!(
            !graph_ui.appearing(h.ui.ui()),
            "and the reappearance is reported once, not per pane"
        );
    }
}
