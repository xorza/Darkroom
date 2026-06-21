//! The ports area of a node body, laid out as a grid: input port+label
//! (col 0), the inline const editor for that input (col 1, so every value
//! lines up regardless of label width), a fill spacer (col 2), and the
//! output port+label (col 3, right-aligned against the node edge). Row `i`
//! holds input `i` and output `i`, so the two sides align. Drawn below the
//! header by [`crate::gui::node::NodeUI`]; the boundary-port rename
//! affordance lives in [`crate::gui::node::port_rename`].

use palantir::{
    Align, Color, Configure, ContextMenu, Corners, Grid, HAlign, InternedStr, MenuItem, Panel,
    Rect, Sense, Shape, Sizing, Spacing, Stroke, Tooltip, Track, Ui, VAlign, WidgetId,
};
use scenarium::data::{DataType, FsPathMode, StaticValue};
use scenarium::function::ValueOption;
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

/// Grid columns: inputs (hug), input values (hug — uniform width keeps the
/// editors aligned), a fill spacer, then outputs (hug). The outputs sit in a
/// *hug* column, not the fill, so the grid's content size includes them: a
/// `fill` column contributes 0 to a hug-sized grid and would collapse,
/// spilling the outputs out of the node (palantir
/// `grid_hug_grid_collapses_fill_tracks`). The fill spacer instead claims any
/// width beyond the ports, pushing the outputs to the node's right edge.
const COL_INPUT: u16 = 0;
const COL_VALUE: u16 = 1;
const COL_OUTPUT: u16 = 3;

pub(crate) fn ports_row(ui: &mut Ui, rcx: RecordCtx<'_>, node: &SceneNode, out: &mut Vec<Intent>) {
    let theme = rcx.theme;
    let n_rows = (node.inputs.len as usize).max(node.outputs.len as usize);
    if n_rows == 0 {
        return;
    }
    let rows: Vec<Track> = vec![Track::hug(); n_rows];
    Grid::new()
        .id_salt("ports")
        .size((Sizing::FILL, Sizing::Hug))
        .cols([
            Track::hug(),
            Track::hug().max(theme.static_value_editor.max_width),
            Track::fill(),
            Track::hug(),
        ])
        .rows(rows)
        .gap_xy(theme.port_gap, theme.port_cols_gap)
        .padding(Spacing::new(
            theme.port_col_pad_x,
            theme.port_col_pad_top,
            theme.port_col_pad_x,
            theme.port_col_pad_top,
        ))
        .show(ui, |ui| {
            input_cells(ui, rcx, node, out);
            output_cells(ui, rcx, node, out);
        });
}

fn input_cells(ui: &mut Ui, rcx: RecordCtx<'_>, node: &SceneNode, out: &mut Vec<Intent>) {
    let names = rcx.scene.input_names(node.inputs);
    let bindings = rcx.scene.bindings(node.inputs);
    let defaults = rcx.scene.defaults(node.inputs);
    let types = rcx.scene.input_types(node.inputs);
    let required = rcx.scene.required(node.inputs);
    let option_spans = rcx.scene.value_option_spans(node.inputs);
    // Boundary (`SubgraphInput`/`SubgraphOutput`) ports route the
    // interface, not literal values — no const affordance.
    let allow_const = !node.boundary;
    for (i, name) in names.iter().enumerate() {
        let port = PortRef {
            node_id: node.id,
            kind: PortKind::Input,
            port_idx: i,
        };
        let binding = bindings.get(i).unwrap_or(&InputBindingView::None);
        let default = defaults.get(i).cloned().flatten();
        let data_type = types.get(i).cloned().unwrap_or_default();
        // A `SubgraphOutput` boundary node's input ports are the subgraph's
        // *outputs* — renameable, except the trailing "+" placeholder.
        let rename = (node.boundary && i + 1 < names.len()).then_some(BoundarySide::Output);
        let tip = type_label(&data_type);
        // A required input with no binding is a missing input — highlight it.
        let missing =
            required.get(i).copied().unwrap_or(false) && matches!(binding, InputBindingView::None);
        input_label_cell(
            ui,
            rcx,
            port,
            i,
            name.clone(),
            binding,
            default,
            allow_const,
            rename,
            &data_type,
            missing,
            &tip,
            out,
        );
        if allow_const && let InputBindingView::Const(value) = binding {
            let options = option_spans
                .get(i)
                .map(|s| rcx.scene.value_options(*s))
                .unwrap_or(&[]);
            value_cell(ui, rcx.theme, port, i, value, &data_type, options, out);
        }
    }
}

