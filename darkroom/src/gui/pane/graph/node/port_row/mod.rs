//! The ports area of a node body, laid out as a grid: input port+label
//! (col 0), the inline const editor for that input (col 1, so every value
//! lines up regardless of label width), a fill spacer (col 2), and the
//! output port+label (col 3, right-aligned against the node edge). Row `i`
//! holds input `i` and output `i`, so the two sides align. Drawn below the
//! header by [`crate::gui::pane::graph::node::NodeUI`]. The low-level
//! glyph primitives (circle, event triangle, hit-box growth) this grid
//! renders each cell with live in the sibling [`glyph`] module.

pub(super) mod glyph;

use glam::Vec2;
use palantir::{
    Align, Configure, ContextMenu, Grid, HAlign, MenuItem, Panel, PopupHandle, Sense, Sizing,
    Spacing, Text, TextStyle, Tooltip, Track, Ui, VAlign, WidgetId,
};
use scenarium::Binding;
use scenarium::FuncEvent;
use scenarium::InputPort;
use scenarium::Library;
use scenarium::NodeId;
use scenarium::{DataType, FsPathMode, Func};

use crate::core::document::{PortKind, PortRef};
use crate::core::edit::intent::types::GraphIntent;
use crate::core::preview;
use crate::gui::EventRef;
use crate::gui::graph_ctx::input_ctx::InputCtx;
use crate::gui::graph_ctx::node_ctx::NodeCtx;
use crate::gui::graph_ctx::output_ctx::OutputCtx;
use crate::gui::pane::graph::ctx::DrawCtx;
use crate::gui::pane::graph::node::port_color::{event_color, port_color};
use crate::gui::pane::graph::node::port_row::glyph::{circle_frame, event_glyph, port_diameter};
use crate::gui::pane::graph::node::value_editor;
use crate::gui::pane::graph::node::{port_wid, set_input};
use crate::gui::requests::Requests;
use crate::gui::state::run_state::ExecStatus;
use crate::gui::theme::Theme;

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

/// `row_tracks` is [`NodeUI`](crate::gui::pane::graph::node::NodeUI)'s retained staging
/// buffer — see its doc for why the grid's rows aren't built fresh here.
pub(super) fn ports_row(
    ui: &mut Ui,
    ncx: NodeCtx<'_>,
    dcx: DrawCtx<'_>,
    row_tracks: &mut Vec<Track>,
    out: &mut Requests,
) {
    let (theme, node) = (ncx.theme(), ncx);
    // Events list under the outputs in the same column, so the output side
    // needs a row per output *and* per event.
    let n_rows = node
        .port_count(PortKind::Input)
        .max(node.port_count(PortKind::Output) + node.events().len());
    if n_rows == 0 {
        return;
    }
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
        .gap_xy(theme.ports.gap, theme.ports.cols_gap)
        .padding(Spacing::new(
            theme.ports.col_pad_x,
            theme.ports.gap,
            theme.ports.col_pad_x,
            theme.ports.gap,
        ))
        .show(ui, |ui| {
            input_cells(ui, ncx, dcx, out);
            output_cells(ui, ncx, dcx, out);
        });
}

/// A port's hover tooltip, built only for the node under the pointer —
/// see [`ports_row`]. Empty otherwise, which
/// [`tooltip_after`](crate::gui::widgets::support::tooltip_after) and
/// [`port_label`] both treat as "no tooltip".
fn tip_for(ncx: NodeCtx<'_>, description: &str, ty: &DataType) -> String {
    if !ncx.tips() {
        return String::new();
    }
    port_tip(description, type_label(ncx.graph_ctx.library(), ty))
}

/// Render `name` as a port's label, with `tip` (the port's data type) as its
/// hover tooltip; empty means no tooltip, as [`tip_for`] returns off the
/// hovered node.
///
/// Opts into [`Sense::HOVER`] rather than capturing clicks: the label needs a
/// trigger anchor for the tooltip, but the node body below it owns selection
/// and drag, so the press has to fall through. Muted ink — the value column is
/// each row's strong element, not the label.
fn port_label(ui: &mut Ui, theme: &Theme, name: &str, tip: &str) {
    let snapshot = Text::new(name)
        .style(&TextStyle {
            color: theme.ports.label,
            ..ui.theme.text.clone()
        })
        .sense(Sense::HOVER)
        .show(ui)
        .snapshot();
    if !tip.is_empty() {
        Tooltip::on(&snapshot).text(tip).show(ui);
    }
}

fn input_cells(ui: &mut Ui, ncx: NodeCtx<'_>, dcx: DrawCtx<'_>, out: &mut Requests) {
    for input in ncx.inputs() {
        input_label_cell(ui, ncx, dcx, input, out);
        value_cell(ui, ncx, input, out);
    }
}

fn output_cells(ui: &mut Ui, ncx: NodeCtx<'_>, dcx: DrawCtx<'_>, out: &mut Requests) {
    let node = ncx;
    let output_count = node.port_count(PortKind::Output);
    for output in node.outputs() {
        output_cell(ui, ncx, dcx, output, out);
    }
    // Events emit from the same (right) side; list them in the rows directly
    // below the data outputs.
    for (i, event) in node.events().iter().enumerate() {
        event_cell(ui, ncx, dcx, i, output_count + i, event);
    }
}

