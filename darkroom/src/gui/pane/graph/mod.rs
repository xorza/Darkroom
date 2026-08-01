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
//! across frames, and the three phases (`prepass`, `draw`, and the sweeps
//! between) that drive it.

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
use scenarium::NodeId;
use std::collections::BTreeSet;

use crate::core::document::Document;
use crate::core::edit::intent::types::GraphIntent;
use crate::gui::app::commands::AppCommand;
use crate::gui::app::commands::edit::EditCommand;
use crate::gui::app::commands::run::RunCommand;
use crate::gui::graph_ctx::GraphCtx;
use crate::gui::pane::graph::background::CanvasBackground;
use crate::gui::pane::graph::canvas::{inner_canvas_widget_id, outer_canvas_widget_id};
use crate::gui::pane::graph::ctx::{CanvasCtx, DrawCtx, Selection};
use crate::gui::pane::graph::frame::cull::CullRegion;
use crate::gui::pane::graph::frame::geometry::CanvasGeometry;
use crate::gui::pane::graph::frame::hits::{CanvasHits, Chip};
use crate::gui::pane::graph::frame::prepass::{emit_path_picks, emit_port_dblclicks};
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
use crate::gui::pane::graph::node::NodeUI;
use crate::gui::pane::graph::paint::inspector::Inspectors;
use crate::gui::pane::graph::paint::wire::{WireEmphasis, WirePass};
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
    /// Whether a pane showed this canvas last frame. Read back by
    /// [`Self::sync_visibility`] as the edge detector behind the transient
    /// reset — a question the current frame alone cannot answer.
    visible: bool,
    background: CanvasBackground,
    pub(crate) geometry: CanvasGeometry,
    /// Last frame's node interactions, swept once at the top of the frame
    /// and read by every pass below instead of each re-polling the same
    /// widget ids. Persistent for ownership only — [`CanvasHits::scan`]
    /// rewrites it whole.
    ///
    /// Swept by `MainWindow::scan_navigation`, *before* the scene is
    /// rebuilt, because the graph-open chip it collects has to resolve
    /// before the tab set settles — so it holds ids from last frame's
    /// projection, and every reader confirms the node is still in the pane
    /// it is drawing before acting.
    pub(crate) hits: CanvasHits,
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
    /// Record the run/cancel toolbar overlaying this canvas's top-left
    /// corner. Called from the dock's content closure right after
    /// [`Self::draw`], so it hit-tests above the canvas and a click on it
    /// never starts a pan.
    pub(crate) fn draw_toolbar(&self, ui: &mut Ui, graph_ctx: GraphCtx<'_>, out: &mut Requests) {
        toolbar::show(ui, self.canvas_ctx(graph_ctx), out);
    }

    /// Sweep last frame's node responses into [`CanvasHits`]. Does nothing
    /// when no pane is showing the graph — the sweep runs before the tab set
    /// settles, so it cannot assume one.
    pub(crate) fn scan_hits(&mut self, ui: &Ui, graph_ctx: GraphCtx<'_>) {
        self.hits.scan(ui, graph_ctx);
    }

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
            hits: _,
            inspectors: _,
            visible: _,
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
    /// The caller turns a `true` into a relayout request: a canvas that has
    /// never recorded has no cached geometry to draw its first frame from,
    /// and a dock op raises no geometry signal of its own
    /// (`UndoStep::invalidates_cached_geometry` is `false` for one).
    ///
    /// Unlike the per-frame reconciles beside it, this cannot simply run
    /// every frame: clearing gestures is only correct on the transition,
    /// since every gesture spans frames by definition.
    pub(crate) fn sync_visibility(&mut self, doc: &Document) -> bool {
        let visible = doc.shows_graph();
        if self.visible == visible {
            return false;
        }
        self.visible = visible;
        self.reset_gestures();
        self.inspectors.close_unpinned();
        true
    }

    /// Pre-record pass — see
    /// [`crate::gui::pane::graph::node::NodeUI::prepass`]. Every input-derived
    /// intent that can change layout is emitted here, *before* the
    /// record, so its effect is applied to `Document` by the pre-record
    /// drain and Pass A records the settled layout:
    ///
    /// - pan/zoom (`emit_pan_zoom` → `GraphIntent::SetViewport`),
    /// - node drag (`node_ui.prepass` → `GraphIntent::MoveSelection`),
    /// - connection commit (`connection_ui.apply` → `GraphIntent::SetInput`).
    ///
    /// Connection commit specifically *must* be here: binding an input
    /// that had a const value removes its inline editor and resizes the
    /// node. If committed during the record (post-record drain), Pass A
    /// records the pre-resize layout and the relayout's Pass B rebuilds
    /// `CanvasGeometry` from that stale cascade — the new connection floats
    /// to the old port. Committing pre-record makes `cascade_A` the
    /// resized layout, so Pass B anchors the curve correctly with no
    /// extra frame. `CanvasGeometry` is rebuilt here (and reused by
    /// [`Self::draw`]) because the commit reads it. Navigation (tab/open) is handled
    /// separately, before this, so the target is already fixed here.
    ///
    /// Runs **once** for the whole graph. The viewport-dependent half —
    /// the bare-canvas gesture classification and pan/zoom, which read one
    /// pane's outer-canvas response — loops the visible panes; everything
    /// else is keyed by document-unique ids and sweeps them all at once.
    pub(crate) fn prepass(&mut self, ui: &mut Ui, graph_ctx: GraphCtx<'_>, out: &mut Requests) {
        debug_assert!(
            graph_ctx.is_visible(),
            "the prepass is reached from an active Graph tab"
        );
        let Self {
            geometry,
            hits,
            inspectors,
            gesture: frame_gesture,
            cancelled,
            pan_anchor,
            node_ui,
            preview_drag,
            new_node_ui,
            connection_ui,
            subscription_ui,
            ..
        } = self;
        // Resolve the frame's bare-canvas gesture and park it for `draw` to
        // read back — the classification is one response poll, and both
        // phases must agree on the winner.
        //
        // One read for the whole frame. A controller with nothing in
        // flight has nothing to cancel, so each applies this to its own
        // state slot unconditionally and lets its existing "no gesture"
        // path do the rest.
        *cancelled = ui.escape_pressed();
        let gesture = classify_canvas_gesture(ui);
        *frame_gesture = gesture;
        pan_zoom::emit_pan_zoom(pan_anchor, ui, graph_ctx, gesture, out);
        node_ui.prepass(ui, graph_ctx, out);
        geometry.rebuild(ui, graph_ctx, hits);
        // Everything below reads the settled geometry and this frame's swept
        // hits, so from here the canvas has a context of its own.
        let cx = CanvasCtx::new(graph_ctx, geometry, hits, gesture, *cancelled);
        // After the rebuild, which is where the port half of `hits` fills:
        // a port double-click rides the same response read as that port's
        // center, so there is nothing to act on before it.
        emit_port_dblclicks(cx, out);
        // Both port-drag claimants sit *after* the rebuild so they read this
        // frame's drag edges and centers, and `preview_drag_modifier` keeps
        // them disjoint: the preview spawn takes the output column under the
        // chord, the wire gesture takes it otherwise.
        preview_drag.apply(ui, cx, out);
        // A node picked from a drop-spawned palette last frame re-floats its
        // wire so the user clicks the exact port to land it.
        let resume = new_node_ui.take_resume_floating();
        connection_ui.apply(ui, cx, resume, out);
        // Subscription wires (emitter → subscriber) latch/commit here, for
        // the same pre-record reasons as the connection gesture above; an
        // emitter glyph and a data port can't both latch (different widget-id
        // spaces).
        subscription_ui.apply(ui, cx, out);
        // Inspector chip toggles + the close-on-outside-action sweep, both
        // off this frame's swept hits.
        inspectors.apply(ui, cx);
        // Last, once: both wire gestures have settled their snap targets
        // above, and the flags this writes are document-unique, so the draw
        // reads finished geometry. Taking the table back `&mut` is what ends
        // `cx` — nothing below may read one.
        connection_ui.bake_snap_hover(geometry);
        subscription_ui.bake_snap_hover(geometry);
        // The keyboard half of the same phase. Last, so a chord reads the
        // document the pointer gestures above were raised against.
        shortcuts::emit(ui, graph_ctx, out);
    }

    /// Record one graph pane: its gestures' record-phase halves, then the
    /// canvas itself. Called once per visible graph tab from the dock's
    /// content closure, so everything here is scoped to `graph_ctx` — the
    /// canvas widget ids included.
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
        // and the Esc were resolved in `prepass` too, and both halves below
        // compose the same context off them — so the two passes cannot
        // disagree about the frame they are drawing.
        self.resolve_gestures(ui, graph_ctx, out);
        self.record_canvas(ui, graph_ctx, out);
    }

    /// This frame's canvas context over `graph_ctx`, off the state
    /// `prepass` settled.
    ///
    /// For the passes that only *read* this canvas. The two that also drive
    /// its controllers hold `&mut self`, so they compose theirs from their own
    /// field destructure instead — a context borrows the geometry and the hits
    /// shared, and it cannot be handed to a method that wants all of `self`
    /// mutably.
    fn canvas_ctx<'a>(&'a self, graph_ctx: GraphCtx<'a>) -> CanvasCtx<'a> {
        CanvasCtx::new(
            graph_ctx,
            &self.geometry,
            &self.hits,
            self.gesture,
            self.cancelled,
        )
    }

    /// The record pass's gesture half: run each controller's record-phase
    /// `apply`. Everything here reads last frame's responses and raises
    /// requests; nothing draws.
    fn resolve_gestures(&mut self, ui: &mut Ui, graph_ctx: GraphCtx<'_>, out: &mut Requests) {
        let Self {
            geometry,
            hits,
            gesture,
            cancelled,
            selection_ui,
            breaker_ui,
            connection_ui,
            new_node_ui,
            node_menu,
            ..
        } = self;
        let cx = CanvasCtx::new(graph_ctx, geometry, hits, *gesture, *cancelled);
        // Click on bare canvas (node panels hit-test first, so this
        // only fires when the click missed every node) clears the
        // selection. Skip when nothing is selected so we don't pollute
        // the undo stack with no-op `SetSelection` entries every time
        // the user clicks the empty canvas. A *drag* on bare canvas is
        // the rubber band (classified as `Select`), not a `Deselect`.
        if cx.gesture() == Some(CanvasGesture::Deselect) && !cx.graph_ctx().selected().is_empty() {
            out.push_graph(GraphIntent::SetSelection {
                to: BTreeSet::new(),
            });
        }
        // `CanvasGeometry` was already rebuilt in `prepass` against every
        // visible graph's scene — `App` rebuilds the scene *before* prepass
        // on the frame a tab becomes active, so prepass never sees a stale
        // graph, and the offset cache fills in port centers for nodes that
        // hadn't recorded yet. `cx` carries that same table; no second
        // rebuild needed.
        selection_ui.apply(ui, cx, out);
        breaker_ui.apply(ui, cx, out);
        // A connection released over empty canvas (detected in `prepass`)
        // opens the new-node popup; picking a node re-floats the wire. Only
        // the pane holding the dropped wire's source claims it.
        let pending_connection = connection_ui.take_pending_connection();
        // A right-click that just ended a floating wire shouldn't also open
        // the palette — suppress the `NewNode` gesture for this frame, by
        // handing the popup a context whose gesture slot is empty.
        let popup_cx = if connection_ui.ended_on_secondary() {
            cx.without_gesture()
        } else {
            cx
        };
        new_node_ui.apply(ui, popup_cx, pending_connection, out);
        // The menu records every frame whatever comes of it — its popup owns a
        // lifecycle that depends on it — and everything a pick means goes onto
        // `out`, so there is no precedence to settle here: a frame in which
        // both a menu pick and a chip click landed raises both.
        node_menu.apply(ui, cx, out);
        emit_chip_commands(cx, out);
    }

    /// The record pass's drawing half: the outer (pan-capture) canvas, the
    /// dotted backdrop, and — under the inner canvas's pan/zoom transform —
    /// the wires, node bodies, inspection panels, and in-flight
    /// gesture previews.
    fn record_canvas(&mut self, ui: &mut Ui, graph_ctx: GraphCtx<'_>, out: &mut Requests) {
        let Self {
            visible: _,
            background,
            geometry,
            hits,
            cancelled,
            inspectors,
            gesture,
            node_ui,
            breaker_ui,
            connection_ui,
            subscription_ui,
            selection_ui,
            ..
        } = self;
        let cx = CanvasCtx::new(graph_ctx, geometry, hits, *gesture, *cancelled);
        let theme = cx.theme();
        let viewport = graph_ctx.viewport();
        let (pan_val, zoom_val) = (viewport.pan, viewport.zoom);
        // Effective selection to paint: the live rubber-band preview while
        // a band is in flight over *this* pane, else its committed set.
        // The preview is kept out of the document, so a band in flight
        // changes what paints without recording an edit.
        let selected = selection_ui
            .preview()
            .map_or(Selection::Committed(graph_ctx.selected()), Selection::swept);

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
                background.draw(ui, theme, pan_val, zoom_val);
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
                        selection_ui.draw(ui, theme);
                        // One bundle for everything this pane records: the
                        // node bodies below, and the inspection
                        // panels after them. Built out here rather than inside
                        // the probe scope so both passes read the same refs.
                        let dcx = DrawCtx::new(cx, selected, inspectors, cull);
                        {
                            let mut probe = breaker_ui.probe();
                            // One emphasis resolution for both wire families:
                            // any wire gesture — either drag controller or an
                            // active breaker scribble — fades the standing set.
                            // All three are scoped to this pane, so a gesture
                            // running on a neighbouring canvas leaves these
                            // wires at full strength.
                            let fading = connection_ui.is_dragging()
                                || subscription_ui.is_dragging()
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
                            // Node bodies paint in `scene.z_order`, so a
                            // clicked node raises above its neighbours.
                            node_ui.draw_all(ui, dcx, &mut probe, out);
                        }
                        // Inspection panels paint after the node bodies so
                        // they sit on top and win clicks over the nodes
                        // beneath; positioned in world coords, so they ride
                        // the inner-canvas transform.
                        inspectors.draw_panels(ui, dcx);
                        breaker_ui.draw(ui, theme);
                        connection_ui.draw_in_flight(ui, cx, canvas_origin);
                        subscription_ui.draw_in_flight(ui, cx, canvas_origin);
                    });
            });
    }
}

