//! The ports area of a node body, laid out as a grid: input port+label
//! (col 0), the inline const editor for that input (col 1, so every value
//! lines up regardless of label width), a fill spacer (col 2), and the
//! output port+label (col 3, right-aligned against the node edge). Row `i`
//! holds input `i` and output `i`, so the two sides align. Drawn below the
//! header by [`crate::gui::node::NodeUI`]; the boundary-port rename
//! affordance lives in [`crate::gui::node::port_rename`]. The low-level
//! glyph primitives (circle, event triangle, hit-box growth) this grid
//! renders each cell with live in the sibling [`glyph`] module.

pub(super) mod glyph;

use palantir::{
    Align, Configure, ContextMenu, Grid, HAlign, MenuItem, Panel, PopupHandle, Sense, Sizing,
    Spacing, Text, TextStyle, Track, Ui, VAlign, WidgetId,
};
use scenarium::Binding;
use scenarium::InputPort;
use scenarium::Library;
use scenarium::NodeId;
use scenarium::OutputPort;
use scenarium::{DataType, FsPathMode};

use crate::core::document::BoundarySide;
use crate::core::document::{PortKind, PortRef};
use crate::core::edit::intent::sink::Intents;
use crate::core::edit::intent::types::Intent;
use crate::gui::EventRef;
use crate::gui::canvas::pin_ui;
use crate::gui::node::port_color::{event_color, port_color};
use crate::gui::node::port_rename::port_label;
use crate::gui::node::port_row::glyph::{circle_frame, event_glyph, port_diameter};
use crate::gui::node::value_editor;
use crate::gui::node::{RecordCtx, node_hovered, port_wid, set_input, set_output_pinned};
use crate::gui::run_state::ExecStatus;
use crate::gui::scene::{InputBindingView, SceneEvent, SceneInput, SceneNode, SceneOutput};
use crate::gui::theme::StaticValueEditorTheme;

/// Grid columns: inputs (hug), input values (hug, capped at `max_width` — so
/// wide editors fit but a very long one ellipsizes; the numeric `DragValue`
/// editor caps itself so it doesn't grow this column), a fill spacer, then
/// outputs (hug). The outputs sit in a
/// *hug* column, not the fill, so the grid's content size includes them: a
/// `fill` column contributes 0 to a hug-sized grid and would collapse,
/// spilling the outputs out of the node (palantir
/// `grid_hug_grid_collapses_fill_tracks`). The fill spacer instead claims any
/// width beyond the ports, pushing the outputs to the node's right edge.
const COL_INPUT: u16 = 0;
const COL_VALUE: u16 = 1;
/// Claims the width beyond the ports. Nothing is placed in it — it exists so
/// the outputs sit at the node's right edge — but it is named so the jump
/// from [`COL_VALUE`] to [`COL_OUTPUT`] reads as deliberate.
const COL_SPACER: u16 = 2;
const COL_OUTPUT: u16 = COL_SPACER + 1;

/// Port row height as a multiple of the body font size. The value editors
/// fill this height (so a chip, dropdown, and text field are all the same
/// size); it must clear the tallest editor's min-content — the inline text
/// field, `line_height + chip padding ≈ 1.9em` — so nothing overflows.
const PORT_ROW_HEIGHT_EM: f32 = 2.0;

/// `row_tracks` is [`NodeUI`](crate::gui::node::NodeUI)'s retained staging
/// buffer — see its doc for why the grid's rows aren't built fresh here.
pub(super) fn ports_row(
    ui: &mut Ui,
    rcx: RecordCtx<'_>,
    node: &SceneNode,
    row_tracks: &mut Vec<Track>,
    out: &mut Intents,
) {
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
    //
    // It also gates the port tooltips: their text is built per port per
    // frame, and no port can be showing one while the pointer is elsewhere,
    // so only the node under it pays for them.
    let hovered = node_hovered(ui, node.id);
    let sve = if hovered {
        &theme.static_value_editor_revealed
    } else {
        &theme.static_value_editor
    };
    // Fixed-height rows (font-relative) so a node's ports stay uniform whether
    // or not an input carries an inline editor (hug makes editor rows taller).
    // Every row of every node gets the same track, so the buffer is rebuilt
    // only when a wider node needs more of them or the theme moves the height
    // — not per node. Not a `const`: the height rides the theme's font size.
    let track = Track::fixed(theme.palantir_theme.text.font_size_px * PORT_ROW_HEIGHT_EM);
    if row_tracks.len() < n_rows || row_tracks.first() != Some(&track) {
        row_tracks.clear();
        row_tracks.resize(n_rows, track);
    }
    Grid::new()
        .id_salt("ports")
        .size((Sizing::FILL, Sizing::HUG))
        .cols([
            Track::hug(),
            Track::hug().max(theme.static_value_editor.max_width),
            Track::fill(),
            Track::hug(),
        ])
        .rows(&row_tracks[..n_rows])
        .gap_xy(theme.port_gap, theme.port_cols_gap)
        .padding(Spacing::new(
            theme.port_col_pad_x,
            theme.port_col_pad_top,
            theme.port_col_pad_x,
            theme.port_col_pad_top,
        ))
        .show(ui, |ui| {
            input_cells(ui, rcx, node, sve, hovered, out);
            output_cells(ui, rcx, node, hovered, out);
        });
}

