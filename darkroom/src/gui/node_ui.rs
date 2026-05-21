use crate::app::AppContext;
use crate::frame_result::FrameResult;
use crate::gui::breaker::BreakerProbe;
use crate::gui::graph_ui::PortCache;
use crate::gui::{NODE_W, PORT_COL_PAD_TOP, PORT_GAP, PORT_RADIUS, PORT_SIZE, PortKind};
use crate::intent::Intent;
use crate::scene::{Scene, SceneNode};
use common::Span;
use glam::Vec2;
use palantir::{
    Align, Background, Color, Configure, Corners, Frame, HAlign, InternedStr, Panel, Rect,
    Response, Sense, Sizing, Spacing, Stroke, Text, Ui, VAlign, WidgetId,
};
use scenarium::prelude::NodeId;

/// Per-node slices into the flat `PortCache.widget_ids` pool — one
/// `Span` for inputs, one for outputs. Indexing matches positional
/// `Node.inputs[i]` / `Func.outputs[i]`. Read a port through
/// `pool[span.range()].get(i)`.
#[derive(Clone, Copy, Default, Debug)]
pub struct NodePortSpans {
    pub inputs: Span,
    pub outputs: Span,
}

/// Owns rendering of every graph node plus the single active drag
/// anchor — the press-frame `pos` is snapshotted here so each
/// `MoveNode` target is `anchor.pos + drag_delta`, not a running
/// integration over the moving source. Only one node can hold the
/// pointer at a time, so one anchor slot is enough.
///
/// `draw_all` is the single entry point; `GraphUI` calls it once per
/// frame with the scene and the port-center pool to fill.
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
        ports: &mut PortCache,
        probe: &mut BreakerProbe<'_>,
    ) {
        if let Some(b) = probe.state.as_deref_mut() {
            b.broken_nodes.clear();
        }
        for n in &scene.nodes {
            let spans = self.draw_one(ui, ctx, scene, n, &mut ports.widget_ids, probe);
            ports.nodes.insert(n.id, spans);
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

    fn draw_one(
        &mut self,
        ui: &mut Ui,
        ctx: &AppContext<'_>,
        scene: &Scene,
        node: &SceneNode,
        widget_ids: &mut Vec<WidgetId>,
        probe: &mut BreakerProbe<'_>,
    ) -> NodePortSpans {
        let inputs = scene.ports(node.inputs);
        let outputs = scene.ports(node.outputs);
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
                ports_row(ui, ctx, node.id, inputs, outputs, widget_ids)
            });
        let spans = panel.inner;
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

        spans
    }

    /// Pre-record pass: peek palantir's input state for any widgets
    /// this `NodeUI` owns and push the corresponding `Intent`s into
    /// `out`. Runs before `Scene::rebuild` in `App::frame`, so any
    /// state mutation applied from these intents (notably drag-driven
    /// `MoveNode`) lands in `Document` before recording — Pass A's
    /// arrange already reflects the cursor; no Pass B relayout retry.
    pub fn prepass(&mut self, ui: &Ui, scene: &Scene, out: &mut FrameResult) {
        let Some(anchor) = self.drag_anchor else {
            return;
        };
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
fn node_widget_id(node_id: NodeId) -> WidgetId {
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

fn ports_row(
    ui: &mut Ui,
    ctx: &AppContext<'_>,
    node_id: NodeId,
    inputs: &[InternedStr],
    outputs: &[InternedStr],
    widget_ids: &mut Vec<WidgetId>,
) -> NodePortSpans {
    Panel::hstack()
        .id_salt("ports")
        .size((Sizing::FILL, Sizing::Hug))
        .show(ui, |ui| {
            let start_in = widget_ids.len() as u32;
            port_column(ui, ctx, node_id, "in", inputs, PortKind::Input, widget_ids);
            let len_in = widget_ids.len() as u32 - start_in;
            let start_out = widget_ids.len() as u32;
            port_column(
                ui,
                ctx,
                node_id,
                "out",
                outputs,
                PortKind::Output,
                widget_ids,
            );
            let len_out = widget_ids.len() as u32 - start_out;
            NodePortSpans {
                inputs: Span::new(start_in, len_in),
                outputs: Span::new(start_out, len_out),
            }
        })
        .inner
}

fn port_column(
    ui: &mut Ui,
    ctx: &AppContext<'_>,
    node_id: NodeId,
    salt: &'static str,
    names: &[InternedStr],
    kind: PortKind,
    widget_ids: &mut Vec<WidgetId>,
) {
    let (idle, hover) = match kind {
        PortKind::Input => (ctx.theme.input_port, ctx.theme.input_port_hover),
        PortKind::Output => (ctx.theme.output_port, ctx.theme.output_port_hover),
    };
    Panel::vstack()
        .id_salt(salt)
        .size((Sizing::Fill(1.0), Sizing::Hug))
        .padding(Spacing::new(0.0, PORT_COL_PAD_TOP, 0.0, PORT_COL_PAD_TOP))
        .gap(PORT_GAP)
        .child_align(match kind {
            PortKind::Input => Align::h(HAlign::Left),
            PortKind::Output => Align::h(HAlign::Right),
        })
        .show(ui, |ui| {
            for (i, name) in names.iter().enumerate() {
                widget_ids.push(port_row(ui, node_id, i, name.clone(), kind, idle, hover));
            }
        });
}

/// Stable widget id for one port circle. Derived from
/// `(node_id, kind, port_idx)` so prepass can look up
/// `response_for(port_circle_wid(..))` without threading the cache —
/// every port's id is reconstructible from its domain coordinates.
pub fn port_circle_wid(node_id: NodeId, kind: PortKind, port_idx: usize) -> WidgetId {
    WidgetId::from_hash(("graph.node.port_circle", node_id, kind as u8, port_idx))
}

/// One port = circle + label, vertically centered. Circle on the outer
/// edge (with negative margin so it overhangs the column), label on
/// the inner kind. Returns the circle's stable `WidgetId` so the
/// canvas-level `draw_connections` can resolve its world rect on
/// demand via `Ui::response_for`.
fn port_row(
    ui: &mut Ui,
    node_id: NodeId,
    i: usize,
    name: InternedStr,
    kind: PortKind,
    fill: Color,
    fill_hover: Color,
) -> WidgetId {
    let margin = match kind {
        PortKind::Input => Spacing::new(-PORT_RADIUS, 0.0, 0.0, 0.0),
        PortKind::Output => Spacing::new(0.0, 0.0, -PORT_RADIUS, 0.0),
    };
    let wid = port_circle_wid(node_id, kind, i);
    let hovered = ui.response_for(wid).hovered;
    let color = if hovered { fill_hover } else { fill };
    Panel::hstack()
        .id_salt(("port", i))
        .size((Sizing::Hug, Sizing::Hug))
        .gap(4.0)
        .child_align(Align::v(VAlign::Center))
        .show(ui, |ui| match kind {
            PortKind::Input => {
                circle_frame(ui, wid, color, margin);
                Text::new(name.clone()).show(ui);
            }
            PortKind::Output => {
                Text::new(name.clone()).show(ui);
                circle_frame(ui, wid, color, margin);
            }
        });
    wid
}

fn circle_frame(ui: &mut Ui, wid: WidgetId, fill: Color, margin: Spacing) -> Response<'_> {
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
        .show(ui)
}