pub(crate) fn port_circle_wid(port: PortRef) -> WidgetId {
    port_wid("port_circle", port)
}

/// An input port's inline const editor (text field, checkbox, or file-pick
/// button).
pub(crate) fn const_editor_wid(input: InputPort) -> WidgetId {
    port_wid("const_editor", input.into())
}

/// An input port's cell (circle + label). The prepass polls it for a
/// double-click on the *label* area — the circle has its own
/// [`port_circle_wid`] and consumes hits over its own rect.
pub(crate) fn input_cell_wid(port: PortRef) -> WidgetId {
    port_wid("input_cell", port)
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

/// Column 0: the input port circle + label, plus the right-click binding
/// menu (anchored here, so right-clicking the circle or label opens it).
/// The circle's `WidgetId` is the deterministic `port_circle_wid(port)`, so
/// `CanvasGeometry`/snap/draw reconstruct it from domain coords.
fn input_label_cell(
    ui: &mut Ui,
    ncx: NodeCtx<'_>,
    dcx: DrawCtx<'_>,
    input: InputCtx<'_>,
    out: &mut Requests,
) {
    let (theme, node) = (ncx.theme(), ncx);
    let port = input.port_ref();
    let tip = tip_for(ncx, input.description(), input.ty());
    // Flag a port only once a run actually failed on it — not on every unbound edit — so
    // the port keeps its data-type color while editing instead of flipping as you
    // bind/unbind. The run named the exact ports it could not feed, so only those light
    // up; the node-level check is what stops a live re-run's stale verdict from lingering
    // once the node reaches a new status.
    let missing = matches!(node.exec_status(), ExecStatus::MissingInputs) && input.missing();
    let fill = if missing {
        theme.status.warning
    } else {
        port_color(
            theme,
            input.ty(),
            PortKind::Input,
            dcx.geometry().ports.is_hovered(port),
        )
    };
    // A required input's port reads as bigger — its total footprint matches
    // a bound output's circle-plus-ring, so "important port" carries the
    // same visual weight on either side. An optional input instead gets a
    // muted outline, so "not required" reads at a glance without needing
    // the bigger required-input footprint.
    let diameter = port_diameter(theme.ports.size, input.required());
    // Matches the node body itself — the ring reads as the node's own surface
    // wrapping around the port, rather than a separate accent.
    let outline = (!input.required()).then_some(theme.card.fill);
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
            if !input.const_only() {
                circle_frame(ui, wid, diameter, fill, outline, margin, &tip);
            }
            port_label(ui, theme, input.name(), &tip);
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
            // Resolved once for both the enable test and the value the pick
            // pushes — the fallback literal is built on read, not stored.
            let default = input.default();
            let can_set = !matches!(input.binding(), Some(Binding::Const(_))) && default.is_some();
            if MenuItem::new("Set constant")
                .enabled(can_set)
                .show(ui, popup)
                .left
                .clicked()
                && let Some(value) = default
            {
                out.push_graph(set_input(port, Binding::Const(value)));
            }
            if MenuItem::new("Clear binding")
                .enabled(input.binding().is_some())
                .show(ui, popup)
                .left
                .clicked()
            {
                out.push_graph(set_input(port, None));
            }
        });
}

/// Column 1: the inline const editor for an input bound to a `Const`. A
/// hug-sized column, so every editor starts at the same x.
fn value_cell(ui: &mut Ui, ncx: NodeCtx<'_>, input: InputCtx<'_>, out: &mut Requests) {
    // The one owner of the "only Const bindings get an inline editor"
    // filter — wired and unbound inputs render no value cell.
    let Some(Binding::Const(value)) = input.binding() else {
        return;
    };
    let port = input.port_ref();
    let data_type = input.ty();
    let value_variants = input.value_variants();
    let editor_id = const_editor_wid(input.port());
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
                ncx.sve(),
                ncx.graph_ctx.library(),
                editor_id,
                value,
                data_type,
                value_variants,
            )
        });
    if let Some(new_value) = edited.inner {
        out.push_graph(set_input(port, Binding::Const(new_value)));
    }
}

/// Column 3: the output label + circle, right-aligned (the fill column
/// pins it to the node's right edge); the circle overhangs that edge. (A dragged
/// satellite can end up anywhere on the canvas, not just overhanging this
/// node).
fn output_cell(
    ui: &mut Ui,
    ncx: NodeCtx<'_>,
    dcx: DrawCtx<'_>,
    output: OutputCtx<'_>,
    out: &mut Requests,
) {
    let theme = ncx.theme();
    let port = output.port_ref();
    // Resolved once for the fill and the tooltip: a wildcard output follows
    // its mirror chain on every read, so the cell asks once.
    let ty = output.ty();
    let fill = port_color(
        theme,
        &ty,
        PortKind::Output,
        dcx.geometry().ports.is_hovered(port),
    );
    let tip = tip_for(ncx, output.description(), &ty);
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
            port_label(ui, theme, output.name(), &tip);
            circle_frame(
                ui,
                wid,
                theme.ports.size,
                fill,
                None,
                Spacing::new(0.0, 0.0, -overhang, 0.0),
                &tip,
            );
        });
    // Double-click to disconnect every consumer is handled in
    // `emit_port_dblclicks` (prepass) alongside the input-side gesture.

    // Right-click anywhere on the cell (circle or label) opens the port menu —
    // mirrors the input side's binding menu.
    let (menu_id, cell_secondary) = (cell.response.id, cell.response.right.clicked());
    open_port_context_menu(ui, menu_id, cell_secondary, wid);
    ContextMenu::for_id(menu_id)
        .size((Sizing::HUG, Sizing::HUG))
        .show(ui, |ui, popup| {
            add_preview_item(ui, popup, ncx, dcx, port, out);
        });
}

