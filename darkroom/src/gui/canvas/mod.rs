pub(crate) mod anchored_menu;
pub(crate) mod background;
pub(crate) mod breaker;
pub(crate) mod connection_ui;
pub(crate) mod cull;
pub(crate) mod geometry;
pub(crate) mod inspector;
pub(crate) mod new_node_ui;
pub(crate) mod node_menu;
pub(crate) mod pan_zoom;
pub(crate) mod selection_ui;
pub(crate) mod subgraph_menu;
pub(crate) mod subscription_ui;
pub(crate) mod wire;

use aperture::{
    Background, Configure, Panel, PointerButton, Rect, Sense, Sizing, TranslateScale, Ui, WidgetId,
};
use glam::Vec2;
use scenarium::graph::NodeId;
use std::collections::BTreeSet;

use crate::core::document::Viewport;
use crate::core::edit::intent::Intent;
use crate::gui::app::AppContext;
use crate::gui::app::commands::AppCommand;
use crate::gui::app::commands::edit::EditCommand;
use crate::gui::canvas::background::CanvasBackground;
use crate::gui::canvas::breaker::BreakerUI;
use crate::gui::canvas::connection_ui::ConnectionUI;
use crate::gui::canvas::geometry::CanvasGeometry;
use crate::gui::canvas::inspector::Inspectors;
use crate::gui::canvas::new_node_ui::NewNodeUi;
use crate::gui::canvas::node_menu::{NodeMenuAction, NodeMenuUi};
use crate::gui::canvas::selection_ui::SelectionUI;
use crate::gui::canvas::subgraph_menu::SubgraphMenuUi;
use crate::gui::canvas::subscription_ui::SubscriptionUI;
use crate::gui::canvas::wire::WireEmphasis;
use crate::gui::node::{NodeUI, RecordCtx, emit_path_picks, emit_port_dblclicks};
use crate::gui::scene::{Scene, SceneNode};
use crate::gui::{PortKind, PortRef};

/// Canvas-level UI scope: owns the port-widget-id cache, the
/// `NodeUI` that renders every graph node, and the manual pan/zoom
/// transform applied to the inner canvas. `frame` reads aperture's
/// pointer-event stream (drag on the outer canvas → pan, wheel/pinch
/// → zoom-about-cursor) and writes the result into [`Scene::pan`] /
/// [`Scene::zoom`], which then drive the inner canvas's
/// `TranslateScale`.
///
/// **Bare-canvas gesture arbitration.** [`classify_canvas_gesture`] reads
/// the outer-canvas response ([`outer_canvas_widget_id`]) + modifiers
/// *once* per phase and resolves which gesture latches this frame into a
/// single [`CanvasGesture`]; each sub-controller is handed that
/// classification and consumes only its own variant, so there's no
/// hand-kept disjointness across files — the precedence lives in one
/// match. Wheel/pinch zoom isn't a latch gesture (it coexists), so it
/// stays inside `emit_pan_zoom` regardless of the classification.
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
    pub(crate) inspectors: Inspectors,
    /// In-flight gesture controllers. Grouped so a tab switch can reset
    /// *all* of them in one assignment (`clear_gestures`) without the
    /// caller enumerating each — and so the persistent caches
    /// (`background`, `geometry`) sitting beside this field survive by
    /// construction.
    gestures: Gestures,
}

/// The resettable, one-gesture-lifetime controllers. Everything here is
/// dropped on a tab switch; nothing here is a cross-frame cache.
#[derive(Default, Debug)]
struct Gestures {
    node_ui: NodeUI,
    breaker_ui: BreakerUI,
    connection_ui: ConnectionUI,
    subscription_ui: SubscriptionUI,
    new_node_ui: NewNodeUi,
    subgraph_menu: SubgraphMenuUi,
    node_menu: NodeMenuUi,
    selection_ui: SelectionUI,
    /// `Scene::pan` snapshot captured at the frame the active pan-drag
    /// latched. While the drag is active, `scene.pan = anchor +
    /// drag_delta`. Input bookkeeping (lifetime = one gesture), not
    /// viewport state.
    pan_anchor: Option<Vec2>,
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

