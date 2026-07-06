//! The ports area of a node body, laid out as a grid: input port+label
//! (col 0), the inline const editor for that input (col 1, so every value
//! lines up regardless of label width), a fill spacer (col 2), and the
//! output port+label (col 3, right-aligned against the node edge). Row `i`
//! holds input `i` and output `i`, so the two sides align. Drawn below the
//! header by [`crate::gui::node::NodeUI`]; the boundary-port rename
//! affordance lives in [`crate::gui::node::port_rename`].

use glam::Vec2;
use palantir::{
    Align, Color, Configure, ContextMenu, Corners, Grid, HAlign, MenuItem, Panel, Rect, Sense,
    Shape, Sizing, Spacing, Stroke, Text, Tooltip, Track, Ui, VAlign, WidgetId,
};
use scenarium::data::{DataType, FsPathMode, StaticValue};
use scenarium::graph::Binding;
use scenarium::graph::NodeId;
use scenarium::library::Library;
use scenarium::node::function::ValueVariant;

use crate::core::document::BoundarySide;
use crate::core::edit::intent::Intent;
use crate::gui::node::port_color::{event_color, port_color};
use crate::gui::node::port_rename::port_label;
use crate::gui::node::value_editor;
use crate::gui::node::{RecordCtx, set_input};
use crate::gui::run_state::ExecStatus;
use crate::gui::scene::{InputBindingView, SceneEvent, SceneInput, SceneNode, SceneOutput};
use crate::gui::theme::Theme;
use crate::gui::{EventRef, PortKind, PortRef};

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

/// Port row height as a multiple of the body font size. Clears the tallest
/// inline editor (a preset dropdown / pick chip ≈ 1.7em) so fixed rows stay
/// uniform without clipping their editor.
const PORT_ROW_HEIGHT_EM: f32 = 1.8;

pub(crate) fn ports_row(ui: &mut Ui, rcx: RecordCtx<'_>, node: &SceneNode, out: &mut Vec<Intent>) {
    let theme = rcx.theme;
    // Events list under the outputs in the same column, so the output side
    // needs a row per output *and* per event.
    let n_rows =
        (node.inputs.len as usize).max(node.outputs.len as usize + node.events.len as usize);
    if n_rows == 0 {
        return;
    }
    // Fixed-height rows (font-relative) so a node's ports stay uniform whether
    // or not an input carries an inline editor (hug makes editor rows taller).
    let row_height = theme.palantir_theme.text.font_size_px * PORT_ROW_HEIGHT_EM;
    let rows: Vec<Track> = vec![Track::fixed(row_height); n_rows];
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
    let inputs = rcx.scene.inputs(node.inputs);
    // Boundary (`SubgraphInput`/`SubgraphOutput`) ports route the
    // interface, not literal values — no const affordance.
    let allow_const = !node.boundary;
    for (i, input) in inputs.iter().enumerate() {
        let port = PortRef {
            node_id: node.id,
            kind: PortKind::Input,
            port_idx: i,
        };
        // A `SubgraphOutput` boundary node's input ports are the subgraph's
        // *outputs* — renameable, except the trailing "+" placeholder.
        let rename = (node.boundary && i + 1 < inputs.len()).then_some(BoundarySide::Output);
        input_label_cell(ui, rcx, port, node, input, rename, out);
        if allow_const && let InputBindingView::Const(value) = &input.binding {
            let variants = rcx.scene.value_variants(input.value_variants);
            value_cell(ui, rcx, port, value, &input.ty, variants, out);
        }
    }
}