/// Where a preview lands relative to the port it was added from: clear of the
/// node body and a little above, so it doesn't cover what it is watching.
const PREVIEW_SPAWN_OFFSET: Vec2 = Vec2::new(80.0, -60.0);

/// "Add preview" — spawn a preview node already wired to this output. The
/// replacement for the old pin toggle: same affordance, but what it creates is
/// an ordinary node the user can move, delete, and undo like any other.
///
/// Hidden when the library has no preview func.
fn add_preview_item(
    ui: &mut Ui,
    popup: &PopupHandle,
    ncx: NodeCtx<'_>,
    dcx: DrawCtx<'_>,
    port: PortRef,
    out: &mut Requests,
) {
    let Some(func) = preview::registered(ncx.graph_ctx.library()) else {
        return;
    };
    if !MenuItem::new("Add preview").show(ui, popup).left.clicked() {
        return;
    }
    // Positioned off the port when its center is known (it is, after the first
    // frame); otherwise the node lands at the origin and the user drags it.
    let pos = dcx
        .geometry()
        .ports
        .center(port)
        .map_or(Vec2::ZERO, |center| center + PREVIEW_SPAWN_OFFSET);
    out.extend_graph(add_preview_intents(func, port, pos, NodeId::unique()));
}

/// The two intents that spawn a preview already reading `port`. Emitted
/// together so one undo removes node *and* wire — the same shape
/// `connection_ui::commit_connection` uses for a boundary port.
pub(crate) fn add_preview_intents(
    func: &Func,
    port: PortRef,
    pos: Vec2,
    node_id: NodeId,
) -> [GraphIntent; 2] {
    [
        GraphIntent::AddNode {
            pos,
            node_id,
            node: func.into(),
            bindings: func.default_bindings(node_id).collect(),
        },
        GraphIntent::SetInput {
            input: InputPort::new(node_id, 0),
            to: Some(Binding::bind(port.node_id, port.port_idx)),
        },
    ]
}

/// One event (emitter) port row: the event name plus an event-colored triangle
/// glyph, right-aligned and overhanging the node edge like a data output. Sits in
/// `COL_OUTPUT` at `row` (below the data outputs). The glyph senses drags so a
/// wire can be pulled from it to a subscriber pin (see `SubscriptionUI`).
fn event_cell(
    ui: &mut Ui,
    ncx: NodeCtx<'_>,
    dcx: DrawCtx<'_>,
    event_idx: usize,
    row: usize,
    event: &FuncEvent,
) {
    let theme = ncx.theme();
    let node_id = ncx.id;
    let overhang = theme.port_overhang();
    let wid = event_glyph_wid(node_id, event_idx);
    let ev = EventRef { node_id, event_idx };
    let fill = event_color(theme, dcx.geometry().events.is_hovered(ev));
    let tip = if ncx.tips() {
        format!("event: {}", event.name)
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
            Text::new(event.name.as_str())
                .style(&TextStyle {
                    color: theme.ports.label,
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
/// [`InputCtx::description`] text (empty = none).
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

#[cfg(test)]
mod tests {
    use super::*;
    use scenarium::{NodeKind, OutputPort};

    use crate::core::preview::preview_func;

    /// "Add preview" spawns a node already reading the port it was raised
    /// from, offset clear of it, as one batch — so a single undo removes both
    /// the node and its wire.
    #[test]
    fn add_preview_spawns_a_node_already_wired_to_the_port() {
        let func = preview_func(Default::default());
        let producer = NodeId::unique();
        let port = PortRef::output(producer, 2);
        let center = Vec2::new(100.0, 40.0);

        let node_id = NodeId::unique();
        let [add, bind] = add_preview_intents(&func, port, center + PREVIEW_SPAWN_OFFSET, node_id);

        let GraphIntent::AddNode {
            pos, node_id, node, ..
        } = add
        else {
            panic!("first intent adds the node, got {add:?}");
        };
        assert_eq!(pos, Vec2::new(180.0, -20.0), "offset clear of the port");
        assert_eq!(node.kind, NodeKind::Func(func.id));
        assert!(matches!(
            bind,
            GraphIntent::SetInput { input, to: Some(Binding::Bind(src)) }
                if input == InputPort::new(node_id, 0)
                    && src == OutputPort::new(producer, 2)
        ));
    }
}
