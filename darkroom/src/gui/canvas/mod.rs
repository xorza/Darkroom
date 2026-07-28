mod anchored_menu;
mod background;
pub(crate) mod breaker;
mod connection_ui;
pub(crate) mod cull;
pub(crate) mod drag_anchor;
pub(crate) mod geometry;
mod graph_menu;
pub(crate) mod inspector;
mod new_node_ui;
pub(crate) mod node_menu;
pub(crate) mod pan_zoom;
mod preview_drag;
mod selection_ui;
mod subscription_ui;
#[cfg(test)]
mod tests;
mod wire;

use glam::Vec2;
use palantir::{
    Background, Configure, Panel, PointerButton, Sense, Sizing, TranslateScale, Ui, WidgetId,
};
use scenarium::Library;
use std::collections::BTreeSet;

use crate::core::document::{GraphRef, Viewport};
use crate::core::edit::intent::sink::Intents;
use crate::core::edit::intent::types::Intent;
use crate::gui::app::AppContext;
use crate::gui::app::commands::AppCommand;
use crate::gui::app::commands::edit::EditCommand;
use crate::gui::app::commands::run::RunCommand;
use crate::gui::canvas::background::CanvasBackground;
use crate::gui::canvas::breaker::BreakerUI;
use crate::gui::canvas::connection_ui::ConnectionUI;
use crate::gui::canvas::cull::CullRegion;
use crate::gui::canvas::geometry::CanvasGeometry;
use crate::gui::canvas::graph_menu::GraphMenuUi;
use crate::gui::canvas::inspector::Inspectors;
use crate::gui::canvas::new_node_ui::NewNodeUi;
use crate::gui::canvas::node_menu::{NodeMenuAction, NodeMenuUi};
use crate::gui::canvas::pan_zoom::PanAnchor;
use crate::gui::canvas::preview_drag::PreviewDrag;
use crate::gui::canvas::selection_ui::SelectionUI;
use crate::gui::canvas::subscription_ui::SubscriptionUI;
use crate::gui::canvas::wire::{WireEmphasis, WirePass};
use crate::gui::node::prepass::{
    emit_cache_evictions, emit_path_picks, emit_play_clicks, emit_port_dblclicks,
};
use crate::gui::node::{NodeUI, RecordCtx};
use crate::gui::scene::{GraphScene, Scene};

/// Canvas-level UI scope, shared by **every** graph pane on screen: the
/// port-widget-id cache, the `NodeUI` that renders graph nodes, the
/// inspection panels, and the in-flight gesture controllers.
///
/// One instance drives N canvases rather than one per pane, because
/// everything here is either keyed by a document-unique id (geometry,
/// inspectors) or inherently singular (there is one pointer, so one drag,
/// one rubber band, one open popup). What *is* per-pane — the canvas
/// widget ids, the viewport, the paint stack — comes from the
/// [`GraphScene`] handed to [`Self::draw`], and each gesture that spans
/// frames records the [`GraphRef`] it latched on so the other panes'
/// passes leave it alone.
///
/// The frame splits accordingly:
/// - [`Self::prepass`] runs **once** over the whole scene, with a small
///   per-pane loop inside for the viewport-dependent parts.
/// - [`Self::draw`] runs **once per visible graph pane**, from the dock's
///   content closure.
///
/// **Bare-canvas gesture arbitration.** [`classify_canvas_gesture`] reads
/// one pane's outer-canvas response ([`outer_canvas_widget_id`]) +
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
    background: CanvasBackground,
    pub(crate) geometry: CanvasGeometry,
    /// Open inspection panels, keyed by node. Outside the gesture group
    /// so pinned panels survive a tab switch; panels only paint for nodes
    /// in the active scene, so off-tab ones hide and reappear.
    inspectors: Inspectors,
    /// This frame's bare-canvas gesture and the pane it latched on,
    /// resolved once in [`Self::prepass`] and read back by [`Self::draw`].
    /// At most one pane can own a press, so one slot is enough.
    gesture: Option<PaneGesture>,
    /// In-flight gesture controllers. Grouped so a tab switch can reset
    /// *all* of them in one assignment (`clear_gestures`) without the
    /// caller enumerating each — and so the persistent caches
    /// (`background`, `geometry`) sitting beside this field survive by
    /// construction.
    gestures: Gestures,
}