fn output_cells(ui: &mut Ui, rcx: RecordCtx<'_>, node: &SceneNode, out: &mut Vec<Intent>) {
    let outputs = rcx.scene.outputs(node.outputs);
    for (i, output) in outputs.iter().enumerate() {
        let port = PortRef {
            node_id: node.id,
            kind: PortKind::Output,
            port_idx: i,
        };
        // A `SubgraphInput` boundary node's output ports are the subgraph's
        // *inputs* — renameable, except the trailing "+" placeholder.
        let rename = (node.boundary && i + 1 < outputs.len()).then_some(BoundarySide::Input);
        output_cell(ui, rcx, port, output, rename, out);
    }
    // Events emit from the same (right) side; list them in the rows directly
    // below the data outputs.
    for (i, event) in rcx.scene.events(node.events).iter().enumerate() {
        event_cell(ui, rcx, node.id, i, outputs.len() + i, event);
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

/// Stable widget id for an input port's cell (circle + label). The prepass
/// polls it for a double-click on the label area (the circle has its own
/// `port_circle_wid`) to toggle the input's binding.
pub(crate) fn input_cell_wid(port: PortRef) -> WidgetId {
    WidgetId::from_hash(("graph.node.input_cell", port.node_id, port.port_idx))
}

/// Column 0: the input port circle + label, plus the right-click binding
/// menu (anchored here, so right-clicking the circle or label opens it).
/// The circle's `WidgetId` is the deterministic `port_circle_wid(port)`, so
/// `PortFrame`/snap/draw reconstruct it from domain coords.
fn input_label_cell(
    ui: &mut Ui,
    rcx: RecordCtx<'_>,
    port: PortRef,
    node: &SceneNode,
    input: &SceneInput,
    rename: Option<BoundarySide>,
    out: &mut Vec<Intent>,
) {
    let theme = rcx.theme;
    let allow_const = !node.boundary;
    let tip = port_tip(
        input.description.as_str(""),
        type_label(rcx.library, &input.ty),
    );
    // Flag a required input's port only once a run actually failed on it (the
    // node is `MissingInputs`) — not on every unbound edit — so the port keeps
    // its data-type color while editing instead of flipping as you bind/unbind.
    let missing = matches!(node.exec_status, ExecStatus::MissingInputs)
        && input.required
        && matches!(input.binding, InputBindingView::None);
    let fill = if missing {
        theme.colors.exec_missing_glow
    } else {
        port_color(
            theme,
            &input.ty,
            PortKind::Input,
            rcx.port_frame.ports.is_hovered(port),
        )
    };
    let overhang = theme.port_overhang();
    let margin = Spacing::new(-overhang, 0.0, 0.0, 0.0);
    let wid = port_circle_wid(port);
    // Stable cell id so the prepass can poll a label-area double-click (the
    // circle has its own `port_circle_wid`); also the context-menu anchor.
    let cell = Panel::hstack()
        .id(input_cell_wid(port))
        .grid_cell((port.port_idx as u16, COL_INPUT))
        .align(Align::new(HAlign::Left, VAlign::Center))
        .size((Sizing::Hug, Sizing::Hug))
        .sense(Sense::CLICK)
        .gap(4.0)
        .child_align(Align::v(VAlign::Center))
        .show(ui, |ui| {
            // A const-only input can't be wired, so it has no connection anchor
            // — render just the label (+ its inline const editor).
            if !input.const_only {
                circle_frame(ui, theme, wid, fill, margin, &tip);
            }
            port_label(ui, rcx, port, input.name.clone(), &tip, rename, out);
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
    // Double-click on the circle or label toggles the binding (clear, or seed
    // the default const when unbound) — handled in `emit_port_dblclicks`
    // (prepass), since adding/removing a `Const` resizes the node and the
    // wires must re-anchor before the record.
    ContextMenu::for_id(menu_id)
        .size((Sizing::Hug, Sizing::Hug))
        .show(ui, |ui, popup| {
            let can_set = allow_const
                && !matches!(input.binding, InputBindingView::Const(_))
                && input.default.is_some();
            if MenuItem::new("Set constant")
                .enabled(can_set)
                .show(ui, popup)
                .clicked()
                && let Some(value) = input.default.clone()
            {
                out.push(set_input(port, Binding::Const(value)));
            }
            if MenuItem::new("Clear binding")
                .enabled(!matches!(input.binding, InputBindingView::None))
                .show(ui, popup)
                .clicked()
            {
                out.push(set_input(port, Binding::None));
            }
        });
}

/// Column 1: the inline const editor for an input bound to a `Const`. A
/// hug-sized column, so every editor starts at the same x.
fn value_cell(
    ui: &mut Ui,
    rcx: RecordCtx<'_>,
    port: PortRef,
    value: &StaticValue,
    data_type: &DataType,
    value_variants: &[ValueVariant],
    out: &mut Vec<Intent>,
) {
    let editor_id = const_editor_wid(port.node_id, port.port_idx);
    // Fill the value column so every editor is the same width (the column
    // hugs to the widest editor's content). `min_size` on the editors keeps
    // a sensible floor; the editor fills this cell, this cell fills the col.
    let edited = Panel::hstack()
        .id_salt(("val", port.port_idx))
        .grid_cell((port.port_idx as u16, COL_VALUE))
        .size((Sizing::FILL, Sizing::Hug))
        .child_align(Align::v(VAlign::Center))
        .show(ui, |ui| {
            value_editor::show(
                ui,
                &rcx.theme.static_value_editor,
                rcx.library,
                editor_id,
                value,
                data_type,
                value_variants,
            )
        });
    if let Some(new_value) = edited.inner {
        out.push(set_input(port, Binding::Const(new_value)));
    }
}

/// Column 2: the output label + circle, right-aligned (the fill column
/// pins it to the node's right edge); the circle overhangs that edge.
fn output_cell(
    ui: &mut Ui,
    rcx: RecordCtx<'_>,
    port: PortRef,
    output: &SceneOutput,
    rename: Option<BoundarySide>,
    out: &mut Vec<Intent>,
) {
    let theme = rcx.theme;
    let fill = port_color(
        theme,
        &output.ty,
        PortKind::Output,
        rcx.port_frame.ports.is_hovered(port),
    );
    let tip = port_tip(
        output.description.as_str(""),
        type_label(rcx.library, &output.ty),
    );
    let wid = port_circle_wid(port);
    let overhang = theme.port_overhang();
    Panel::hstack()
        .id_salt(("out", port.port_idx))
        .grid_cell((port.port_idx as u16, COL_OUTPUT))
        .align(Align::new(HAlign::Right, VAlign::Center))
        .size((Sizing::Hug, Sizing::Hug))
        .gap(4.0)
        .child_align(Align::v(VAlign::Center))
        .show(ui, |ui| {
            port_label(ui, rcx, port, output.name.clone(), &tip, rename, out);
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
    // `emit_port_dblclicks` (prepass) alongside the input-side gesture.
}

/// One event (emitter) port row: the event name plus a white triangle glyph,
/// right-aligned and overhanging the node edge like a data output. Sits in
/// `COL_OUTPUT` at `row` (below the data outputs). The glyph senses drags so a
/// wire can be pulled from it to a subscriber pin (see `SubscriptionUI`).
fn event_cell(
    ui: &mut Ui,
    rcx: RecordCtx<'_>,
    node_id: NodeId,
    event_idx: usize,
    row: usize,
    event: &SceneEvent,
) {
    let theme = rcx.theme;
    let overhang = theme.port_overhang();
    let wid = event_glyph_wid(node_id, event_idx);
    let ev = EventRef { node_id, event_idx };
    let fill = event_color(theme, rcx.port_frame.events.is_hovered(ev));
    let tip = format!("event: {}", event.name.as_str(""));
    Panel::hstack()
        .id_salt(("event", event_idx))
        .grid_cell((row as u16, COL_OUTPUT))
        .align(Align::new(HAlign::Right, VAlign::Center))
        .size((Sizing::Hug, Sizing::Hug))
        .gap(4.0)
        .child_align(Align::v(VAlign::Center))
        .show(ui, |ui| {
            Text::new(event.name.clone()).show(ui);
            event_glyph(
                ui,
                theme,
                wid,
                fill,
                Spacing::new(0.0, 0.0, -overhang, 0.0),
                &tip,
            );
        });
}

/// Stable widget id for an event port glyph. A separate id space from data
/// ports (`port_circle_wid`) because events are indexed independently of
/// outputs. `pub(crate)` so `PortFrame` / `SubscriptionUI` reconstruct it
/// from domain coords (`EventRef`) to poll the drag.
pub(crate) fn event_glyph_wid(node_id: NodeId, event_idx: usize) -> WidgetId {
    WidgetId::from_hash(("graph.node.event_glyph", node_id, event_idx))
}

/// Paints an event port glyph: a right-pointing triangle (a port dot rotated
/// 90°), the same `port_size` box and edge overhang as a data port's circle,
/// so it lines up with the outputs above it. `fill` carries the hover state;
/// `tip` shows as a hover tooltip. Senses `CLICK | DRAG` so a subscription
/// wire can be dragged out of it.
fn event_glyph(ui: &mut Ui, theme: &Theme, wid: WidgetId, fill: Color, margin: Spacing, tip: &str) {
    let port = theme.port_size;
    let glyph = Panel::zstack()
        .id(wid)
        .size((Sizing::Fixed(port), Sizing::Fixed(port)))
        .margin(margin)
        .sense(Sense::CLICK | Sense::DRAG)
        .show(ui, |ui| {
            // Right-pointing isosceles triangle filling the port box: the apex
            // points outward (away from the node body), matching the emit
            // direction. SDF-antialiased via the triangle primitive.
            ui.add_shape(Shape::Triangle {
                a: Vec2::new(0.0, 0.0),
                b: Vec2::new(0.0, port),
                c: Vec2::new(port, port * 0.5),
                radius: 0.0,
                fill: fill.into(),
                stroke: Stroke::ZERO,
            });
        });
    if !tip.is_empty() {
        Tooltip::for_(&glyph.response.snapshot())
            .text(tip.to_owned())
            .show(ui);
    }
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

/// A port's hover tooltip: its `description` (when the func declares one) above a
/// dimmer type line, else just the type. `description` is the resolved
/// [`crate::gui::scene::SceneInput::description`] text (empty = none).
fn port_tip(description: &str, type_label: String) -> String {
    if description.is_empty() {
        type_label
    } else {
        format!("{description}\n{type_label}")
    }
}

/// Human-readable type for a port tooltip: scalar names, the picker mode for
/// paths, `Null` (the untyped boundary placeholder) as "any", and a registered
/// `Custom`/`Enum` type's display name (the raw id if it isn't registered).
fn type_label(library: &Library, ty: &DataType) -> String {
    match ty {
        // The untyped boundary placeholder reads as "any" here, not "null".
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
        _ => library.type_name(ty).into_owned(),
    }
}
