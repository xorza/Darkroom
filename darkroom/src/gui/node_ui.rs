use crate::frame_result::FrameResult;
use crate::gui::graph_ui::PortCache;
use crate::gui::{NODE_W, PORT_COL_PAD_TOP, PORT_GAP, PORT_RADIUS, PORT_SIZE, Side};
use crate::intent::Intent;
use crate::scene::{Scene, SceneNode};
use common::Span;
use glam::Vec2;
use palantir::{
    Align, Background, Color, Configure, Corners, Frame, HAlign, InternedStr, Panel, Response,
    Sense, Sizing, Spacing, Stroke, Text, Ui, VAlign, WidgetId,
};
use scenarium::prelude::NodeId;

const NODE_FILL: u32 = 0x2d2d33;
const NODE_BORDER: u32 = 0x5a5a66;
const HEADER_FILL: u32 = 0x3a3a44;
const INPUT_COLOR: u32 = 0x77c97a;
const OUTPUT_COLOR: u32 = 0xe39a4a;

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
        scene: &Scene,
        ports: &mut PortCache,
        out: &mut FrameResult,
    ) {
        for n in &scene.nodes {
            let spans = self.draw_one(ui, scene, n, &mut ports.widget_ids, out);
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
        scene: &Scene,
        node: &SceneNode,
        widget_ids: &mut Vec<WidgetId>,
        _out: &mut FrameResult,
    ) -> NodePortSpans {
        let inputs = scene.ports(node.inputs);
        let outputs = scene.ports(node.outputs);

        let mut spans = NodePortSpans::default();
        let response = Panel::vstack()
            .id_salt(("graph.node", node.id))
            .position(node.pos)
            .size((Sizing::Fixed(NODE_W), Sizing::Hug))
            .sense(Sense::DRAG)
            .background(Background {
                fill: Color::hex(NODE_FILL).into(),
                stroke: Stroke::solid(Color::hex(NODE_BORDER), 1.0),
                radius: Corners::all(6.0),
                ..Default::default()
            })
            .show(ui, |ui| {
                header(ui, node.name.clone());
                spans = ports_row(ui, inputs, outputs, widget_ids);
            });

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
    pub fn prepass(&self, ui: &Ui, out: &mut FrameResult) {
        if let Some(anchor) = self.drag_anchor
            && let Some(delta) = ui.response_for(anchor.widget_id).drag_delta
        {
            out.push(Intent::MoveNode {
                node_id: anchor.node_id,
                to: anchor.pos + delta,
            });
        }
    }
}

fn header(ui: &mut Ui, name: InternedStr) {
    Panel::vstack()
        .id_salt("header")
        .size((Sizing::FILL, Sizing::Hug))
        .padding(Spacing::xy(8.0, 4.0))
        .background(Background {
            fill: Color::hex(HEADER_FILL).into(),
            radius: Corners::new(6.0, 6.0, 0.0, 0.0),
            ..Default::default()
        })
        .show(ui, |ui| {
            Text::new(name).show(ui);
        });
}

fn ports_row(
    ui: &mut Ui,
    inputs: &[InternedStr],
    outputs: &[InternedStr],
    widget_ids: &mut Vec<WidgetId>,
) -> NodePortSpans {
    let mut spans = NodePortSpans::default();
    Panel::hstack()
        .id_salt("ports")
        .size((Sizing::FILL, Sizing::Hug))
        .show(ui, |ui| {
            let start_in = widget_ids.len() as u32;
            port_column(ui, "in", inputs, Side::Left, widget_ids);
            let len_in = widget_ids.len() as u32 - start_in;
            let start_out = widget_ids.len() as u32;
            port_column(ui, "out", outputs, Side::Right, widget_ids);
            let len_out = widget_ids.len() as u32 - start_out;
            spans = NodePortSpans {
                inputs: Span::new(start_in, len_in),
                outputs: Span::new(start_out, len_out),
            };
        });
    spans
}

fn port_column(
    ui: &mut Ui,
    salt: &'static str,
    names: &[InternedStr],
    side: Side,
    widget_ids: &mut Vec<WidgetId>,
) {
    Panel::vstack()
        .id_salt(salt)
        .size((Sizing::Fill(1.0), Sizing::Hug))
        .padding(Spacing::new(0.0, PORT_COL_PAD_TOP, 0.0, PORT_COL_PAD_TOP))
        .gap(PORT_GAP)
        .child_align(match side {
            Side::Left => Align::h(HAlign::Left),
            Side::Right => Align::h(HAlign::Right),
        })
        .show(ui, |ui| {
            for (i, name) in names.iter().enumerate() {
                widget_ids.push(port_row(ui, i, name.clone(), side));
            }
        });
}

/// One port = circle + label, vertically centered. Circle on the outer
/// edge (with negative margin so it overhangs the column), label on
/// the inner side. Returns the circle's stable `WidgetId` so the
/// canvas-level `draw_connections` can resolve its world rect on
/// demand via `Ui::response_for`.
fn port_row(ui: &mut Ui, i: usize, name: InternedStr, side: Side) -> WidgetId {
    let (fill, margin) = match side {
        Side::Left => (
            Color::hex(INPUT_COLOR),
            Spacing::new(-PORT_RADIUS, 0.0, 0.0, 0.0),
        ),
        Side::Right => (
            Color::hex(OUTPUT_COLOR),
            Spacing::new(0.0, 0.0, -PORT_RADIUS, 0.0),
        ),
    };
    let mut wid = WidgetId::default();
    Panel::hstack()
        .id_salt(("port", i))
        .size((Sizing::Hug, Sizing::Hug))
        .gap(4.0)
        .child_align(Align::v(VAlign::Center))
        .show(ui, |ui| {
            let circle = |ui: &mut Ui| circle_frame(ui, fill, margin);
            let label = |ui: &mut Ui| {
                Text::new(name.clone()).show(ui);
            };
            let circle_resp = match side {
                Side::Left => {
                    let r = circle(ui);
                    label(ui);
                    r
                }
                Side::Right => {
                    label(ui);
                    circle(ui)
                }
            };
            wid = circle_resp.widget_id();
        });
    wid
}

fn circle_frame(ui: &mut Ui, fill: Color, margin: Spacing) -> Response {
    // Explicit `id_salt` instead of `auto_id`: every port circle
    // shares the same `#[track_caller]` site (this function), so
    // `auto_id` collides across siblings → `SeenIds::record`
    // disambiguates, but `Frame::show` reads `response_for` with the
    // pre-disambiguation id and gets `None` back. The parent port
    // row already has a unique `id_salt(("port", i))`, so
    // `parent.with("circle")` is unique per port.
    //
    // Port circles sense CLICK so a press lands on the port and does
    // not fall through to the parent node panel — that's what keeps
    // node-drag from latching when the user grabs a port.
    Frame::new()
        .id_salt("circle")
        .size((Sizing::Fixed(PORT_SIZE), Sizing::Fixed(PORT_SIZE)))
        .margin(margin)
        .sense(Sense::CLICK)
        .background(Background {
            fill: fill.into(),
            radius: Corners::all(PORT_RADIUS),
            ..Default::default()
        })
        .show(ui)
}