/// A bare-canvas gesture together with the pane whose canvas it landed on.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
struct PaneGesture {
    target: GraphRef,
    gesture: CanvasGesture,
}

/// The resettable, one-gesture-lifetime controllers. Everything here is
/// dropped on a tab switch, and nothing here carries meaning across frames.
#[derive(Default, Debug)]
struct Gestures {
    node_ui: NodeUI,
    breaker_ui: BreakerUI,
    connection_ui: ConnectionUI,
    preview_drag: PreviewDrag,
    subscription_ui: SubscriptionUI,
    new_node_ui: NewNodeUi,
    graph_menu: GraphMenuUi,
    node_menu: NodeMenuUi,
    selection_ui: SelectionUI,
    /// `Scene::pan` snapshot captured at the frame the active pan-drag
    /// latched, keyed by the pane that latched it. While the drag is
    /// active, that pane's `viewport.pan = anchor + drag_delta`. Input
    /// bookkeeping (lifetime = one gesture), not viewport state — and
    /// keyed because `emit_pan_zoom` runs once per visible pane, so the
    /// idle ones must not consume the live one's release edge.
    pan_anchor: PanAnchor<GraphRef>,
}

impl GraphUI {
    /// Drop all in-flight gesture state while **keeping** cross-frame
    /// caches — notably `CanvasGeometry`'s port-offset table, so connections
    /// still anchor on the first frame after a tab switch. Called when
    /// the active tab changes.
    pub(crate) fn clear_gestures(&mut self) {
        self.gestures = Gestures::default();
        // Transient inspection panels are tab-local; drop them on a
        // switch. Pinned ones persist and reappear with their nodes.
        self.inspectors.close_unpinned();
    }

    /// Take the node context-menu action picked this frame, if any, with the
    /// pane it was picked in. The `Editor` resolves it against that pane's
    /// live selection (it owns the `Document` needed to build the duplicate /
    /// removal intents).
    pub(crate) fn take_node_menu_action(&mut self) -> Option<(NodeMenuAction, GraphRef)> {
        self.gestures.node_menu.take_action()
    }

    /// Pre-record pass — see
    /// [`crate::gui::node::NodeUI::prepass`]. Every input-derived
    /// intent that can change layout is emitted here, *before* the
    /// record, so its effect is applied to `Document` by the pre-record
    /// drain and Pass A records the settled layout:
    ///
    /// - pan/zoom (`emit_pan_zoom` → `Intent::SetViewport`),
    /// - node drag (`node_ui.prepass` → `Intent::MoveSelection`),
    /// - connection commit (`connection_ui.apply` → `Intent::SetInput`).
    ///
    /// Connection commit specifically *must* be here: binding an input
    /// that had a const value removes its inline editor and resizes the
    /// node. If committed during the record (post-record drain), Pass A
    /// records the pre-resize layout and the relayout's Pass B rebuilds
    /// `CanvasGeometry` from that stale cascade — the new connection floats
    /// to the old port. Committing pre-record makes `cascade_A` the
    /// resized layout, so Pass B anchors the curve correctly with no
    /// extra frame. `CanvasGeometry` is rebuilt here (and reused by `frame`)
    /// because the commit reads it. Navigation (tab/open) is handled
    /// separately, before this, so the target is already fixed here.
    ///
    /// Runs **once** for the whole scene. The viewport-dependent half —
    /// the bare-canvas gesture classification and pan/zoom, which read one
    /// pane's outer-canvas response — loops the visible panes; everything
    /// else is keyed by document-unique ids and sweeps them all at once,
    /// resolving each hit's edit target from `SceneNode::owner`.
    pub(crate) fn prepass(
        &mut self,
        ui: &mut Ui,
        scene: &Scene,
        library: &Library,
        out: &mut Intents,
    ) {
        // Resolve the frame's bare-canvas gesture and park it (with its
        // pane) for `draw` to read back — the classification is one
        // response poll, and both phases must agree on the winner.
        self.gesture = None;
        for graph in scene.graphs() {
            let target = graph.target();
            let gesture = classify_canvas_gesture(ui, target);
            if let Some(gesture) = gesture {
                self.gesture = Some(PaneGesture { target, gesture });
            }
            pan_zoom::emit_pan_zoom(&mut self.gestures.pan_anchor, ui, graph, gesture, out);
            emit_port_dblclicks(ui, graph, out);
        }
        self.gestures.node_ui.prepass(ui, scene, out);
        self.geometry.rebuild(ui, scene);
        // Both port-drag claimants sit *after* the rebuild so they read this
        // frame's drag edges and centers, and `preview_drag_modifier` keeps
        // them disjoint: the preview spawn takes the output column under the
        // chord, the wire gesture takes it otherwise.
        self.gestures
            .preview_drag
            .apply(ui, scene, &self.geometry, library, out);
        // A node picked from a drop-spawned palette last frame re-floats its
        // wire so the user clicks the exact port to land it.
        let resume = self.gestures.new_node_ui.take_resume_floating();
        self.gestures
            .connection_ui
            .apply(ui, scene, &self.geometry, resume, out);
        // Subscription wires (emitter → subscriber) latch/commit here, for
        // the same pre-record reasons as the connection gesture above; an
        // emitter glyph and a data port can't both latch (different widget-id
        // spaces).
        self.gestures
            .subscription_ui
            .apply(ui, scene, &self.geometry, out);
        // Inspector chip toggles + the close-on-outside-action sweep, both
        // read off last frame's responses like everything else here. Whole
        // scene, so a panel pinned on a pane that just closed is pruned.
        self.inspectors.apply(ui, scene);
    }