fn output_cells(ui: &mut Ui, rcx: RecordCtx<'_>, node: &SceneNode, out: &mut Vec<Intent>) {
    let names = rcx.scene.output_names(node.outputs);
    let types = rcx.scene.output_types(node.outputs);
    for (i, name) in names.iter().enumerate() {
        let port = PortRef {
            node_id: node.id,
            kind: PortKind::Output,
            port_idx: i,
        };
        let data_type = types.get(i).cloned().unwrap_or_default();
        // A `SubgraphInput` boundary node's output ports are the subgraph's
        // *inputs* — renameable, except the trailing "+" placeholder.
        let rename = (node.boundary && i + 1 < names.len()).then_some(BoundarySide::Input);
        output_cell(ui, rcx, port, i, name.clone(), data_type, rename, out);
    }
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

/// Column 0: the input port circle + label, plus the right-click binding
/// menu (anchored here, so right-clicking the circle or label opens it).
/// The circle's `WidgetId` is the deterministic `port_circle_wid(port)`, so
/// `PortFrame`/snap/draw reconstruct it from domain coords.
#[allow(clippy::too_many_arguments)]
fn input_label_cell(
    ui: &mut Ui,
    rcx: RecordCtx<'_>,
    port: PortRef,
    row: usize,
    name: InternedStr,
    binding: &InputBindingView,
    default_static: Option<StaticValue>,
    allow_const: bool,
    rename: Option<BoundarySide>,
    data_type: &DataType,
    missing: bool,
    tip: &str,
    out: &mut Vec<Intent>,
) {
    let theme = rcx.theme;
    // A missing required input paints its port in the warning color regardless
    // of data type, so the unfilled port stands out before a run.
    let fill = if missing {
        theme.exec_missing_glow
    } else {
        port_color(
            theme,
            data_type,
            PortKind::Input,
            rcx.port_frame.is_hovered(port),
        )
    };
    let overhang = theme.port_radius() + theme.port_col_pad_x;
    let margin = Spacing::new(-overhang, 0.0, 0.0, 0.0);
    let wid = port_circle_wid(port);
    let cell = Panel::hstack()
        .id_salt(("in", port.port_idx))
        .grid_cell((row as u16, COL_INPUT))
        .align(Align::new(HAlign::Left, VAlign::Center))
        .size((Sizing::Hug, Sizing::Hug))
        .sense(Sense::CLICK)
        .gap(4.0)
        .child_align(Align::v(VAlign::Center))
        .show(ui, |ui| {
            circle_frame(ui, theme, wid, fill, margin, tip);
            port_label(ui, rcx, port, name, tip, rename, out);
        });
    // Open on right-click anywhere on the cell — circle or label. The
    // circle has its own `Sense::CLICK` and consumes hits over its rect, so
    // the cell snapshot alone misses clicks on the circle (no bubbling).
    let menu_id = cell.response.widget_id();
    let cell_secondary = cell.response.secondary_clicked();
    let circle_state = ui.response_for(wid);
    if (cell_secondary || circle_state.secondary_clicked)
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

/// Column 1: the inline const editor for an input bound to a `Const`. A
/// hug-sized column, so every editor starts at the same x.
#[allow(clippy::too_many_arguments)]
fn value_cell(
    ui: &mut Ui,
    theme: &Theme,
    port: PortRef,
    row: usize,
    value: &StaticValue,
    data_type: &DataType,
    value_options: &[ValueOption],
    out: &mut Vec<Intent>,
) {
    let editor_id = const_editor_wid(port.node_id, port.port_idx);
    // Fill the value column so every editor is the same width (the column
    // hugs to the widest editor's content). `min_size` on the editors keeps
    // a sensible floor; the editor fills this cell, this cell fills the col.
    let edited = Panel::hstack()
        .id_salt(("val", port.port_idx))
        .grid_cell((row as u16, COL_VALUE))
        .size((Sizing::FILL, Sizing::Hug))
        .child_align(Align::v(VAlign::Center))
        .show(ui, |ui| {
            value_editor::show(
                ui,
                &theme.static_value_editor,
                editor_id,
                value,
                data_type,
                value_options,
            )
        });
    if let Some(new_value) = edited.inner {
        out.push(set_input(port, Binding::Const(new_value)));
    }
}

/// Column 2: the output label + circle, right-aligned (the fill column
/// pins it to the node's right edge); the circle overhangs that edge.
#[allow(clippy::too_many_arguments)]
fn output_cell(
    ui: &mut Ui,
    rcx: RecordCtx<'_>,
    port: PortRef,
    row: usize,
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
    let tip = type_label(&data_type);
    let wid = port_circle_wid(port);
    let overhang = theme.port_radius() + theme.port_col_pad_x;
    Panel::hstack()
        .id_salt(("out", port.port_idx))
        .grid_cell((row as u16, COL_OUTPUT))
        .align(Align::new(HAlign::Right, VAlign::Center))
        .size((Sizing::Hug, Sizing::Hug))
        .gap(4.0)
        .child_align(Align::v(VAlign::Center))
        .show(ui, |ui| {
            port_label(ui, rcx, port, name, &tip, rename, out);
            circle_frame(
                ui,
                theme,
                wid,
                fill,
                Spacing::new(0.0, 0.0, -overhang, 0.0),
                &tip,
            );
        });
    // Double-click to disconnect every consumer is handled in
    // `emit_port_disconnects` (prepass) alongside the input-side gesture.
}

/// Hover / grab box scaled past the painted dot so ports are easier to
/// hit and snap to, while the visible circle stays `port_size`.
const PORT_HIT_SCALE: f32 = 1.8;

fn circle_frame(
    ui: &mut Ui,
    theme: &Theme,
    wid: WidgetId,
    fill: Color,
    margin: Spacing,
    tip: &str,
) {
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
    let circle = Panel::zstack()
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
    if !tip.is_empty() {
        Tooltip::for_(&circle.response.snapshot())
            .text(tip.to_owned())
            .show(ui);
    }
}

/// Human-readable type for a port tooltip. Mirrors `DataType`'s `Display`
/// for the scalars, names the picker mode for paths, and reads `Null`
/// (the untyped boundary placeholder) as "any".
fn type_label(ty: &DataType) -> String {
    match ty {
        DataType::Null => "any".to_owned(),
        DataType::FsPath(cfg) => {
            let mode = match cfg.mode {
                FsPathMode::Directory => "directory",
                FsPathMode::ExistingFile => "file",
                FsPathMode::NewFile => "save path",
            };
            if cfg.extensions.is_empty() {
                format!("path · {mode}")
            } else {
                format!("path · {mode} ({})", cfg.extensions.join(", "))
            }
        }
        other => other.to_string(),
    }
}
