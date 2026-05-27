pub(crate) mod background;
pub(crate) mod breaker;
pub(crate) mod connection_ui;
pub(crate) mod new_node_ui;
pub(crate) mod pan_zoom;
pub(crate) mod port_frame;
pub(crate) mod selection_ui;
pub(crate) mod subgraph_menu;

use glam::Vec2;
use palantir::{Background, Configure, Panel, Sense, Sizing, TranslateScale, Ui, WidgetId};
use std::collections::BTreeSet;

use crate::app::AppContext;
use crate::edit::intent::Intent;
use crate::gui::canvas::background::CanvasBackground;
use crate::gui::canvas::breaker::BreakerUI;
use crate::gui::canvas::connection_ui::ConnectionUI;
use crate::gui::canvas::new_node_ui::NewNodeUi;
use crate::gui::canvas::port_frame::PortFrame;
use crate::gui::canvas::selection_ui::SelectionUI;
use crate::gui::canvas::subgraph_menu::SubgraphMenuUi;
use crate::gui::menu_bar::MenuCommand;
use crate::gui::node::{NodeUI, RecordCtx, emit_port_disconnects};
use crate::gui::{PortKind, PortRef};
use crate::scene::{Scene, SceneNode};

/// Canvas-level UI scope: owns the port-widget-id cache, the
/// `NodeUI` that renders every graph node, and the manual pan/zoom
/// transform applied to the inner canvas. `frame` reads palantir's
/// pointer-event stream (drag on the outer canvas → pan, wheel/pinch
/// → zoom-about-cursor) and writes the result into [`Scene::pan`] /
/// [`Scene::zoom`], which then drive the inner canvas's
/// `TranslateScale`.
///
/// **Bare-canvas gesture arbitration.** Several sub-controllers each
/// poll the *same* outer-canvas response ([`outer_canvas_widget_id`])
/// and self-select on a button/modifier guard. The guards are kept
/// mutually exclusive by convention — there's no central dispatcher, so
/// keep them disjoint when editing:
/// - **Middle-drag** → pan ([`crate::gui::canvas::pan_zoom::emit_pan_zoom`]).
/// - **Wheel / pinch** → zoom-about-cursor (same).
/// - **Plain LMB-drag** (no modifier) → rubber-band select (`SelectionUI`).
/// - **Ctrl+LMB-drag** or **RMB-drag** → connection breaker (`BreakerUI`).
/// - **RMB-click** (not drag) → new-node popup (`NewNodeUi`).
///
/// Node panels and port circles live in the *inner* canvas and hit-test
/// first, so these only fire when a gesture falls through to bare canvas.
#[derive(Default, Debug)]
pub(crate) struct GraphUI {
    background: CanvasBackground,
    port_frame: PortFrame,
    /// In-flight gesture controllers. Grouped so a tab switch can reset
    /// *all* of them in one assignment (`clear_gestures`) without the
    /// caller enumerating each — and so the persistent caches
    /// (`background`, `port_frame`) sitting beside this field survive by
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
    new_node_ui: NewNodeUi,
    subgraph_menu: SubgraphMenuUi,
    selection_ui: SelectionUI,
    /// `Scene::pan` snapshot captured at the frame the active pan-drag
    /// latched. While the drag is active, `scene.pan = anchor +
    /// drag_delta`. Input bookkeeping (lifetime = one gesture), not
    /// viewport state.
    pan_anchor: Option<Vec2>,
}