/// The per-cell decisions [`ports_row`] settles once for the whole node.
#[derive(Clone, Copy, Debug)]
struct CellOpts {
    /// Which interface side this port renames, when it is a renameable
    /// boundary port — `None` for an ordinary port and for the trailing "+"
    /// placeholder.
    rename: Option<BoundarySide>,
    /// Build the hover tooltip. Only the node under the pointer does.
    tips: bool,
    /// Offer pinning this output. Outputs only; a definition pane resolves no
    /// single occurrence to pin.
    pinning: bool,
}

/// A port's hover tooltip, built only for the node under the pointer —
/// see [`ports_row`]. Empty otherwise, which
/// [`tooltip_after`](crate::gui::widgets::support::tooltip_after) and
/// [`port_label`] both treat as "no tooltip".
fn tip_for(rcx: RecordCtx<'_>, wanted: bool, description: &str, ty: &DataType) -> String {
    if !wanted {
        return String::new();
    }
    port_tip(description, type_label(rcx.library, ty))
}

fn input_cells(
    ui: &mut Ui,
    rcx: RecordCtx<'_>,
    node: &SceneNode,
    sve: &StaticValueEditorTheme,
    tips: bool,
    out: &mut Intents,
) {
    let inputs = rcx.graph.inputs(node.inputs);
    // Boundary (`GraphInput`/`GraphOutput`) ports route the
    // interface, not literal values — no const affordance.
    let allow_const = !node.boundary;
    for (i, input) in inputs.iter().enumerate() {
        let port = PortRef {
            node_id: node.id,
            kind: PortKind::Input,
            port_idx: i,
        };
        // A `GraphOutput` boundary node's input ports are the graph's
        // *outputs* — renameable, except the trailing "+" placeholder.
        let opts = CellOpts {
            rename: (node.boundary && i + 1 < inputs.len()).then_some(BoundarySide::Output),
            tips,
            pinning: false,
        };
        input_label_cell(ui, rcx, port, node, input, opts, out);
        if allow_const {
            value_cell(ui, rcx, sve, port, input, out);
        }
    }
}

fn output_cells(ui: &mut Ui, rcx: RecordCtx<'_>, node: &SceneNode, tips: bool, out: &mut Intents) {
    let outputs = rcx.graph.outputs(node.outputs);
    for (i, output) in outputs.iter().enumerate() {
        let port = PortRef {
            node_id: node.id,
            kind: PortKind::Output,
            port_idx: i,
        };
        // A `GraphInput` boundary node's output ports are the graph's
        // *inputs* — renameable, except the trailing "+" placeholder.
        let opts = CellOpts {
            rename: (node.boundary && i + 1 < outputs.len()).then_some(BoundarySide::Input),
            tips,
            pinning: rcx.graph.run_available(),
        };
        output_cell(ui, rcx, port, output, opts, out);
    }
    // Events emit from the same (right) side; list them in the rows directly
    // below the data outputs.
    for (i, event) in rcx.graph.events(node.events).iter().enumerate() {
        event_cell(ui, rcx, node.id, i, outputs.len() + i, event, tips);
    }
}

pub(crate) fn port_circle_wid(port: PortRef) -> WidgetId {
    port_wid("port_circle", port)
}

/// An input port's inline const editor (text field, checkbox, or file-pick
/// button).
pub(super) fn const_editor_wid(input: InputPort) -> WidgetId {
    port_wid("const_editor", input_ref(input))
}

/// An input port's cell (circle + label). The prepass polls it for a
/// double-click on the *label* area — the circle has its own
/// [`port_circle_wid`] and consumes hits over its own rect.
pub(super) fn input_cell_wid(port: PortRef) -> WidgetId {
    port_wid("input_cell", port)
}

fn input_ref(input: InputPort) -> PortRef {
    PortRef {
        node_id: input.node_id,
        kind: PortKind::Input,
        port_idx: input.port_idx,
    }
}

/// Open `menu_id`'s context menu when the cell or its port circle was
/// secondary-clicked this frame — shared by the input and output cells.
///
/// `cell_secondary` is read by the caller before this runs: the cell's
/// `Response` borrows `ui`, and this needs `ui` mutably. The circle senses
/// its own `Sense::CLICK` and consumes hits over its rect, so the cell's
/// click alone misses a right-click landed on the circle (no bubbling);
/// checking both closes that gap.
fn open_port_context_menu(ui: &mut Ui, menu_id: WidgetId, cell_secondary: bool, circle: WidgetId) {
    if (cell_secondary || ui.response_for(circle).right.clicked())
        && let Some(p) = ui.pointer_pos()
    {
        ContextMenu::open(ui, menu_id, p);
    }
}

