//! The ports area of a node body, laid out as a grid: input port+label
//! (col 0), the inline const editor for that input (col 1, so every value
//! lines up regardless of label width), a fill spacer (col 2), and the
//! output port+label (col 3, right-aligned against the node edge). Row `i`
//! holds input `i` and output `i`, so the two sides align. Drawn below the
//! header by [`crate::gui::node::NodeUI`]; the boundary-port rename
//! affordance lives in [`crate::gui::node::port_rename`]. The low-level
//! glyph primitives (circle, event triangle, hit-box growth) this grid
//! renders each cell with live in the sibling [`glyph`] module.

pub(crate) mod glyph;

use aperture::{
    Align, Configure, ContextMenu, Grid, HAlign, MenuItem, Panel, Sense, Sizing, Spacing, Text,
    TextStyle, Track, Ui, VAlign, WidgetId,
};
use scenarium::Binding;
use scenarium::InputPort;
use scenarium::Library;
use scenarium::NodeId;
use scenarium::OutputPort;
use scenarium::{DataType, FsPathMode};

use crate::core::document::BoundarySide;
use crate::core::document::{PortKind, PortRef};
use crate::core::edit::intent::types::Intent;
use crate::gui::EventRef;
use crate::gui::canvas::pin_ui;
use crate::gui::node::port_color::{event_color, port_color};
use crate::gui::node::port_rename::port_label;
use crate::gui::node::port_row::glyph::{PortDecoration, circle_frame, event_glyph, port_diameter};
use crate::gui::node::value_editor;
use crate::gui::node::{RecordCtx, node_hovered, set_input, set_output_pinned};
use crate::gui::run_state::ExecStatus;
use crate::gui::scene::{InputBindingView, SceneEvent, SceneInput, SceneNode, SceneOutput};
use crate::gui::theme::StaticValueEditorTheme;

/// Grid columns: inputs (hug), input values (hug, capped at `max_width` — so
/// wide editors fit but a very long one ellipsizes; the numeric `DragValue`
/// editor caps itself so it doesn't grow this column), a fill spacer, then
/// outputs (hug). The outputs sit in a
/// *hug* column, not the fill, so the grid's content size includes them: a
/// `fill` column contributes 0 to a hug-sized grid and would collapse,
/// spilling the outputs out of the node (aperture
/// `grid_hug_grid_collapses_fill_tracks`). The fill spacer instead claims any
/// width beyond the ports, pushing the outputs to the node's right edge.
const COL_INPUT: u16 = 0;
const COL_VALUE: u16 = 1;
const COL_OUTPUT: u16 = 3;

/// Port row height as a multiple of the body font size. The value editors
/// fill this height (so a chip, dropdown, and text field are all the same
/// size); it must clear the tallest editor's min-content — the inline text
/// field, `line_height + chip padding ≈ 1.9em` — so nothing overflows.
const PORT_ROW_HEIGHT_EM: f32 = 2.0;

pub(crate) fn ports_row(ui: &mut Ui, rcx: RecordCtx<'_>, node: &SceneNode, out: &mut Vec<Intent>) {
    let theme = rcx.theme;
    // Events list under the outputs in the same column, so the output side
    // needs a row per output *and* per event.
    let n_rows =
        (node.inputs.len as usize).max(node.outputs.len as usize + node.events.len as usize);
    if n_rows == 0 {
        return;
    }
    // Pointer-over-node surfaces the (otherwise invisible) const-editor
    // chips at half strength — the edit affordance appears exactly when the
    // pointer is in the neighborhood, and geometry never changes.
    let sve = if node_hovered(ui, node.id) {
        &theme.static_value_editor_revealed
    } else {
        &theme.static_value_editor
    };
    // Fixed-height rows (font-relative) so a node's ports stay uniform whether
    // or not an input carries an inline editor (hug makes editor rows taller).
    let row_height = theme.aperture_theme.text.font_size_px * PORT_ROW_HEIGHT_EM;
    let rows: Vec<Track> = vec![Track::fixed(row_height); n_rows];
    Grid::new()
        .id_salt("ports")
        .size((Sizing::FILL, Sizing::HUG))
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
            input_cells(ui, rcx, node, sve, out);
            output_cells(ui, rcx, node, out);
        });
}

