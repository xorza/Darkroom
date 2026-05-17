use crate::frame_result::FrameResult;
use crate::gui::graph_ui::PortCache;
use crate::gui::{NODE_W, PORT_COL_PAD_TOP, PORT_GAP, PORT_RADIUS, PORT_SIZE, Side};
use crate::intent::Intent;
use crate::scene::{Scene, SceneNode};
use glam::Vec2;
use palantir::{
    Align, Background, Color, Configure, Corners, Frame, HAlign, InternedStr, Panel, Rect,
    Response, Sense, Sizing, Spacing, Stroke, Text, Ui, VAlign,
};
use scenarium::prelude::NodeId;

const NODE_FILL: u32 = 0x2d2d33;
const NODE_BORDER: u32 = 0x5a5a66;
const HEADER_FILL: u32 = 0x3a3a44;
const INPUT_COLOR: u32 = 0x77c97a;
const OUTPUT_COLOR: u32 = 0xe39a4a;

/// Per-node slices into the flat `PortCache.centers` pool — one for
/// inputs, one for outputs. Indexing matches positional `Node.inputs[i]`
/// / `Func.outputs[i]`. A node only earns an entry in `PortCache.nodes`
/// after every port resolved a layout rect (frame 2+); first-frame
/// nodes simply don't appear in the cache, and `draw_connections`
/// skips them via `nodes.get(&id)` returning `None`.
#[derive(Clone, Copy, Default, Debug)]
pub struct NodePortSpans {
    pub inputs: PortSpan,
    pub outputs: PortSpan,
}

#[derive(Clone, Copy, Default, Debug)]
pub struct PortSpan {
    pub start: u32,
    pub len: u32,
}

impl PortSpan {
    pub fn get(self, pool: &[Vec2], idx: usize) -> Option<Vec2> {
        if idx >= self.len as usize {
            return None;
        }
        Some(pool[self.start as usize + idx])
    }
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
            if let Some(spans) = self.draw_one(ui, scene, n, &mut ports.centers, out) {
                ports.nodes.insert(n.id, spans);
            }
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
        centers: &mut Vec<Vec2>,
        out: &mut FrameResult,
    ) -> Option<NodePortSpans> {
        let inputs = scene.ports(node.inputs);
        let outputs = scene.ports(node.outputs);
        let mut spans = None;
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
                spans = ports_row(ui, inputs, outputs, centers);
            });

        if response.drag_started() {
            self.drag_anchor = Some(DragAnchor {
                node_id: node.id,
                pos: node.pos,
            });
        }
        if let (Some(delta), Some(anchor)) = (response.drag_delta(), self.drag_anchor)
            && anchor.node_id == node.id
        {
            out.push(Intent::MoveNode {
                node_id: node.id,
                to: anchor.pos + delta,
            });
        }

        spans
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
    centers: &mut Vec<Vec2>,
) -> Option<NodePortSpans> {
    let node_start = centers.len();
    let mut spans = None;
    Panel::hstack()
        .id_salt("ports")
        .size((Sizing::FILL, Sizing::Hug))
        .show(ui, |ui| {
            let start_in = centers.len() as u32;
            let in_ok = port_column(ui, "in", inputs, Side::Left, centers);
            let len_in = centers.len() as u32 - start_in;
            let start_out = centers.len() as u32;
            let out_ok = port_column(ui, "out", outputs, Side::Right, centers);
            let len_out = centers.len() as u32 - start_out;
            if in_ok && out_ok {
                spans = Some(NodePortSpans {
                    inputs: PortSpan {
                        start: start_in,
                        len: len_in,
                    },
                    outputs: PortSpan {
                        start: start_out,
                        len: len_out,
                    },
                });
            }
        });
    if spans.is_none() {
        centers.truncate(node_start);
    }
    spans
}

fn port_column(
    ui: &mut Ui,
    salt: &'static str,
    names: &[InternedStr],
    side: Side,
    centers: &mut Vec<Vec2>,
) -> bool {
    let mut all_ok = true;
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
                match port_row(ui, i, name.clone(), side) {
                    Some(c) => centers.push(c),
                    None => {
                        all_ok = false;
                        // Placeholder so positional indexing within this
                        // column stays aligned for any siblings that
                        // *did* resolve; caller truncates centers back
                        // to the node's start position when bailing.
                        centers.push(Vec2::ZERO);
                    }
                }
            }
        });
    all_ok
}

/// One port = circle + label, vertically centered. Circle on the outer
/// edge (with negative margin so it overhangs the column), label on
/// the inner side. Returns the circle's center in world coords from
/// the prior-frame layout.
fn port_row(ui: &mut Ui, i: usize, name: InternedStr, side: Side) -> Option<Vec2> {
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
    let mut center = None;
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
            center = rect_center(circle_resp.rect());
        });
    center
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

fn rect_center(r: Option<Rect>) -> Option<Vec2> {
    r.map(|r| r.min + Vec2::new(r.size.w * 0.5, r.size.h * 0.5))
}