    /// Record one graph pane: its gestures' record-phase halves, then the
    /// canvas itself. Called once per visible graph tab from the dock's
    /// content closure, so everything here is scoped to `graph` — the
    /// canvas widget ids included.
    ///
    /// Returns the [`AppCommand`] this pane contributes, if any. Which
    /// command *wins the frame* is not decided here: the caller arbitrates
    /// every tab kind's answer through one `claim`, so the canvas states its
    /// own precedence and nothing else.
    pub(crate) fn draw(
        &mut self,
        ui: &mut Ui,
        ctx: &AppContext<'_>,
        graph: GraphScene<'_>,
        out: &mut Intents,
    ) -> Option<AppCommand> {
        // Pan/zoom was already folded into the document in `prepass`
        // and mirrored into `scene` by `Scene::rebuild`, so the
        // transform below reads the up-to-date viewport directly. The
        // gesture was classified there too; only the pane that owns this
        // frame's press sees a `Some`.
        let gesture = self
            .gesture
            .filter(|g| g.target == graph.target())
            .map(|g| g.gesture);
        let command = self.resolve_gestures(ui, ctx, graph, gesture, out);
        self.bake_snap_hovers();
        self.record_canvas(ui, ctx, graph, out);
        command
    }

    /// The record pass's gesture half: run each controller's record-phase
    /// `apply`, and settle which [`AppCommand`] (if any) this pane
    /// contributes. Everything here reads last frame's responses and pushes
    /// intents; nothing draws.
    fn resolve_gestures(
        &mut self,
        ui: &mut Ui,
        ctx: &AppContext<'_>,
        graph: GraphScene<'_>,
        gesture: Option<CanvasGesture>,
        out: &mut Intents,
    ) -> Option<AppCommand> {
        let target = graph.target();
        // Click on bare canvas (node panels hit-test first, so this
        // only fires when the click missed every node) clears the
        // selection. Skip when nothing is selected so we don't pollute
        // the undo stack with no-op `SetSelection` entries every time
        // the user clicks the empty canvas. A *drag* on bare canvas is
        // the rubber band (classified as `Select`), not a `Deselect`.
        if gesture == Some(CanvasGesture::Deselect) && !graph.selected().is_empty() {
            out.push(
                target,
                Intent::SetSelection {
                    to: BTreeSet::new(),
                },
            );
        }
        // `CanvasGeometry` was already rebuilt in `prepass` against every
        // visible graph's scene — `App` rebuilds the scene *before* prepass
        // on the frame a tab becomes active, so prepass never sees a stale
        // graph, and the offset cache fills in port centers for nodes that
        // hadn't recorded yet. Reuse it here; no second rebuild needed.
        self.gestures
            .selection_ui
            .apply(ui, graph, &self.geometry, gesture, out);
        self.gestures.breaker_ui.apply(ui, graph, gesture, out);
        // A connection released over empty canvas (detected in `prepass`)
        // opens the new-node popup; picking a node re-floats the wire. Only
        // the pane holding the dropped wire's source claims it.
        let pending_connection = self
            .gestures
            .connection_ui
            .take_pending_connection_in(graph);
        // A right-click that just ended a floating wire shouldn't also open
        // the palette — suppress the `NewNode` gesture for this frame.
        let popup_gesture = if self.gestures.connection_ui.ended_on_secondary() {
            None
        } else {
            gesture
        };
        self.gestures
            .new_node_ui
            .apply(ui, ctx, graph, popup_gesture, pending_connection, out);
        // This pane's own precedence, in the order written: first source to
        // answer wins, and nothing below can overwrite a decision above.
        //
        // Both context menus are polled whatever comes of it — their popups
        // own a lifecycle that has to record every frame, and a pick's other
        // effects (a `DetachGraph` intent, a stashed `NodeMenuAction`) land
        // through `out` rather than through the return. The chip scans are
        // pure reads over last frame's responses, so `or_else` short-circuits
        // past them once a menu has answered.
        self.gestures
            .graph_menu
            .apply(ui, graph, out)
            .or(self.gestures.node_menu.apply(ui, graph, out))
            .or_else(|| emit_chip_command(ui, graph))
    }

