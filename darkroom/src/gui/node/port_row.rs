//! The two port columns of a node body: one labeled row per port, each
//! with its grab/snap circle, plus the inline const editor on bound
//! inputs and the right-click binding menu. Drawn below the header by
//! [`crate::gui::node::NodeUI`]; the boundary-port rename affordance
//! lives in [`crate::gui::node::port_rename`].

use palantir::{
    Align, Color, Configure, ContextMenu, Corners, HAlign, InternedStr, MenuItem, Panel, Rect,
    Sense, Shape, Sizing, Spacing, Stroke, Ui, VAlign, WidgetId,
};
use scenarium::data::{DataType, StaticValue};
use scenarium::graph::Binding;
use scenarium::prelude::NodeId;

use crate::core::document::BoundarySide;
use crate::core::edit::intent::Intent;
use crate::gui::node::port_color::port_color;
use crate::gui::node::port_rename::port_label;
use crate::gui::node::value_editor;
use crate::gui::node::{RecordCtx, set_input};
use crate::gui::scene::{InputBindingView, SceneNode};
use crate::gui::theme::Theme;
use crate::gui::{PortKind, PortRef};

pub(crate) fn ports_row(ui: &mut Ui, rcx: RecordCtx<'_>, node: &SceneNode, out: &mut Vec<Intent>) {
    let theme = rcx.theme;
    Panel::hstack()
        .id_salt("ports")
        .size((Sizing::FILL, Sizing::Hug))
        .padding(Spacing::xy(theme.port_col_pad_x, 0.0))
        .gap(theme.port_cols_gap)
        .show(ui, |ui| {
            input_column(ui, rcx, node, out);
            output_column(ui, rcx, node, out);
        });
}

fn input_column(ui: &mut Ui, rcx: RecordCtx<'_>, node: &SceneNode, out: &mut Vec<Intent>) {
    let names = rcx.scene.input_names(node.inputs);
    let bindings = rcx.scene.bindings(node.inputs);
    let defaults = rcx.scene.defaults(node.inputs);
    let types = rcx.scene.input_types(node.inputs);
    let theme = rcx.theme;
    // Boundary (`SubgraphInput`/`SubgraphOutput`) ports route the
    // interface, not literal values — no const affordance.
    let allow_const = !node.boundary;
    Panel::vstack()
        .id_salt("in")
        .size((Sizing::Fill(1.0), Sizing::Hug))
        .padding(Spacing::new(
            0.0,
            theme.port_col_pad_top,
            0.0,
            theme.port_col_pad_top,
        ))
        .gap(theme.port_gap)
        .child_align(Align::h(HAlign::Left))
        .show(ui, |ui| {
            for (i, name) in names.iter().enumerate() {
                let port = PortRef {
                    node_id: node.id,
                    kind: PortKind::Input,
                    port_idx: i,
                };
                let binding = bindings.get(i).unwrap_or(&InputBindingView::None);
                let default = defaults.get(i).cloned().flatten();
                let data_type = types.get(i).cloned().unwrap_or_default();
                // A `SubgraphOutput` boundary node's input ports are the
                // subgraph's *outputs* — renameable, except the trailing
                // "+" placeholder.
                let rename = (node.boundary && i + 1 < names.len()).then_some(BoundarySide::Output);
                input_port_row(
                    ui,
                    rcx,
                    port,
                    name.clone(),
                    binding,
                    default,
                    data_type,
                    allow_const,
                    rename,
                    out,
                );
            }
        });
}

fn output_column(ui: &mut Ui, rcx: RecordCtx<'_>, node: &SceneNode, out: &mut Vec<Intent>) {
    let names = rcx.scene.output_names(node.outputs);
    let types = rcx.scene.output_types(node.outputs);
    let theme = rcx.theme;
    Panel::vstack()
        .id_salt("out")
        .size((Sizing::Fill(1.0), Sizing::Hug))
        .padding(Spacing::new(
            0.0,
            theme.port_col_pad_top,
            0.0,
            theme.port_col_pad_top,
        ))
        .gap(theme.port_gap)
        .child_align(Align::h(HAlign::Right))
        .show(ui, |ui| {
            for (i, name) in names.iter().enumerate() {
                let port = PortRef {
                    node_id: node.id,
                    kind: PortKind::Output,
                    port_idx: i,
                };
                let data_type = types.get(i).cloned().unwrap_or_default();
                // A `SubgraphInput` boundary node's output ports are the
                // subgraph's *inputs* — renameable, except the trailing
                // "+" placeholder.
                let rename = (node.boundary && i + 1 < names.len()).then_some(BoundarySide::Input);
                output_port_row(ui, rcx, port, name.clone(), data_type, rename, out);
            }
        });
}

/// Stable widget id for one port circle. Derived from
/// `(node_id, kind, port_idx)` so prepass can look up
/// `response_for(port_circle_wid(..))` without threading the cache —
/// every port's id is reconstructible from its domain coordinates.
pub(crate) fn port_circle_wid(port: PortRef) -> WidgetId {
    WidgetId::from_hash((
        "graph.node.port_circle",
        port.node_id,
        port.kind as u8,
        port.port_idx,
    ))
}