/// Queue the [`AppCommand`] behind each chip click in the recorded tree: an
/// `FsPath` input's pick button, a node header's play or cache-eviction chip.
///
/// Each source surfaces only a domain fact — which node to run, which port to
/// pick a path for — and naming `AppCommand` is the canvas's job, since it is
/// what knows these are app-tier. So the translation lives here rather than in
/// `node`. All three are pure reads over [`CanvasHits`].
fn emit_chip_commands(cx: CanvasCtx<'_>, out: &mut Requests) {
    let hits = cx.hits();
    // A hit is keyed by a document-unique `NodeId`, so it can belong to a
    // neighbouring pane — or to a node this pane no longer holds, since the
    // sweep ran against last frame's projection. Both fall out here.
    let in_scope = |id: NodeId| cx.graph_ctx().contains(id).then_some(id);
    if let Some(req) = emit_path_picks(cx) {
        out.push_app(AppCommand::Edit(EditCommand::PickInputPath(req)));
    }
    // A header play-chip click runs that node's cone — the same command the
    // context menu's "Run to this node" resolves to.
    if let Some(node_id) = hits.chip(Chip::Play).and_then(in_scope) {
        out.push_app(AppCommand::Run(RunCommand::Node(node_id)));
    }
    if let Some(node_id) = hits.chip(Chip::EvictCache).and_then(in_scope) {
        out.push_app(AppCommand::Run(RunCommand::EvictCache(node_id)));
    }
}

#[cfg(test)]
pub(crate) mod harness;
