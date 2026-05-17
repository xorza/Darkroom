use palantir::{
    Align, Background, Color, Configure, Corners, Frame, HAlign, Panel, Sizing, Spacing, Stroke,
    Text, Ui, VAlign,
};
use scenarium::prelude::{Func, Node};

use crate::gui::{HEADER_H, NODE_W, PORT_COL_PAD_TOP, PORT_GAP, PORT_RADIUS, PORT_SIZE, Side};
use crate::model::ViewNode;

const NODE_FILL: u32 = 0x2d2d33;
const NODE_BORDER: u32 = 0x5a5a66;
const HEADER_FILL: u32 = 0x3a3a44;
const INPUT_COLOR: u32 = 0x77c97a;
const OUTPUT_COLOR: u32 = 0xe39a4a;

/// One graph node, composed: header band + two-column port grid.
/// Both port columns are `Fill(1)` so they split the body evenly;
/// each port is a small `Frame` with a negative left/right margin so
/// the circle straddles the node's side instead of sitting flush
/// against the inside edge.
pub fn draw(ui: &mut Ui, view_node: &ViewNode, node: &Node, func: &Func) {
    Panel::vstack()
        .id_salt(("graph.node", view_node.id))
        .position(view_node.pos)
        .size((Sizing::Fixed(NODE_W), Sizing::Hug))
        .background(Background {
            fill: Color::hex(NODE_FILL).into(),
            stroke: Stroke::solid(Color::hex(NODE_BORDER), 1.0),
            radius: Corners::all(6.0),
            ..Default::default()
        })
        .show(ui, |ui| {
            header(ui, &node.name);
            let input_names: Vec<&str> = node.inputs.iter().map(|i| i.name.as_str()).collect();
            let output_names: Vec<&str> = func.outputs.iter().map(|o| o.name.as_str()).collect();
            ports_row(ui, &input_names, &output_names);
        });
}

fn header(ui: &mut Ui, name: &str) {
    Panel::vstack()
        .id_salt("header")
        .size((Sizing::FILL, Sizing::Fixed(HEADER_H)))
        .padding(Spacing::xy(8.0, 4.0))
        .background(Background {
            fill: Color::hex(HEADER_FILL).into(),
            radius: Corners::new(6.0, 6.0, 0.0, 0.0),
            ..Default::default()
        })
        .show(ui, |ui| {
            Text::new(name.to_string()).show(ui);
        });
}

fn ports_row(ui: &mut Ui, inputs: &[&str], outputs: &[&str]) {
    Panel::hstack()
        .id_salt("ports")
        .size((Sizing::FILL, Sizing::Hug))
        .show(ui, |ui| {
            port_column(ui, "in", inputs, Side::Left);
            port_column(ui, "out", outputs, Side::Right);
        });
}

fn port_column(ui: &mut Ui, salt: &'static str, names: &[&str], side: Side) {
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
                port_row(ui, i, name, side);
            }
        });
}

/// One port = circle + label, vertically centered. Circle on the outer
/// edge (with negative margin so it overhangs the column), label on
/// the inner side.
fn port_row(ui: &mut Ui, i: usize, name: &str, side: Side) {
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
    Panel::hstack()
        .id_salt(("port", i))
        .size((Sizing::Hug, Sizing::Hug))
        .gap(4.0)
        .child_align(Align::v(VAlign::Center))
        .show(ui, |ui| {
            let circle = |ui: &mut Ui| {
                Frame::new()
                    .auto_id()
                    .size((Sizing::Fixed(PORT_SIZE), Sizing::Fixed(PORT_SIZE)))
                    .margin(margin)
                    .background(Background {
                        fill: fill.into(),
                        radius: Corners::all(PORT_RADIUS),
                        ..Default::default()
                    })
                    .show(ui);
            };
            let label = |ui: &mut Ui| {
                Text::new(name.to_string()).show(ui);
            };
            match side {
                Side::Left => {
                    circle(ui);
                    label(ui);
                }
                Side::Right => {
                    label(ui);
                    circle(ui);
                }
            }
        });
}