fn input_cells(
    ui: &mut Ui,
    rcx: RecordCtx<'_>,
    node: &SceneNode,
    sve: &StaticValueEditorTheme,
    out: &mut Vec<Intent>,
) {
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
        if allow_const {
            value_cell(ui, rcx, sve, port, input, out);
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
pub(crate) fn const_editor_wid(input: InputPort) -> WidgetId {
    WidgetId::from_hash(("graph.node.const_editor", input.node_id, input.port_idx))
}

/// Stable widget id for an input port's cell (circle + label). The prepass
/// polls it for a double-click on the label area (the circle has its own
/// `port_circle_wid`) to toggle the input's binding.
pub(crate) fn input_cell_wid(port: PortRef) -> WidgetId {
    WidgetId::from_hash(("graph.node.input_cell", port.node_id, port.port_idx))
}

/// Opens `menu_id`'s context menu when either the cell or its port circle
/// was secondary-clicked this frame — shared by the input and output
/// cells. The circle senses its own `Sense::CLICK` and consumes hits over
/// its rect, so the cell's own click alone misses a right-click landed on
/// the circle (no bubbling); checking both closes that gap.
fn open_port_context_menu(
    ui: &mut Ui,
    menu_id: WidgetId,
    cell_secondary: bool,
    circle_secondary: bool,
) {
    if (cell_secondary || circle_secondary)
        && let Some(p) = ui.pointer_pos()
    {
        ContextMenu::open(ui, menu_id, p);
    }
}

/// Column 0: the input port circle + label, plus the right-click binding
/// menu (anchored here, so right-clicking the circle or label opens it).
/// The circle's `WidgetId` is the deterministic `port_circle_wid(port)`, so
/// `CanvasGeometry`/snap/draw reconstruct it from domain coords.
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
        &input.description.borrow_str(),
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
            rcx.geometry.ports.is_hovered(port),
        )
    };
    // A required input's port reads as bigger — its total footprint matches
    // a bound output's circle-plus-ring, so "important port" carries the
    // same visual weight on either side. An optional input instead gets a
    // muted outline, so "not required" reads at a glance without needing
    // the bigger required-input footprint.
    let diameter = port_diameter(theme.port_size, input.required);
    // Matches the node body itself — the ring reads as the node's own surface
    // wrapping around the port, rather than a separate accent.
    let decoration = if input.required {
        PortDecoration::None
    } else {
        PortDecoration::Outline(theme.colors.node_fill)
    };
    let radius = diameter * 0.5;
    let overhang = theme.port_overhang_for(radius);
    let margin = Spacing::new(-overhang, 0.0, 0.0, 0.0);
    let wid = port_circle_wid(port);
    // Stable cell id so the prepass can poll a label-area double-click (the
    // circle has its own `port_circle_wid`); also the context-menu anchor.
    let cell = Panel::hstack()
        .id(input_cell_wid(port))
        .grid_cell((port.port_idx as u16, COL_INPUT))
        .align(Align::new(HAlign::Left, VAlign::Center))
        .size((Sizing::HUG, Sizing::HUG))
        .sense(Sense::CLICK)
        .gap(4.0)
        .child_align(Align::v(VAlign::Center))
        .show(ui, |ui| {
            // A const-only input can't be wired, so it has no connection anchor
            // — render just the label (+ its inline const editor).
            if !input.const_only {
                circle_frame(ui, wid, diameter, fill, decoration, margin, &tip);
            }
            port_label(ui, rcx, port, input.name.clone(), &tip, rename, out);
        });
    // Open on right-click anywhere on the cell — circle or label.
    let menu_id = cell.response.id;
    let cell_secondary = cell.response.right.clicked();
    let circle_state = ui.response_for(wid);
    open_port_context_menu(ui, menu_id, cell_secondary, circle_state.right.clicked());
    // Double-click on the circle or label toggles the binding (clear, or seed
    // the default const when unbound) — handled in `emit_port_dblclicks`
    // (prepass), since adding/removing a `Const` resizes the node and the
    // wires must re-anchor before the record.
    ContextMenu::for_id(menu_id)
        .size((Sizing::HUG, Sizing::HUG))
        .show(ui, |ui, popup| {
            let can_set = allow_const
                && !matches!(input.binding, InputBindingView::Const(_))
                && input.default.is_some();
            if MenuItem::new("Set constant")
                .enabled(can_set)
                .show(ui, popup)
                .left
                .clicked()
                && let Some(value) = input.default.clone()
            {
                out.push(set_input(port, Binding::Const(value)));
            }
            if MenuItem::new("Clear binding")
                .enabled(!matches!(input.binding, InputBindingView::None))
                .show(ui, popup)
                .left
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
    sve: &StaticValueEditorTheme,
    port: PortRef,
    input: &SceneInput,
    out: &mut Vec<Intent>,
) {
    // The one owner of the "only Const bindings get an inline editor"
    // filter — wired and unbound inputs render no value cell.
    let InputBindingView::Const(value) = &input.binding else {
        return;
    };
    let data_type = &input.ty;
    let value_variants = rcx.scene.value_variants(input.value_variants);
    let editor_id = const_editor_wid(InputPort::new(port.node_id, port.port_idx));
    // Fill the value column so every editor is the same width (the column
    // hugs to the widest editor's content). `min_size` on the editors keeps
    // a sensible floor; the editor fills this cell, this cell fills the col.
    let edited = Panel::hstack()
        .id_salt(("val", port.port_idx))
        .grid_cell((port.port_idx as u16, COL_VALUE))
        .size((Sizing::FILL, Sizing::FILL))
        .child_align(Align::v(VAlign::Center))
        .show(ui, |ui| {
            value_editor::show(
                ui,
                sve,
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

/// Column 3: the output label + circle, right-aligned (the fill column
/// pins it to the node's right edge); the circle overhangs that edge. A
/// pinned output's bezier + satellite are a canvas-level decoration, not
/// painted here — see `crate::gui::canvas::pin_ui::PinUi::draw` (a dragged
/// satellite can end up anywhere on the canvas, not just overhanging this
/// node).
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
        rcx.geometry.ports.is_hovered(port),
    );
    let tip = port_tip(
        &output.description.borrow_str(),
        type_label(rcx.library, &output.ty),
    );
    let wid = port_circle_wid(port);
    let overhang = theme.port_overhang();
    let cell = Panel::hstack()
        .id_salt(("out", port.port_idx))
        .grid_cell((port.port_idx as u16, COL_OUTPUT))
        .align(Align::new(HAlign::Right, VAlign::Center))
        .size((Sizing::HUG, Sizing::HUG))
        .sense(Sense::CLICK)
        .gap(4.0)
        .child_align(Align::v(VAlign::Center))
        .show(ui, |ui| {
            port_label(ui, rcx, port, output.name.clone(), &tip, rename, out);
            circle_frame(
                ui,
                wid,
                theme.port_size,
                fill,
                PortDecoration::None,
                Spacing::new(0.0, 0.0, -overhang, 0.0),
                &tip,
            );
        });
    // Double-click to disconnect every consumer is handled in
    // `emit_port_dblclicks` (prepass) alongside the input-side gesture.

    // Right-click anywhere on the cell (circle or label) opens the same
    // toggle as a menu item — mirrors the input side's binding menu.
    let menu_id = cell.response.id;
    let cell_secondary = cell.response.right.clicked();
    let circle_state = ui.response_for(wid);
    // Creating a pin is a Cmd+drag from the circle, repositioning one is a
    // plain drag off its satellite (see `PinUi`) — neither is a click, so
    // the menu item below and the drag are the only ways to pin/unpin.
    open_port_context_menu(ui, menu_id, cell_secondary, circle_state.right.clicked());
    ContextMenu::for_id(menu_id)
        .size((Sizing::HUG, Sizing::HUG))
        .show(ui, |ui, popup| {
            let pinned = output.pin_position.is_some();
            let label = if pinned { "Unpin output" } else { "Pin output" };
            if MenuItem::new(label).show(ui, popup).left.clicked() {
                let pinning = !pinned;
                out.push(set_output_pinned(port, pinning));
                // Unlike Cmd+drag (which places a fresh pin via its own
                // drag anchor), this toggle has no drag to derive a
                // position from — seed one explicitly so the widget floats
                // clear of the node instead of landing on top of it.
                if pinning && let Some(port_center) = rcx.geometry.ports.center(port) {
                    let out_port = OutputPort::new(port.node_id, port.port_idx);
                    out.push(pin_ui::seed_pin_position_intent(
                        out_port,
                        port_center + pin_ui::default_pin_offset(theme),
                    ));
                }
            }
        });
}

/// One event (emitter) port row: the event name plus an event-colored triangle
/// glyph, right-aligned and overhanging the node edge like a data output. Sits in
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
    let fill = event_color(theme, rcx.geometry.events.is_hovered(ev));
    let tip = format!("event: {}", &*event.name.borrow_str());
    Panel::hstack()
        .id_salt(("event", event_idx))
        .grid_cell((row as u16, COL_OUTPUT))
        .align(Align::new(HAlign::Right, VAlign::Center))
        .size((Sizing::HUG, Sizing::HUG))
        .gap(4.0)
        .child_align(Align::v(VAlign::Center))
        .show(ui, |ui| {
            // Muted like the data-port labels (see `port_label`).
            Text::new(event.name.clone())
                .style(TextStyle {
                    color: theme.colors.port_label,
                    ..ui.theme.text
                })
                .show(ui);
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
/// outputs. `pub(crate)` so `CanvasGeometry` / `SubscriptionUI` reconstruct it
/// from domain coords (`EventRef`) to poll the drag.
pub(crate) fn event_glyph_wid(node_id: NodeId, event_idx: usize) -> WidgetId {
    WidgetId::from_hash(("graph.node.event_glyph", node_id, event_idx))
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
/// paths, `Any` (the untyped boundary placeholder) as "any", and a registered
/// `Custom`/`Enum` type's display name (the raw id if it isn't registered).
fn type_label(library: &Library, ty: &DataType) -> String {
    match ty {
        DataType::Any => "any".to_owned(),
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