    /// Bake each in-flight wire drag's snap target into `CanvasGeometry`'s
    /// hover flags. Each controller knows which glyph layer its target lives
    /// in, so the override is one call apiece rather than an accessor per
    /// layer read back out here.
    fn bake_snap_hovers(&mut self) {
        self.gestures
            .connection_ui
            .bake_snap_hover(&mut self.geometry);
        self.gestures
            .subscription_ui
            .bake_snap_hover(&mut self.geometry);
    }

    /// The record pass's drawing half: the outer (pan-capture) canvas, the
    /// dotted backdrop, and — under the inner canvas's pan/zoom transform —
    /// the wires, node bodies, inspection panels, and in-flight
    /// gesture previews.
    fn record_canvas(
        &mut self,
        ui: &mut Ui,
        ctx: &AppContext<'_>,
        graph: GraphScene<'_>,
        out: &mut Intents,
    ) {
        let target = graph.target();
        let Self {
            background,
            geometry,
            inspectors,
            gesture: _,
            gestures:
                Gestures {
                    node_ui,
                    breaker_ui,
                    connection_ui,
                    preview_drag: _,
                    subscription_ui,
                    new_node_ui: _,
                    graph_menu: _,
                    node_menu: _,
                    selection_ui,
                    pan_anchor: _,
                },
        } = self;
        let viewport = graph.viewport();
        let (pan_val, zoom_val) = (viewport.pan, viewport.zoom);
        // Effective selection to paint: the live rubber-band preview while
        // a band is in flight over *this* pane, else its committed set.
        // Kept off `Scene` so the projection stays a read-only mirror of
        // `Document`.
        let selected = selection_ui.preview(target).unwrap_or(graph.selected());

        // Outer canvas: covers the whole pane, paints the canvas
        // background, owns the input routing for empty-canvas
        // gestures. Senses:
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
            .id(outer_canvas_widget_id(target))
            .size((Sizing::FILL, Sizing::FILL))
            .sense(Sense::CLICK | Sense::DRAG | Sense::SCROLL | Sense::PINCH)
            .clip_rect()
            .background(Background::fill(ctx.theme.colors.canvas_bg))
            .show(ui, |ui| {
                // Dotted backdrop in screen space, beneath the inner
                // (transformed) canvas — so it paints under everything.
                background.draw(ui, ctx, target, pan_val, zoom_val);
                Panel::canvas()
                    .id(inner_canvas_widget_id(target))
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
                            .response_for(inner_canvas_widget_id(target))
                            .layout_rect
                            .map(|r| r.min)
                            .unwrap_or(Vec2::ZERO);
                        let cull = CullRegion::from_canvas(
                            ui.response_for(outer_canvas_widget_id(target)).layout_rect,
                            canvas_origin,
                            &viewport,
                        );
                        // Painted first so it sits beneath the
                        // connections and node bodies.
                        selection_ui.draw(ui, ctx, target);
                        // One bundle for everything this pane records: the
                        // node bodies below, and the inspection
                        // panels after them. Built out here rather than inside
                        // the probe scope so both passes read the same refs.
                        let rcx = RecordCtx {
                            theme: ctx.theme,
                            library: ctx.library,
                            graph,
                            selected,
                            geometry,
                            inspectors,
                            run_state: ctx.run_state,
                        };
                        {
                            let mut probe = breaker_ui.probe(target);
                            // One emphasis resolution for both wire families:
                            // any wire gesture — either drag controller or an
                            // active breaker scribble — fades the standing set.
                            // All three are scoped to this pane, so a gesture
                            // running on a neighbouring canvas leaves these
                            // wires at full strength.
                            let fading = connection_ui.dragging_in(graph)
                                || subscription_ui.dragging_in(graph)
                                || probe.is_active();
                            let emphasis =
                                WireEmphasis::resolve(ctx.theme.colors.canvas_bg, fading);
                            // Both wire renderers share these inputs, so
                            // they're bundled once and reborrowed into each.
                            // Subscription wires sit under the node bodies
                            // like data wires (drawn before `draw_all`), and
                            // share the breaker probe so they're all
                            // cuttable — one passing behind an unrelated node
                            // goes under it rather than drawing on top.
                            let mut wires = WirePass {
                                rcx,
                                cull,
                                probe: &mut probe,
                                emphasis: &emphasis,
                            };
                            connection_ui::draw(ui, &mut wires);
                            subscription_ui::draw(ui, &mut wires);
                            // Node bodies paint in `scene.z_order`, so a
                            // clicked node raises above its neighbours.
                            node_ui.draw_all(ui, rcx, cull, &mut probe, out);
                        }
                        // Inspection panels paint after the node bodies so
                        // they sit on top and win clicks over the nodes
                        // beneath; positioned in world coords, so they ride
                        // the inner-canvas transform.
                        inspectors.draw_panels(ui, rcx);
                        breaker_ui.draw(ui, ctx, target);
                        connection_ui.draw_in_flight(ui, ctx, graph, geometry, canvas_origin);
                        subscription_ui.draw_in_flight(ui, ctx, graph, geometry, canvas_origin);
                    });
            });
    }
}