    /// The nodes with an open inspection panel, for the frame loop to
    /// request runtime values for.
    pub(crate) fn open_inspector_nodes(&self) -> impl Iterator<Item = NodeId> + '_ {
        self.inspectors.open_nodes()
    }

    /// Take the node context-menu action picked this frame, if any. The
    /// `Editor` resolves it against the live selection (it owns the
    /// `Document` needed to build the duplicate / removal intents).
    pub(crate) fn take_node_menu_action(&mut self) -> Option<NodeMenuAction> {
        self.gestures.node_menu.take_action()
    }

    /// Pre-record pass — see
    /// [`crate::gui::node::NodeUI::prepass`]. Every input-derived
    /// intent that can change layout is emitted here, *before* the
    /// record, so its effect is applied to `Document` by the pre-record
    /// drain and Pass A records the settled layout:
    ///
    /// - pan/zoom (`emit_pan_zoom` → `Intent::SetViewport`),
    /// - node drag (`node_ui.prepass` → `Intent::MoveNodes`),
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
    pub(crate) fn prepass(&mut self, ui: &mut Ui, scene: &Scene, out: &mut Vec<Intent>) {
        let gesture = classify_canvas_gesture(ui);
        pan_zoom::emit_pan_zoom(&mut self.gestures.pan_anchor, ui, scene, gesture, out);
        self.gestures.node_ui.prepass(ui, scene, out);
        emit_port_dblclicks(ui, scene, out);
        self.geometry.rebuild(ui, scene);
        // A node picked from a drop-spawned palette last frame re-floats its
        // wire so the user clicks the exact port to land it.
        let resume = self.gestures.new_node_ui.take_resume_floating();
        self.gestures
            .connection_ui
            .apply(ui, scene, &self.geometry, resume, out);
        // Subscription wires (emitter → subscriber) latch/commit here too,
        // for the same pre-record reasons; an emitter glyph and a data port
        // can't both latch (different widget-id spaces).
        self.gestures
            .subscription_ui
            .apply(ui, scene, &self.geometry, out);
    }

    pub(crate) fn frame(
        &mut self,
        ui: &mut Ui,
        ctx: &AppContext<'_>,
        scene: &Scene,
        out: &mut Vec<Intent>,
        cmd: &mut Option<AppCommand>,
    ) {
        // Pan/zoom was already folded into the document in `prepass`
        // and mirrored into `scene` by `Scene::rebuild`, so the
        // transform below reads the up-to-date viewport directly.
        let gesture = classify_canvas_gesture(ui);
        // Click on bare canvas (node panels hit-test first, so this
        // only fires when the click missed every node) clears the
        // selection. Skip when nothing is selected so we don't pollute
        // the undo stack with no-op `SetSelection` entries every time
        // the user clicks the empty canvas. A *drag* on bare canvas is
        // the rubber band (classified as `Select`), not a `Deselect`.
        if gesture == Some(CanvasGesture::Deselect) && !scene.selected_nodes.is_empty() {
            out.push(Intent::SetSelection {
                to: BTreeSet::new(),
            });
        }
        // `CanvasGeometry` was already rebuilt in `prepass` against the active
        // graph's scene — `App` rebuilds the scene *before* prepass on the
        // frame a tab becomes active, so prepass never sees a stale graph,
        // and the offset cache fills in port centers for nodes that hadn't
        // recorded yet. Reuse it here; no second rebuild needed.
        self.gestures
            .selection_ui
            .apply(ui, scene, &self.geometry, gesture, out);
        self.gestures.breaker_ui.apply(ui, scene, gesture, out);
        // A connection released over empty canvas (detected in `prepass`)
        // opens the new-node popup; picking a node re-floats the wire.
        let pending_connection = self.gestures.connection_ui.take_pending_connection();
        // A right-click that just ended a floating wire shouldn't also open
        // the palette — suppress the `NewNode` gesture for this frame.
        let popup_gesture = if self.gestures.connection_ui.ended_on_secondary() {
            None
        } else {
            gesture
        };
        self.gestures
            .new_node_ui
            .apply(ui, ctx, scene, popup_gesture, pending_connection, out);
        self.gestures.subgraph_menu.apply(ui, scene, out, cmd);
        self.gestures.node_menu.apply(ui, scene, out);
        // A click on an FsPath input's pick button surfaces a deferred
        // PickInputPath command (App opens the dialog outside the record).
        // The node UI returns a domain request; the canvas — which owns the
        // command channel — translates it, so node code never names
        // `AppCommand`. A command already set this frame wins.
        if cmd.is_none()
            && let Some(req) = emit_path_picks(ui, scene)
        {
            *cmd = Some(AppCommand::Edit(EditCommand::PickInputPath(req)));
        }
        // Bake the snap target into `CanvasGeometry.hovered` so node_ui's
        // port_row picks up the hover color via the same lookup it
        // uses for ordinary mouse-over. `response.hovered` is
        // suppressed on every widget except the drag-capture owner
        // while a drag is live, so without this override the
        // snapped-but-not-captured target stays at its idle color.
        if let Some(snap) = self.gestures.connection_ui.snap_port() {
            self.geometry.ports.set_hovered(snap);
        }
        // Same for an event drag's snapped subscription pin (emitter-started
        // drag) or snapped emitter glyph (subscriber-started drag).
        if let Some(sub) = self.gestures.subscription_ui.snap_sub() {
            self.geometry.subs.set_hovered(sub);
        }
        if let Some(emitter) = self.gestures.subscription_ui.snap_emitter() {
            self.geometry.events.set_hovered(emitter);
        }
        // Cycle inspector toggles + close transient panels on outside
        // actions, all from last-frame responses (same timing as every
        // other gesture here).
        self.inspectors.apply(ui, scene);

        let Self {
            background,
            geometry,
            inspectors,
            gestures:
                Gestures {
                    node_ui,
                    breaker_ui,
                    connection_ui,
                    subscription_ui,
                    new_node_ui: _,
                    subgraph_menu: _,
                    node_menu: _,
                    selection_ui,
                    pan_anchor: _,
                },
        } = self;
        let pan_val = scene.viewport.pan;
        let zoom_val = scene.viewport.zoom;
        // Effective selection to paint: the live rubber-band preview while
        // a band is in flight, else the committed set. Kept off `Scene` so
        // the projection stays a read-only mirror of `Document`.
        let selected = selection_ui.preview().unwrap_or(&scene.selected_nodes);

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
            .id(outer_canvas_widget_id())
            .size((Sizing::FILL, Sizing::FILL))
            .sense(Sense::CLICK | Sense::DRAG | Sense::SCROLL | Sense::PINCH)
            .clip_rect()
            .background(Background {
                fill: ctx.theme.colors.canvas_bg.into(),
                ..Default::default()
            })
            .show(ui, |ui| {
                // Dotted backdrop in screen space, beneath the inner
                // (transformed) canvas — so it paints under everything.
                background.draw(ui, ctx, pan_val, zoom_val);
                Panel::canvas()
                    .id(inner_canvas_widget_id())
                    .size((Sizing::FILL, Sizing::FILL))
                    .transform(TranslateScale::new(pan_val, zoom_val))
                    .show(ui, |ui| {
                        // Inner canvas's pre-transform origin. Shapes
                        // and child node panels recorded inside this
                        // closure share the inner canvas's transform
                        // (aperture's `Panel::transform` applies to
                        // the body: child subtrees AND direct
                        // shapes), so port `layout_rect`s and bezier
                        // endpoints stay aligned at every zoom.
                        let canvas_origin = ui
                            .response_for(inner_canvas_widget_id())
                            .layout_rect
                            .map(|r| r.min)
                            .unwrap_or(Vec2::ZERO);
                        // The world rect on screen, for record-time culling:
                        // only nodes and wires intersecting it are recorded.
                        // `None` until the outer canvas measures (first
                        // frame) — everything records.
                        let visible =
                            ui.response_for(outer_canvas_widget_id())
                                .layout_rect
                                .map(|r| {
                                    let outer_local = Rect {
                                        min: r.min - canvas_origin,
                                        size: r.size,
                                    };
                                    cull::visible_world_rect(outer_local, &scene.viewport)
                                });
                        // Painted first so it sits beneath the
                        // connections and node bodies.
                        selection_ui.draw(ui, ctx);
                        {
                            let mut probe = breaker_ui.probe(canvas_origin);
                            // One emphasis resolution for both wire families:
                            // any wire gesture — either drag controller or an
                            // active breaker scribble — fades the standing set.
                            let fading = connection_ui.dragging()
                                || subscription_ui.dragging()
                                || probe.state.is_some();
                            let emphasis =
                                WireEmphasis::resolve(ctx.theme.colors.canvas_bg, fading);
                            connection_ui
                                .draw(ui, ctx, scene, geometry, visible, &mut probe, &emphasis);
                            // Subscription wires sit under the node bodies
                            // like data wires (drawn before `draw_all`), and
                            // share the breaker probe so they're cuttable too.
                            subscription_ui
                                .draw(ui, ctx, scene, geometry, visible, &mut probe, &emphasis);
                            let rcx = RecordCtx {
                                theme: ctx.theme,
                                library: ctx.library,
                                scene,
                                selected,
                                geometry,
                                inspectors,
                            };
                            node_ui.draw_all(ui, rcx, visible, &mut probe, out);
                        }
                        // Inspection panels paint after the node bodies so
                        // they sit on top and win clicks over the nodes
                        // beneath; positioned in world coords, so they ride
                        // the inner-canvas transform.
                        inspectors.draw_panels(
                            ui,
                            ctx.theme,
                            ctx.library,
                            scene,
                            geometry,
                            ctx.run_state,
                        );
                        breaker_ui.draw(ui, ctx);
                        connection_ui.draw_in_flight(ui, ctx, scene, geometry, canvas_origin);
                        subscription_ui.draw_in_flight(ui, ctx, scene, geometry, canvas_origin);
                    });
            });
    }
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
pub(crate) enum CanvasGesture {
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

/// Resolve the bare-canvas gesture for this frame from the outer-canvas
/// response + modifiers. Drag-starts are checked before clicks (aperture
/// reports `clicked`/`secondary_clicked` only on a release that *didn't*
/// drag, but the explicit ordering keeps the precedence obvious). `None`
/// when nothing latched — an idle canvas, or a press a node/port captured.
///
/// This only ever sees presses that *missed* every node and port: a
/// node/badge widget captures its own press, so a right-click on a node
/// body or `S` badge routes to `node_menu` / `subgraph_menu` (which read
/// those widgets' `secondary_clicked` directly) and never reaches here —
/// `NewNode` is therefore right-click-on-*empty*-canvas by construction.
pub(crate) fn classify_canvas_gesture(ui: &Ui) -> Option<CanvasGesture> {
    let resp = ui.response_for(outer_canvas_widget_id());
    if resp.drag_started_by(PointerButton::Middle) {
        return Some(CanvasGesture::Pan);
    }
    if resp.drag_started_by(PointerButton::Right) {
        return Some(CanvasGesture::Breaker(PointerButton::Right));
    }
    if resp.drag_started_by(PointerButton::Left) {
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
    if resp.secondary_clicked || resp.double_clicked_by(PointerButton::Left) {
        return Some(CanvasGesture::NewNode);
    }
    if resp.clicked {
        return Some(CanvasGesture::Deselect);
    }
    None
}

/// Every `PortRef` of `node` on the given side, in port order. Single
/// source for the "iterate a node's ports by kind" loop that
/// `CanvasGeometry::rebuild` and the connection scans all need, so scan order
/// and paint order can't drift apart.
pub(crate) fn node_ports(node: &SceneNode, kind: PortKind) -> impl Iterator<Item = PortRef> + '_ {
    let span = match kind {
        PortKind::Input => node.inputs,
        PortKind::Output => node.outputs,
    };
    let count = span.len as usize;
    (0..count).map(move |port_idx| PortRef {
        node_id: node.id,
        kind,
        port_idx,
    })
}

/// Outer-canvas-local coords → inner-canvas pre-transform world
/// coords. Inner canvas applies `TranslateScale::new(pan, zoom)`,
/// so `outer = pan + zoom * world`.
pub(crate) fn to_world(outer_local: Vec2, viewport: &Viewport) -> Vec2 {
    let zoom = if viewport.zoom > 0.0 {
        viewport.zoom
    } else {
        1.0
    };
    (outer_local - viewport.pan) / zoom
}

/// The pointer in inner-canvas world coords, or `None` when it's off-window.
/// The free end of an in-flight wire that hasn't snapped to a target yet;
/// `canvas_origin` is the inner canvas's pre-transform origin.
pub(crate) fn pointer_world(ui: &mut Ui, scene: &Scene, canvas_origin: Vec2) -> Option<Vec2> {
    ui.pointer_pos()
        .map(|p| to_world(p - canvas_origin, &scene.viewport))
}

/// Stable id for the outer (pan-capture) canvas. `auto_stable` mixes
/// `file!()`/`line!()` so calls from different source lines stay
/// distinct; here we only need the id to survive between frames.
pub(crate) const fn outer_canvas_widget_id() -> WidgetId {
    WidgetId::auto_stable()
}

/// Stable id for the inner (transformed) canvas. Used as the widget
/// seed and for resolving the canvas's pre-transform origin in
/// connection draws.
const fn inner_canvas_widget_id() -> WidgetId {
    WidgetId::auto_stable()
}