/// The "Remove port" item both cells end their menu with: interface ports
/// are authored, so removal is this explicit action, never a side effect of
/// unwiring. Renders nothing on a port that isn't a renameable boundary one.
fn remove_port_item(
    ui: &mut Ui,
    popup: &PopupHandle,
    port: PortRef,
    rename: Option<BoundarySide>,
    out: &mut Intents,
) {
    if let Some(side) = rename
        && MenuItem::new("Remove port").show(ui, popup).left.clicked()
    {
        out.push(Intent::RemoveBoundaryPort {
            side,
            idx: port.port_idx,
        });
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
    opts: CellOpts,
    out: &mut Intents,
) {
    let theme = rcx.theme;
    let allow_const = !node.boundary;
    let tip = tip_for(rcx, opts.tips, &input.description.borrow_str(), &input.ty);
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
    let outline = (!input.required).then_some(theme.colors.node_fill);
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
                circle_frame(ui, wid, diameter, fill, outline, margin, &tip);
            }
            port_label(ui, rcx, port, input.name.clone(), &tip, opts.rename, out);
        });
    // Open on right-click anywhere on the cell — circle or label.
    let (menu_id, cell_secondary) = (cell.response.id, cell.response.right.clicked());
    open_port_context_menu(ui, menu_id, cell_secondary, wid);
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
                out.push(set_input(port, None));
            }
            remove_port_item(ui, popup, port, opts.rename, out);
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
    out: &mut Intents,
) {
    // The one owner of the "only Const bindings get an inline editor"
    // filter — wired and unbound inputs render no value cell.
    let InputBindingView::Const(value) = &input.binding else {
        return;
    };
    let data_type = &input.ty;
    let value_variants = rcx.graph.value_variants(input.value_variants);
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
/// painted here — see `crate::gui::canvas::pin_ui::draw_pin` (a dragged
/// satellite can end up anywhere on the canvas, not just overhanging this
/// node).
fn output_cell(
    ui: &mut Ui,
    rcx: RecordCtx<'_>,
    port: PortRef,
    output: &SceneOutput,
    opts: CellOpts,
    out: &mut Intents,
) {
    let theme = rcx.theme;
    let fill = port_color(
        theme,
        &output.ty,
        PortKind::Output,
        rcx.geometry.ports.is_hovered(port),
    );
    let tip = tip_for(rcx, opts.tips, &output.description.borrow_str(), &output.ty);
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
            port_label(ui, rcx, port, output.name.clone(), &tip, opts.rename, out);
            circle_frame(
                ui,
                wid,
                theme.port_size,
                fill,
                None,
                Spacing::new(0.0, 0.0, -overhang, 0.0),
                &tip,
            );
        });
    // Double-click to disconnect every consumer is handled in
    // `emit_port_dblclicks` (prepass) alongside the input-side gesture.

    // Right-click anywhere on the cell (circle or label) opens the same
    // toggle as a menu item — mirrors the input side's binding menu.
    //
    // Creating a pin is a Cmd+drag from the circle, repositioning one is a
    // plain drag off its satellite (see `PinUi`) — neither is a click, so
    // the menu item below and the drag are the only ways to pin/unpin.
    let (menu_id, cell_secondary) = (cell.response.id, cell.response.right.clicked());
    open_port_context_menu(ui, menu_id, cell_secondary, wid);
    ContextMenu::for_id(menu_id)
        .size((Sizing::HUG, Sizing::HUG))
        .show(ui, |ui, popup| {
            let pinned = output.pin_position.is_some();
            let label = if pinned { "Unpin output" } else { "Pin output" };
            if MenuItem::new(label)
                .enabled(pinned || opts.pinning)
                .show(ui, popup)
                .left
                .clicked()
            {
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
            remove_port_item(ui, popup, port, opts.rename, out);
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
    tips: bool,
) {
    let theme = rcx.theme;
    let overhang = theme.port_overhang();
    let wid = event_glyph_wid(node_id, event_idx);
    let ev = EventRef { node_id, event_idx };
    let fill = event_color(theme, rcx.geometry.events.is_hovered(ev));
    let tip = if tips {
        format!("event: {}", &*event.name.borrow_str())
    } else {
        String::new()
    };
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
                .style(&TextStyle {
                    color: theme.colors.port_label,
                    ..ui.theme.text.clone()
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

/// An event port glyph. A separate id space from data ports
/// ([`port_circle_wid`]) because events are indexed independently of outputs.
pub(crate) fn event_glyph_wid(node_id: NodeId, event_idx: usize) -> WidgetId {
    WidgetId::from_hash(("graph.node", "event_glyph", node_id, event_idx))
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
                FsPathMode::ExistingFiles => "files",
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
