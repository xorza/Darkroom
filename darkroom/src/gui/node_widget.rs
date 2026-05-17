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

/// World-space centers of each port circle, indexed positionally
/// (matches `Node.inputs[i]` / `Func.outputs[i]`). `None` on the
/// first frame a port is visible (no prior-frame layout yet) and
/// during the frame a node's `view_node.pos` changes (the rect read
/// here lags layout by one frame).
pub struct NodePorts {
    pub inputs: Vec<Option<Vec2>>,
    pub outputs: Vec<Option<Vec2>>,
}

/// One graph node, composed: header band + two-column port grid.
/// Returns the laid-out port centers (read from the prior-frame
/// cascade snapshot via `Response::rect`) so the caller can wire
/// connection beziers exactly to the rendered circles.
pub fn draw(ui: &mut Ui, scene: &Scene, node: &SceneNode) -> NodePorts {
    let mut ports = NodePorts {
        inputs: Vec::new(),
        outputs: Vec::new(),
    };
    let inputs = scene.ports(node.inputs);
    let outputs = scene.ports(node.outputs);
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
            ports = ports_row(ui, inputs, outputs);
        });
    ports
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

fn ports_row(ui: &mut Ui, inputs: &[InternedStr], outputs: &[InternedStr]) -> NodePorts {
    let mut np = NodePorts {
        inputs: Vec::new(),
        outputs: Vec::new(),
    };
    Panel::hstack()
        .id_salt("ports")
        .size((Sizing::FILL, Sizing::Hug))
        .show(ui, |ui| {
            np.inputs = port_column(ui, "in", inputs, Side::Left);
            np.outputs = port_column(ui, "out", outputs, Side::Right);
        });
    np
}

fn port_column(
    ui: &mut Ui,
    salt: &'static str,
    names: &[InternedStr],
    side: Side,
) -> Vec<Option<Vec2>> {
    let mut centers = Vec::with_capacity(names.len());
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
                centers.push(port_row(ui, i, name.clone(), side));
            }
        });
    centers
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