/// Whether the modifier reserving an output-port drag for spawning a preview
/// is held. The one place that chord is decided: [`PreviewDrag`] claims an
/// output drag under it, and `ConnectionUI` drops the output column from its
/// latch candidates under the same condition — stated once so the two cannot
/// drift into both claiming, or neither.
pub(super) fn preview_drag_modifier(ui: &mut Ui) -> bool {
    ui.modifiers().ctrl
}

/// Which bare-canvas gesture a fresh press/click latches this frame.
/// Resolved once by [`classify_canvas_gesture`] so the precedence among
/// the competing controllers lives in a single place rather than being
/// re-derived (and kept disjoint by hand) in each one.
///
/// Covers the *latch* frame only: continuation of an in-flight gesture is
/// tracked by each controller's own `Option<state>`, and wheel/pinch zoom
/// coexists with everything (handled in `emit_pan_zoom`, not here).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum CanvasGesture {
    /// Middle-button drag → viewport pan.
    Pan,
    /// Plain LMB-drag (no modifier) → rubber-band selection.
    Select,
    /// Ctrl+LMB-drag or RMB-drag → connection breaker. Carries the button
    /// that latched it, since the breaker polls that same button for
    /// continuation/release (a Cmd+LMB breaker must keep reading Left).
    Breaker(PointerButton),
    /// RMB-click or LMB double-click on empty canvas (no drag) → new-node
    /// popup.
    NewNode,
    /// LMB-click (no drag) → clear selection.
    Deselect,
}