/// Stable widget id for an input port's inline const editor (text field,
/// checkbox, or file-pick button). Reconstructible from domain coords so
/// the path-pick scan can poll the button's click without threading state.
pub(crate) fn const_editor_wid(node_id: NodeId, port_idx: usize) -> WidgetId {
    WidgetId::from_hash(("graph.node.const_editor", node_id, port_idx))
}

/// One output port = label + circle, vertically centered. Circle has a
/// negative right margin so it overhangs the column. The circle's
/// `WidgetId` is the deterministic `port_circle_wid(port)`, so
/// downstream consumers (`PortFrame::rebuild`, snap, draw) reconstruct
/// it from domain coords without threading any cache.
fn output_port_row(
    ui: &mut Ui,
    rcx: RecordCtx<'_>,
    port: PortRef,
    name: InternedStr,
    data_type: DataType,
    rename: Option<BoundarySide>,
    out: &mut Vec<Intent>,
) {
    let theme = rcx.theme;
    let fill = port_color(
        theme,
        &data_type,
        PortKind::Output,
        rcx.port_frame.is_hovered(port),
    );
    let wid = port_circle_wid(port);
    let overhang = theme.port_radius() + theme.port_col_pad_x;
    Panel::hstack()
        .id_salt(("port", port.port_idx))
        .size((Sizing::Hug, Sizing::Hug))
        .gap(4.0)
        .child_align(Align::v(VAlign::Center))
        .show(ui, |ui| {
            port_label(ui, rcx, port, name, rename, out);
            circle_frame(ui, theme, wid, fill, Spacing::new(0.0, 0.0, -overhang, 0.0));
        });
    // Double-click to disconnect every consumer is handled in
    // `emit_port_disconnects` (prepass) alongside the input-side gesture.
}

#[allow(clippy::too_many_arguments)]
fn input_port_row(
    ui: &mut Ui,
    rcx: RecordCtx<'_>,
    port: PortRef,
    name: InternedStr,
    binding: &InputBindingView,
    default_static: Option<StaticValue>,
    data_type: DataType,
    allow_const: bool,
    rename: Option<BoundarySide>,
    out: &mut Vec<Intent>,
) {
    let theme = rcx.theme;
    let fill = port_color(
        theme,
        &data_type,
        PortKind::Input,
        rcx.port_frame.is_hovered(port),
    );
    let overhang = theme.port_radius() + theme.port_col_pad_x;
    let margin = Spacing::new(-overhang, 0.0, 0.0, 0.0);
    let wid = port_circle_wid(port);
    let row = Panel::hstack()
        .id_salt(("port", port.port_idx))
        .size((Sizing::Hug, Sizing::Hug))
        .sense(Sense::CLICK)
        .gap(4.0)
        .child_align(Align::v(VAlign::Center))
        .show(ui, |ui| {
            circle_frame(ui, theme, wid, fill, margin);
            port_label(ui, rcx, port, name.clone(), rename, out);
            if allow_const && let InputBindingView::Const(value) = binding {
                let editor_id = const_editor_wid(port.node_id, port.port_idx);
                if let Some(new_value) =
                    value_editor::show(ui, &theme.static_value_editor, editor_id, value, &data_type)
                {
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
    // Double-click on the circle clears the binding — handled in
    // `emit_port_disconnects` (prepass), since clearing a `Const` resizes
    // the node and the wires must re-anchor before the record.
    ContextMenu::for_id(menu_id)
        .size((Sizing::Hug, Sizing::Hug))
        .show(ui, |ui, popup| {
            let can_set = allow_const
                && !matches!(binding, InputBindingView::Const(_))
                && default_static.is_some();
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

/// Hover / grab box scaled past the painted dot so ports are easier to
/// hit and snap to, while the visible circle stays `port_size`.
const PORT_HIT_SCALE: f32 = 1.8;

fn circle_frame(ui: &mut Ui, theme: &Theme, wid: WidgetId, fill: Color, margin: Spacing) {
    let port = theme.port_size;
    let hit = port * PORT_HIT_SCALE;
    let inset = (hit - port) * 0.5;

    // The sensing element is `hit`-sized, but the extra (`inset` on each
    // side) is pulled back out of the layout with negative margin, so
    // node layout and the dot's position are unchanged — only the
    // hover/grab area grows. The dot itself paints as a centered shape.
    let [l, t, r, b] = margin.as_array();
    let hit_margin = Spacing::new(l - inset, t - inset, r - inset, b - inset);
    let radius = theme.port_radius();

    // Explicit `id(wid)` so the cross-frame id stays stable: prepass
    // computes the same `port_circle_wid` and reads its response,
    // record paints with the same id — no drift even if the parent
    // structure shifts. CLICK | DRAG so the port (a) intercepts the
    // press before it falls through to the node body's `Sense::DRAG`,
    // and (b) can latch a connection drag.
    Panel::zstack()
        .id(wid)
        .size((Sizing::Fixed(hit), Sizing::Fixed(hit)))
        .margin(hit_margin)
        .sense(Sense::CLICK | Sense::DRAG)
        .show(ui, |ui| {
            ui.add_shape(Shape::RoundedRect {
                local_rect: Some(Rect::new(inset, inset, port, port)),
                corners: Corners::all(radius),
                fill: fill.into(),
                stroke: Stroke::ZERO,
            });
        });
}
