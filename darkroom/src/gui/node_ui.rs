use crate::app::AppContext;
use crate::gui::breaker::BreakerProbe;
use crate::gui::graph_ui::PortFrame;
use crate::gui::value_editor;
use crate::gui::{NODE_W, PORT_COL_PAD_TOP, PORT_GAP, PORT_RADIUS, PORT_SIZE, PortKind, PortRef};
use crate::intent::Intent;
use crate::scene::{InputBindingView, Scene, SceneNode};
use glam::Vec2;
use palantir::{
    Align, Background, Color, Configure, ContextMenu, Corners, Frame, HAlign, InternedStr,
    MenuItem, Panel, Rect, Sense, Sizing, Spacing, Stroke, Text, Ui, VAlign, WidgetId,
};
use scenarium::data::StaticValue;
use scenarium::function::FuncInput;
use scenarium::graph::Binding;
use scenarium::prelude::{Func, NodeId};

/// Owns rendering of every graph node plus the single active drag
/// anchor — the press-frame `pos` is snapshotted here so each
/// `MoveNode` target is `anchor.pos + drag_delta`, not a running
/// integration over the moving source. Only one node can hold the
/// pointer at a time, so one anchor slot is enough.
///
/// `draw_all` is the single entry point; `GraphUI` calls it once per
/// frame after [`crate::gui::graph_ui::PortFrame`] has been rebuilt
/// from last-frame's responses.
#[derive(Default, Debug)]
pub struct NodeUI {
    drag_anchor: Option<DragAnchor>,
}

#[derive(Clone, Copy, Debug)]
struct DragAnchor {
    node_id: NodeId,
    pos: Vec2,
    /// Captured from the `drag_started` frame's `Response::widget_id()`
    /// so subsequent frames can `ui.response_for(widget_id)` *before*
    /// recording and bake the current `drag_delta` into `.position(...)`.
    /// Lets the node paint at the cursor's location in Pass A directly
    /// — no need to wait for Pass B's relayout to catch up.
    widget_id: WidgetId,
}

