use crate::gui::{NODE_W, PORT_COL_PAD_TOP, PORT_GAP, PORT_RADIUS, PORT_SIZE, Side};
use crate::scene::{Scene, SceneNode};
use glam::Vec2;
use palantir::{
    Align, Background, Color, Configure, Corners, Frame, HAlign, InternedStr, Panel, Rect,
    Response, Sizing, Spacing, Stroke, Text, Ui, VAlign,
};

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
#[derive(Clone, Copy, Default)]
pub struct NodePortSpans {
    pub inputs: PortSpan,
    pub outputs: PortSpan,
}

#[derive(Clone, Copy, Default)]
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

/// One graph node, composed: header band + two-column port grid.
/// Pushes per-port world-space circle centers into `centers` and
/// returns the spans covering this node's input and output rows.
/// Returns `None` if any port failed to resolve a layout rect — the
/// caller should `centers.truncate(start)` to drop the partial push so
/// the pool indexes stay tight.
pub fn draw(
    ui: &mut Ui,
    scene: &Scene,
    node: &SceneNode,
    centers: &mut Vec<Vec2>,
) -> Option<NodePortSpans> {
    let inputs = scene.ports(node.inputs);
    let outputs = scene.ports(node.outputs);
    let mut spans = None;
    Panel::vstack()
        .id_salt(("graph.node", node.id))
        .position(node.pos)
        .size((Sizing::Fixed(NODE_W), Sizing::Hug))
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
    spans
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
            } else {
                ui.request_relayout();
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
    Frame::new()
        .id_salt("circle")
        .size((Sizing::Fixed(PORT_SIZE), Sizing::Fixed(PORT_SIZE)))
        .margin(margin)
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