/// Resolve `target`'s bare-canvas gesture for this frame from that pane's
/// outer-canvas response + modifiers. Drag-starts are checked before clicks
/// (palantir reports `clicked`/`secondary_clicked` only on a release that
/// *didn't* drag, but the explicit ordering keeps the precedence obvious).
/// `None` when nothing latched — an idle canvas, or a press a node/port
/// captured. With several panes open at most one can answer `Some`: the
/// press lands on exactly one canvas.
///
/// This only ever sees presses that *missed* every node and port: a
/// node/badge widget captures its own press, so a right-click on a node
/// body or `G` badge routes to `node_menu` / `graph_menu` (which read
/// those widgets' `secondary_clicked` directly) and never reaches here —
/// `NewNode` is therefore right-click-on-*empty*-canvas by construction.
fn classify_canvas_gesture(ui: &mut Ui, target: GraphRef) -> Option<CanvasGesture> {
    let resp = ui.response_for(outer_canvas_widget_id(target));
    if resp.middle.drag.started() {
        return Some(CanvasGesture::Pan);
    }
    if resp.right.drag.started() {
        return Some(CanvasGesture::Breaker(PointerButton::Right));
    }
    if resp.left.drag.started() {
        return Some(if ui.modifiers().ctrl {
            CanvasGesture::Breaker(PointerButton::Left)
        } else {
            CanvasGesture::Select
        });
    }
    // A double-click sets `clicked` *and* `double_click` on the same frame,
    // so this must precede the plain-click `Deselect` arm to win. The first
    // click of the pair already ran its own `Deselect`, so the selection is
    // clear by the time the popup opens.
    if resp.right.clicked() || resp.left.double_clicked() {
        return Some(CanvasGesture::NewNode);
    }
    if resp.left.clicked() {
        return Some(CanvasGesture::Deselect);
    }
    None
}

/// The one `AppCommand` a chip click in the recorded tree can produce this
/// frame: an `FsPath` input's pick button, a node header's play or
/// or cache-eviction chip. First hit in that order wins.
///
/// Each scan surfaces only a domain fact — which node to run, which port to
/// pick a path for — and naming `AppCommand` is the canvas's job, since it
/// owns the command channel. So the translation lives here rather than in
/// `node`. All three are pure reads over last frame's responses,
/// which is why [`GraphUI::draw`] can skip the whole group once something
/// else has claimed the frame.
fn emit_chip_command(ui: &Ui, graph: GraphScene<'_>) -> Option<AppCommand> {
    if let Some(req) = emit_path_picks(ui, graph) {
        return Some(AppCommand::Edit(EditCommand::PickInputPath(req)));
    }
    // A header play-chip click runs that node's cone — the same command the
    // context menu's "Run to this node" resolves to.
    if let Some(node_id) = emit_play_clicks(ui, graph) {
        return Some(AppCommand::Run(RunCommand::Node(node_id)));
    }
    if let Some(node_id) = emit_cache_evictions(ui, graph) {
        return Some(AppCommand::Run(RunCommand::EvictCache(node_id)));
    }
    None
}

/// Outer-canvas-local coords → inner-canvas pre-transform world
/// coords. Inner canvas applies `TranslateScale::new(pan, zoom)`,
/// so `outer = pan + zoom * world`.
fn to_world(outer_local: Vec2, viewport: &Viewport) -> Vec2 {
    (outer_local - viewport.pan) / viewport.zoom
}

/// The pointer in inner-canvas world coords, or `None` when it's off-window.
/// Where an in-flight wire's free end sits before it snaps to a target;
/// `canvas_origin` is the inner canvas's pre-transform origin.
fn pointer_world(ui: &mut Ui, graph: GraphScene<'_>, canvas_origin: Vec2) -> Option<Vec2> {
    ui.pointer_pos()
        .map(|p| to_world(p - canvas_origin, &graph.viewport()))
}

/// Stable id for one pane's outer (pan-capture) canvas. Keyed by the graph
/// it shows, so two panes side by side hit-test independently and a
/// gesture polled by target reaches the right canvas.
pub(crate) fn outer_canvas_widget_id(target: GraphRef) -> WidgetId {
    WidgetId::from_hash(("graph.canvas.outer", target))
}

/// Stable id for one pane's inner (transformed) canvas. Used as the widget
/// seed and for resolving the canvas's pre-transform origin in connection
/// draws.
fn inner_canvas_widget_id(target: GraphRef) -> WidgetId {
    WidgetId::from_hash(("graph.canvas.inner", target))
}