impl NodeUI {
    /// Iterate every scene node, recording its widget tree and
    /// pushing port circle centers into `centers`. Inserts into
    /// `port_nodes` only when every port resolved a layout rect.
    /// Emits an `Intent::MoveNode` for any node holding an active
    /// LMB drag on its body (port circles capture their own clicks
    /// via `Sense::CLICK` so drags don't latch off the port grabs).
    pub fn draw_all(
        &mut self,
        ui: &mut Ui,
        ctx: &AppContext<'_>,
        scene: &Scene,
        port_frame: &PortFrame,
        probe: &mut BreakerProbe<'_>,
        out: &mut Vec<Intent>,
    ) {
        if let Some(b) = probe.state.as_deref_mut() {
            b.broken_nodes.clear();
        }
        for n in &scene.nodes {
            self.draw_one(ui, ctx, scene, n, port_frame, probe, out);
        }
        // Drop the anchor if its target node vanished from the graph
        // (mid-drag delete). Without this, the slot would linger and
        // could fire when a fresh node reused the id.
        if let Some(a) = self.drag_anchor
            && !scene.nodes.iter().any(|n| n.id == a.node_id)
        {
            self.drag_anchor = None;
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn draw_one(
        &mut self,
        ui: &mut Ui,
        ctx: &AppContext<'_>,
        scene: &Scene,
        node: &SceneNode,
        port_frame: &PortFrame,
        probe: &mut BreakerProbe<'_>,
        out: &mut Vec<Intent>,
    ) {
        let inputs = scene.ports(node.inputs);
        let outputs = scene.ports(node.outputs);
        let bindings = scene.bindings(node.input_bindings);
        let func = ctx.func_lib.by_id(&node.func_id);
        let theme = ctx.theme;

        // Probe last-frame's body rect (in canvas world coords) against
        // the breaker polyline. Hit → recolor border red and flag the
        // node for deletion on release. First-frame nodes have no rect
        // yet, so the breaker simply can't catch them until next frame
        // — acceptable: the user can't aim at something that hasn't
        // been painted.
        let body_rect = ui
            .response_for(node_widget_id(node.id))
            .layout_rect
            .map(|r| Rect {
                min: r.min - probe.origin,
                size: r.size,
            });
        let broken = match (probe.state.as_deref(), body_rect) {
            (Some(b), Some(r)) => b.intersects_rect(r),
            _ => false,
        };
        if broken {
            // unwrap: `broken == true` implies `state` is `Some`.
            probe
                .state
                .as_deref_mut()
                .unwrap()
                .broken_nodes
                .push(node.id);
        }
        let border = if broken {
            theme.connection_broken
        } else {
            theme.node_border
        };

        let panel = Panel::vstack()
            .id(node_widget_id(node.id))
            .position(node.pos)
            .size((Sizing::Fixed(NODE_W), Sizing::Hug))
            .sense(Sense::DRAG)
            .background(Background {
                fill: theme.node_fill.into(),
                stroke: Stroke::solid(border, theme.node_border_width),
                corners: Corners::all(theme.node_corner_radius),
                ..Default::default()
            })
            .show(ui, |ui| {
                header(ui, ctx, node.name.clone());
                ports_row(
                    ui, ctx, node.id, func, inputs, outputs, bindings, port_frame, out,
                );
            });
        let response = panel.response;

        // Latch the anchor on the press-frame edge; subsequent frames'
        // `prepass` peeks `response_for(widget_id)` before record runs
        // and converts `drag_delta` into a `MoveNode` applied to
        // `Document` upstream of `Scene::rebuild`.
        if response.drag_started() {
            self.drag_anchor = Some(DragAnchor {
                node_id: node.id,
                pos: node.pos,
                widget_id: response.widget_id(),
            });
        }
    }

    /// Pre-record pass: peek palantir's input state for any widgets
    /// this `NodeUI` owns and push the corresponding `Intent`s into
    /// `out`. Runs before `Scene::rebuild` in `App::frame`, so any
    /// state mutation applied from these intents (notably drag-driven
    /// `MoveNode`) lands in `Document` before recording — Pass A's
    /// arrange already reflects the cursor; no Pass B relayout retry.
    pub fn prepass(&mut self, ui: &Ui, scene: &Scene, out: &mut Vec<Intent>) {
        let Some(anchor) = self.drag_anchor else {
            return;
        };
        // Drop a stale anchor whose node was removed last frame (e.g.
        // breaker swipe deleted the dragged node). Without this, the
        // emitted `MoveNode` would target a missing node and panic in
        // `build_step`. `draw_all` also clears stale anchors, but only
        // after this prepass runs.
        if !scene.nodes.iter().any(|n| n.id == anchor.node_id) {
            self.drag_anchor = None;
            return;
        }
        let resp = ui.response_for(anchor.widget_id);
        // `drag_started` on a still-active anchor means a *new* gesture
        // just latched on the same widget — `record` will replace the
        // anchor this frame; emitting now with the stale `anchor.pos`
        // makes the node snap to the previous gesture's start point.
        if resp.drag_started() {
            self.drag_anchor = None;
            return;
        }
        // No `drag_delta` means the drag isn't latched anymore (release
        // or pointer-left-surface). Drop the anchor so the next gesture
        // starts fresh.
        let Some(delta) = resp.drag_delta() else {
            self.drag_anchor = None;
            return;
        };
        // `drag_delta` is in screen pixels; node positions live in the
        // canvas's pre-transform frame. Divide by zoom so cursor travel
        // matches node travel at every zoom level.
        let zoom = if scene.zoom > 0.0 { scene.zoom } else { 1.0 };
        out.push(Intent::MoveNode {
            node_id: anchor.node_id,
            to: anchor.pos + delta / zoom,
        });
    }
}

/// Stable widget id for the node's outer body panel. Derived from
/// the domain `NodeId` so `response_for` can probe last-frame's
/// arranged rect (used by the connection breaker's body-hit test)
/// without needing the panel's response to round-trip first.
pub(super) fn node_widget_id(node_id: NodeId) -> WidgetId {
    WidgetId::from_hash(("graph.node.body", node_id))
}

fn header(ui: &mut Ui, ctx: &AppContext<'_>, name: InternedStr) {
    let theme = ctx.theme;
    let r = theme.header_corner_radius;
    Panel::vstack()
        .id_salt("header")
        .size((Sizing::FILL, Sizing::Hug))
        .padding(Spacing::xy(8.0, 4.0))
        .background(Background {
            fill: theme.header_fill.into(),
            corners: Corners::new(r, r, 0.0, 0.0),
            ..Default::default()
        })
        .show(ui, |ui| {
            Text::new(name).show(ui);
        });
}

#[allow(clippy::too_many_arguments)]
fn ports_row(
    ui: &mut Ui,
    ctx: &AppContext<'_>,
    node_id: NodeId,
    func: Option<&Func>,
    inputs: &[InternedStr],
    outputs: &[InternedStr],
    bindings: &[InputBindingView],
    port_frame: &PortFrame,
    out: &mut Vec<Intent>,
) {
    Panel::hstack()
        .id_salt("ports")
        .size((Sizing::FILL, Sizing::Hug))
        .show(ui, |ui| {
            input_column(ui, ctx, node_id, func, inputs, bindings, port_frame, out);
            output_column(ui, ctx, node_id, outputs, port_frame);
        });
}

#[allow(clippy::too_many_arguments)]
fn input_column(
    ui: &mut Ui,
    ctx: &AppContext<'_>,
    node_id: NodeId,
    func: Option<&Func>,
    names: &[InternedStr],
    bindings: &[InputBindingView],
    port_frame: &PortFrame,
    out: &mut Vec<Intent>,
) {
    let (idle, hover) = (ctx.theme.input_port, ctx.theme.input_port_hover);
    Panel::vstack()
        .id_salt("in")
        .size((Sizing::Fill(1.0), Sizing::Hug))
        .padding(Spacing::new(0.0, PORT_COL_PAD_TOP, 0.0, PORT_COL_PAD_TOP))
        .gap(PORT_GAP)
        .child_align(Align::h(HAlign::Left))
        .show(ui, |ui| {
            for (i, name) in names.iter().enumerate() {
                let port = PortRef {
                    node_id,
                    kind: PortKind::Input,
                    port_idx: i,
                };
                let fill = if port_frame.is_hovered(port) {
                    hover
                } else {
                    idle
                };
                let binding = bindings.get(i).unwrap_or(&InputBindingView::None);
                let func_input = func.and_then(|f| f.inputs.get(i));
                input_port_row(ui, port, name.clone(), fill, binding, func_input, out);
            }
        });
}

fn output_column(
    ui: &mut Ui,
    ctx: &AppContext<'_>,
    node_id: NodeId,
    names: &[InternedStr],
    port_frame: &PortFrame,
) {
    let (idle, hover) = (ctx.theme.output_port, ctx.theme.output_port_hover);
    Panel::vstack()
        .id_salt("out")
        .size((Sizing::Fill(1.0), Sizing::Hug))
        .padding(Spacing::new(0.0, PORT_COL_PAD_TOP, 0.0, PORT_COL_PAD_TOP))
        .gap(PORT_GAP)
        .child_align(Align::h(HAlign::Right))
        .show(ui, |ui| {
            for (i, name) in names.iter().enumerate() {
                let port = PortRef {
                    node_id,
                    kind: PortKind::Output,
                    port_idx: i,
                };
                let fill = if port_frame.is_hovered(port) {
                    hover
                } else {
                    idle
                };
                output_port_row(ui, port, name.clone(), fill);
            }
        });
}

/// Stable widget id for one port circle. Derived from
/// `(node_id, kind, port_idx)` so prepass can look up
/// `response_for(port_circle_wid(..))` without threading the cache —
/// every port's id is reconstructible from its domain coordinates.
pub fn port_circle_wid(port: PortRef) -> WidgetId {
    WidgetId::from_hash((
        "graph.node.port_circle",
        port.node_id,
        port.kind as u8,
        port.port_idx,
    ))
}

/// One output port = label + circle, vertically centered. Circle has a
/// negative right margin so it overhangs the column. The circle's
/// `WidgetId` is the deterministic `port_circle_wid(port)`, so
/// downstream consumers (`PortFrame::rebuild`, snap, draw) reconstruct
/// it from domain coords without threading any cache.
fn output_port_row(ui: &mut Ui, port: PortRef, name: InternedStr, fill: Color) {
    let wid = port_circle_wid(port);
    Panel::hstack()
        .id_salt(("port", port.port_idx))
        .size((Sizing::Hug, Sizing::Hug))
        .gap(4.0)
        .child_align(Align::v(VAlign::Center))
        .show(ui, |ui| {
            Text::new(name).show(ui);
            circle_frame(ui, wid, fill, Spacing::new(0.0, 0.0, -PORT_RADIUS, 0.0));
        });
}

#[allow(clippy::too_many_arguments)]
fn input_port_row(
    ui: &mut Ui,
    port: PortRef,
    name: InternedStr,
    fill: Color,
    binding: &InputBindingView,
    func_input: Option<&FuncInput>,
    out: &mut Vec<Intent>,
) {
    let margin = Spacing::new(-PORT_RADIUS, 0.0, 0.0, 0.0);
    let wid = port_circle_wid(port);
    let row = Panel::hstack()
        .id_salt(("port", port.port_idx))
        .size((Sizing::Hug, Sizing::Hug))
        .sense(Sense::CLICK)
        .gap(4.0)
        .child_align(Align::v(VAlign::Center))
        .show(ui, |ui| {
            circle_frame(ui, wid, fill, margin);
            Text::new(name.clone()).show(ui);
            if let InputBindingView::Const(value) = binding {
                let editor_id =
                    WidgetId::from_hash(("graph.node.const_editor", port.node_id, port.port_idx));
                if let Some(new_value) = value_editor::show(ui, editor_id, value) {
                    out.push(set_input(port, Binding::Const(new_value)));
                }
            }
        });
    // Open on right-click anywhere on the row — circle, label, or
    // editor. The circle has its own `Sense::CLICK` and consumes hits
    // over its rect, so the row's snapshot alone misses clicks landing
    // on the circle (no event bubbling in palantir's hit-test).
    let menu_id = row.response.widget_id();
    let row_secondary = row.response.secondary_clicked();
    let circle_state = ui.response_for(wid);
    if (row_secondary || circle_state.secondary_clicked)
        && let Some(p) = ui.pointer_pos()
    {
        ContextMenu::open(ui, menu_id, p);
    }
    // Double-click on the port circle clears the binding (mirrors the
    // deprecated-darkroom gesture). Palantir tracks the two-click edge
    // per-button via `ResponseState::double_clicked()`.
    if circle_state.double_clicked() {
        out.push(set_input(port, Binding::None));
    }
    let default_static = func_input.map(default_static_value);
    ContextMenu::for_id(menu_id)
        .size((Sizing::Hug, Sizing::Hug))
        .show(ui, |ui, popup| {
            let can_set =
                !matches!(binding, InputBindingView::Const(_)) && default_static.is_some();
            if MenuItem::new("Set constant")
                .enabled(can_set)
                .show(ui, popup)
                .clicked()
                && let Some(value) = default_static.clone()
            {
                out.push(set_input(port, Binding::Const(value)));
            }
            if MenuItem::new("Clear binding")
                .enabled(!matches!(binding, InputBindingView::None))
                .show(ui, popup)
                .clicked()
            {
                out.push(set_input(port, Binding::None));
            }
        });
}

fn set_input(port: PortRef, to: Binding) -> Intent {
    Intent::SetInput {
        node_id: port.node_id,
        input_idx: port.port_idx,
        to,
    }
}

/// Default `StaticValue` for a function input — its declared
/// `default_value` if any, otherwise the zero/empty of its `DataType`.
fn default_static_value(func_input: &FuncInput) -> StaticValue {
    func_input
        .default_value
        .clone()
        .unwrap_or_else(|| StaticValue::from(&func_input.data_type))
}

fn circle_frame(ui: &mut Ui, wid: WidgetId, fill: Color, margin: Spacing) {
    // Explicit `id(wid)` so the cross-frame id stays stable: prepass
    // computes the same `port_circle_wid` and reads its response,
    // record paints with the same id — no drift even if the parent
    // structure shifts. CLICK | DRAG so the port (a) intercepts the
    // press before it falls through to the node body's `Sense::DRAG`,
    // and (b) can latch a connection drag.
    Frame::new()
        .id(wid)
        .size((Sizing::Fixed(PORT_SIZE), Sizing::Fixed(PORT_SIZE)))
        .margin(margin)
        .sense(Sense::CLICK | Sense::DRAG)
        .background(Background {
            fill: fill.into(),
            corners: Corners::all(PORT_RADIUS),
            ..Default::default()
        })
        .show(ui);
}