impl GraphUI {
    /// Drop all in-flight gesture state while **keeping** cross-frame
    /// caches — notably `PortFrame`'s port-offset table, so connections
    /// still anchor on the first frame after a tab switch. Called when
    /// the active tab changes.
    pub(crate) fn clear_gestures(&mut self) {
        self.gestures = Gestures::default();
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
    /// `PortFrame` from that stale cascade — the new connection floats
    /// to the old port. Committing pre-record makes `cascade_A` the
    /// resized layout, so Pass B anchors the curve correctly with no
    /// extra frame. `PortFrame` is rebuilt here (and reused by `frame`)
    /// because the commit reads it. Navigation (tab/open) is handled
    /// separately, before this, so the target is already fixed here.
    pub(crate) fn prepass(&mut self, ui: &mut Ui, scene: &Scene, out: &mut Vec<Intent>) {
        pan_zoom::emit_pan_zoom(&mut self.gestures.pan_anchor, ui, scene, out);
        self.gestures.node_ui.prepass(ui, scene, out);
        emit_port_disconnects(ui, scene, out);
        self.port_frame.rebuild(ui, scene);
        self.gestures
            .connection_ui
            .apply(ui, scene, &self.port_frame, out);
    }

    pub(crate) fn frame(
        &mut self,
        ui: &mut Ui,
        ctx: &AppContext<'_>,
        scene: &mut Scene,
        out: &mut Vec<Intent>,
        cmd: &mut Option<MenuCommand>,
    ) {
        // Pan/zoom was already folded into the document in `prepass`
        // and mirrored into `scene` by `Scene::rebuild`, so the
        // transform below reads the up-to-date viewport directly.
        // Click on bare canvas (node panels hit-test first, so this
        // only fires when the click missed every node) clears the
        // selection. Skip when nothing is selected so we don't pollute
        // the undo stack with no-op `SetSelection` entries every time
        // the user clicks the empty canvas. A *drag* on bare canvas is
        // the rubber band (handled by `selection_ui`), not a click.
        if !scene.selected_nodes.is_empty() && ui.response_for(outer_canvas_widget_id()).clicked {
            out.push(Intent::SetSelection {
                to: BTreeSet::new(),
            });
        }
        // `PortFrame` was already rebuilt in `prepass` against the active
        // graph's scene — `App` rebuilds the scene *before* prepass on the
        // frame a tab becomes active, so prepass never sees a stale graph,
        // and the offset cache fills in port centers for nodes that hadn't
        // recorded yet. Reuse it here; no second rebuild needed.
        self.gestures.selection_ui.apply(ui, scene, out);
        self.gestures.breaker_ui.apply(ui, scene, out);
        self.gestures.new_node_ui.apply(ui, ctx, scene, out);
        self.gestures.subgraph_menu.apply(ui, scene, out, cmd);
        // Bake the snap target into `PortFrame.hovered` so node_ui's
        // port_row picks up the hover color via the same lookup it
        // uses for ordinary mouse-over. `response.hovered` is
        // suppressed on every widget except the drag-capture owner
        // while a drag is live, so without this override the
        // snapped-but-not-captured target stays at its idle color.
        if let Some(snap) = self.gestures.connection_ui.snap_port() {
            self.port_frame.set_hovered(snap);
        }

        let Self {
            background,
            port_frame,
            gestures:
                Gestures {
                    node_ui,
                    breaker_ui,
                    connection_ui,
                    new_node_ui: _,
                    subgraph_menu: _,
                    selection_ui,
                    pan_anchor: _,
                },
        } = self;
        let pan_val = scene.pan;
        let zoom_val = scene.zoom;

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
                fill: ctx.theme.canvas_bg.into(),
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
                        // (palantir's `Panel::transform` applies to
                        // the body: child subtrees AND direct
                        // shapes), so port `layout_rect`s and bezier
                        // endpoints stay aligned at every zoom.
                        let canvas_origin = ui
                            .response_for(inner_canvas_widget_id())
                            .layout_rect
                            .map(|r| r.min)
                            .unwrap_or(Vec2::ZERO);
                        // Painted first so it sits beneath the
                        // connections and node bodies.
                        selection_ui.draw(ui, ctx);
                        {
                            let mut probe = breaker_ui.probe(canvas_origin);
                            connection_ui.draw(ui, ctx, scene, port_frame, &mut probe);
                            let rcx = RecordCtx {
                                theme: ctx.theme,
                                scene,
                                port_frame,
                            };
                            node_ui.draw_all(ui, rcx, &mut probe, out);
                        }
                        breaker_ui.draw(ui, ctx);
                        connection_ui.draw_in_flight(ui, ctx, scene, port_frame, canvas_origin);
                    });
            });
    }
}

/// Every `PortRef` of `node` on the given side, in port order. Single
/// source for the "iterate a node's ports by kind" loop that
/// `PortFrame::rebuild` and the connection scans all need, so scan order
/// and paint order can't drift apart.
pub(crate) fn node_ports<'a>(
    scene: &'a Scene,
    node: &'a SceneNode,
    kind: PortKind,
) -> impl Iterator<Item = PortRef> + 'a {
    let span = match kind {
        PortKind::Input => node.inputs,
        PortKind::Output => node.outputs,
    };
    let count = scene.ports(span).len();
    (0..count).map(move |port_idx| PortRef {
        node_id: node.id,
        kind,
        port_idx,
    })
}

/// Outer-canvas-local coords → inner-canvas pre-transform world
/// coords. Inner canvas applies `TranslateScale::new(pan, zoom)`,
/// so `outer = pan + zoom * world`.
pub(crate) fn to_world(outer_local: Vec2, scene: &Scene) -> Vec2 {
    let zoom = if scene.zoom > 0.0 { scene.zoom } else { 1.0 };
    (outer_local - scene.pan) / zoom
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
