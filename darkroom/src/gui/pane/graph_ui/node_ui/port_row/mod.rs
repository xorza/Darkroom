//! The ports area of a node body, laid out as a grid: input port and label (col 0), the
//! inline const editor for that input (col 1, so every value lines up regardless of label
//! width), a fill spacer (col 2), and the output port and label (col 3, right-aligned against
//! the node edge). Row `i` holds input `i` and output `i`, so the two sides align. Drawn below
//! the header by [`crate::gui::pane::graph_ui::node_ui::NodeUi`]. This module is the grid
//! orchestration and per-cell rendering; the sensing glyph each cell terminates a wire on is
//! the shared [`PortGlyph`] widget's.

use std::fmt;
use std::fmt::Display;
use std::fmt::Formatter;
use std::sync::Arc;

use glam::Vec2;
use palantir::{
    Align, Configure, Grid, InternedStr, Menu, MenuItem, Panel, Sense, Sizing, Spacing, Text,
    TextStyle, Track, Ui, VAlign, WidgetId, fmt,
};
use scenarium::Binding;
use scenarium::FuncEvent;
use scenarium::InputPort;
use scenarium::Library;
use scenarium::NodeId;
use scenarium::{ConstValue, DataType, FsPathMode};

use crate::core::edit::graph_intent::GraphIntent;
use crate::core::preview_sink;
use crate::gui::frame::app_command::AppCommand;
use crate::gui::frame::app_command::{EditCommand, PathPick};
use crate::gui::graph_ctx::input_ctx::InputCtx;
use crate::gui::graph_ctx::node_ctx::NodeCtx;
use crate::gui::graph_ctx::output_ctx::OutputCtx;
use crate::gui::pane::graph_ui::gesture::preview_drag::PreviewSpawn;
use crate::gui::pane::graph_ui::node_ui::const_editor::{ConstEditor, Field};
use crate::gui::pane::graph_ui::node_ui::node_record::NodeRecord;
use crate::gui::pane::graph_ui::wid;
use crate::gui::port_ref::PortKind;
use crate::gui::theme::Theme;
use crate::gui::theme::hover::Hover;
use crate::gui::widgets::port_glyph;
use crate::gui::widgets::port_glyph::PortGlyph;
use crate::gui::widgets::support;
use scenarium::EventPort;

/// Grid columns: inputs (hug), input values (hug, capped at `max_width`, so a very long
/// editor ellipsizes; the numeric `DragValue` editor caps itself), a fill spacer, then outputs
/// (hug). The outputs sit in a hug column, not the fill, so the grid's content size includes
/// them: a `fill` column contributes 0 to a hug-sized grid and would collapse, spilling the
/// outputs out of the node (palantir `grid_hug_grid_collapses_fill_tracks`). The fill spacer
/// claims any width beyond the ports, pushing the outputs to the node's right edge.
const COL_INPUT: u16 = 0;
const COL_VALUE: u16 = 1;
/// Claims the width beyond the ports; nothing is placed in it.
const COL_SPACER: u16 = 2;
const COL_OUTPUT: u16 = COL_SPACER + 1;

/// `row_tracks` is [`NodeUi`](crate::gui::pane::graph_ui::node_ui::NodeUi)'s retained staging
/// buffer.
pub(super) fn show(ui: &mut Ui, record: &mut NodeRecord<'_, '_>, row_tracks: &mut Vec<Track>) {
    let (theme, node) = (record.node.theme(), record.node);
    // Events list under the outputs in the same column: a row per output and per event.
    let row_count = node
        .port_count(PortKind::Input)
        .max(node.port_count(PortKind::Output) + node.events().len());
    if row_count == 0 {
        return;
    }
    // Fixed-height rows so a node's ports stay uniform whether or not an input carries an inline
    // editor. Every row of every node gets the same track, so the buffer is rebuilt only when a
    // node with more rows needs more of them or the theme moves the height.
    let track = Track::fixed(theme.row_height());
    if row_tracks.len() < row_count || row_tracks.first() != Some(&track) {
        row_tracks.clear();
        row_tracks.resize(row_count, track);
    }
    Grid::new()
        .id_salt("ports")
        .size((Sizing::FILL, Sizing::HUG))
        .cols([
            Track::HUG,
            Track::HUG.with_max(theme.const_editor.rest.max_width),
            Track::FILL,
            Track::HUG,
        ])
        .rows(&row_tracks[..row_count])
        .line_gap(theme.port.gap)
        .gap(theme.port.cols_gap)
        .padding(Spacing::new(
            theme.port.col_pad_x,
            theme.port.gap,
            theme.port.col_pad_x,
            theme.port.gap,
        ))
        .show(ui, |ui| {
            input_cells(ui, record);
            output_cells(ui, record);
        });
}

/// A port's hover tooltip: its `description` (the resolved [`InputCtx::description`] text,
/// empty when the func declares none) above a dimmer type line, else just the type.
#[derive(Debug, Clone, Copy)]
struct PortTooltip<'a> {
    description: &'a str,
    data_type: &'a DataType,
}

impl PortTooltip<'_> {
    /// The tooltip for a port of `node_ctx`, interned straight into this pass's text arena, so
    /// no `String` sits between it and the widget. Built only for the node under the pointer;
    /// `None` off that node, which [`support::tooltip_after`] and [`PortLabel`] take as "no
    /// tooltip".
    fn intern(self, ui: &mut Ui, node_ctx: NodeCtx<'_>) -> Option<InternedStr> {
        let Self {
            description,
            data_type,
        } = self;
        if !node_ctx.tooltips() {
            return None;
        }
        let type_label = TypeLabel {
            library: node_ctx.graph_ctx.library(),
            data_type,
        };
        Some(if description.is_empty() {
            fmt!(ui, "{type_label}")
        } else {
            fmt!(ui, "{description}\n{type_label}")
        })
    }
}

/// A port's label, `name`, with `tooltip` (the port's data type) as its hover
/// tooltip; `None` means no tooltip, as [`PortTooltip::intern`] answers off the
/// hovered node.
#[derive(Debug, Clone, Copy)]
struct PortLabel<'a> {
    name: &'a str,
    tooltip: Option<InternedStr>,
}

impl PortLabel<'_> {
    /// Opts into [`Sense::HOVER`] rather than capturing clicks: the label needs a trigger anchor
    /// for the tooltip, but the node body below it owns selection and drag. Muted ink, as the
    /// value column is each row's strong element.
    fn show(self, ui: &mut Ui, theme: &Theme) {
        let label = Text::new(self.name)
            .style(&label_style(ui, theme))
            .sense(Sense::HOVER)
            .show(ui);
        support::tooltip_after(ui, &label, self.tooltip);
    }
}

fn input_cells(ui: &mut Ui, record: &mut NodeRecord<'_, '_>) {
    for input in record.node.inputs() {
        input_label_cell(ui, record, input);
        value_cell(ui, record, input);
    }
}

fn output_cells(ui: &mut Ui, record: &mut NodeRecord<'_, '_>) {
    let node = record.node;
    let output_count = node.port_count(PortKind::Output);
    for output in node.outputs() {
        output_cell(ui, record, output);
    }
    // Events emit from the same (right) side, in the rows directly below the data outputs.
    for (idx, event) in node.events().iter().enumerate() {
        EventRow {
            idx,
            row: output_count + idx,
            event,
        }
        .show(ui, record);
    }
}

/// The grid row of port or event row `idx`.
fn grid_row(idx: usize) -> u16 {
    u16::try_from(idx).expect("a node's rows fit the grid's u16 index")
}

/// Open `menu_id`'s context menu at the pointer, shared by the input and output cells, on a
/// frame the cell or its port circle was secondary-clicked.
///
/// The caller reads the click first, as the cell's `Response` borrows `ui`. It is the cell's
/// right-click or the port glyph's: the glyph consumes hits over its rect, so the cell alone
/// misses a right-click landed on the circle (no bubbling).
fn open_port_context_menu(ui: &mut Ui, menu_id: WidgetId) {
    if let Some(p) = ui.pointer_pos() {
        Menu::open(ui, menu_id, p);
    }
}

/// The ink a port or event label wears: the ports' muted label ink.
fn label_style(ui: &Ui, theme: &Theme) -> TextStyle {
    TextStyle {
        color: theme.port.label,
        ..ui.theme().text
    }
}

/// Column 0: the input port circle and label, plus the right-click binding menu, anchored here
/// so right-clicking the circle or label opens it. The circle's `WidgetId` is
/// `wid::port_circle(port)`, so `CanvasGeometry`, snap and draw reconstruct it.
fn input_label_cell(ui: &mut Ui, record: &mut NodeRecord<'_, '_>, input: InputCtx<'_>) {
    let (node_ctx, draw_ctx) = (record.node, record.stage.ctx);
    let theme = node_ctx.theme();
    let port = input.port_ref();
    let tooltip = PortTooltip {
        description: input.description(),
        data_type: input.data_type(),
    }
    .intern(ui, node_ctx);
    let label = PortLabel {
        name: input.name(),
        tooltip,
    };
    let fill = Hover::from(draw_ctx.geometry().ports.is_hovered(port))
        .lift(theme.port_color(input.data_type()));
    // Flag a port only once a run actually failed on it, so it does not flip as you bind/unbind:
    // the run named the exact ports it could not feed, and only while the node's status is that
    // verdict. A ring in the warning ink around the type's own hue keeps the type readable. A
    // required input's port reads as bigger: a disc of the footprint an optional input's circle
    // and ring take. An optional one is the plain circle in a ring of the body's fill.
    let (diameter, outline) = if input.missing() {
        (theme.port.size, Some(theme.status.warning))
    } else if input.required() {
        (
            port_glyph::enlarged_diameter(theme.port.size, theme.stroke.line),
            None,
        )
    } else {
        (theme.port.size, Some(theme.card.fill))
    };
    let margin = theme.port_margin(PortKind::Input, diameter * 0.5);
    let wid = wid::port_circle(port);
    // The label area answers a double-click of its own, as the circle has its own id; also the
    // context-menu anchor.
    let cell = Panel::hstack()
        .id(wid::input_cell(port))
        .grid_cell((grid_row(port.port_idx), COL_INPUT))
        .align(Align::LEFT)
        .size((Sizing::HUG, Sizing::HUG))
        .sense(Sense::CLICK)
        .gap(theme.space.xs)
        .child_align(Align::v(VAlign::Center))
        .show(ui, |ui| {
            // A const-only input can't be wired, so it has no wire anchor: just the label.
            if input.const_only() {
                label.show(ui, theme);
                None
            } else {
                let mut circle = PortGlyph::circle(wid, diameter, fill)
                    .margin(margin)
                    .tooltip(tooltip);
                if let Some(color) = outline {
                    circle = circle.outline(color);
                }
                if *input.data_type() == DataType::Any {
                    circle = circle.hollow();
                }
                let glyph = circle.show(ui, theme);
                label.show(ui, theme);
                Some(glyph)
            }
        });
    // Open on right-click anywhere on the cell — circle or label.
    let glyph = cell.inner;
    let menu_id = cell.response.id;
    if cell.response.right.clicked() || glyph.is_some_and(|glyph| glyph.right.clicked()) {
        open_port_context_menu(ui, menu_id);
    }
    // Double-click on the circle or the label toggles the binding: clear it,
    // or seed the default const when unbound.
    //
    // Adding or removing a `Const` resizes the node. The intent drains at the end of this record
    // pass, and a double-click is action input, which always earns a second pass, so the body
    // re-arranges at its settled size with its wires re-anchored before the frame paints.
    if cell.response.left.double_clicked() || glyph.is_some_and(|glyph| glyph.left.double_clicked())
    {
        match input.binding() {
            // Boundary ports route the interface — no const affordance, so an
            // unbound one has nothing to seed (its label double-click renames).
            None => {
                if let Some(default) = input.default() {
                    record.stage.out.push_graph(GraphIntent::set_input(
                        input.port(),
                        Binding::Const(default),
                    ));
                }
            }
            Some(_) => record
                .stage
                .out
                .push_graph(GraphIntent::set_input(input.port(), None)),
        }
    }
    Menu::for_id(menu_id)
        .size((Sizing::HUG, Sizing::HUG))
        .show(ui, |ui| {
            // Resolved once for both the enable test and the value the pick pushes.
            let default = input.default();
            let can_set = !matches!(input.binding(), Some(Binding::Const(_))) && default.is_some();
            if MenuItem::new("Set constant")
                .disabled(!can_set)
                .show(ui)
                .left
                .clicked()
                && let Some(value) = default
            {
                record
                    .stage
                    .out
                    .push_graph(GraphIntent::set_input(input.port(), Binding::Const(value)));
            }
            if MenuItem::new("Clear binding")
                .disabled(input.binding().is_none())
                .show(ui)
                .left
                .clicked()
            {
                record
                    .stage
                    .out
                    .push_graph(GraphIntent::set_input(input.port(), None));
            }
        });
}

/// Column 1: the inline const editor for an input bound to a `Const`, and
/// [`UNSET`](crate::gui::widgets::format::shown_value::UNSET) for one bound to
/// nothing that could hold one. A hug-sized column, so every editor starts at
/// the same x.
fn value_cell(ui: &mut Ui, record: &mut NodeRecord<'_, '_>, input: InputCtx<'_>) {
    let node_ctx = record.node;
    let port = input.port();
    let editor_id = wid::const_editor(port);
    // The one owner of the "only Const bindings get an inline editor" filter: a wired input's
    // value is its wire, and one that takes only a wire has no field to leave empty, so neither
    // renders a value cell.
    let value = match input.binding() {
        Some(Binding::Const(value)) => value,
        Some(Binding::Wire(_)) => return,
        None if !input.takes_const() => return,
        None => {
            value_column(ui, port, |ui| {
                Field {
                    theme: node_ctx.const_editor_theme(),
                    id: editor_id,
                }
                .empty(ui);
            });
            return;
        }
    };
    let data_type = input.data_type();
    let value_variants = input.value_variants();
    // An `FsPath` editor draws a pick button; clicking it raises a blocking file dialog, which
    // only `App` can run, so this is the app tier. Whether a click means "pick a path" is a
    // question about the port's type, which decided to draw the button, so it is answered here
    // rather than by a canvas-level sweep.
    if ui.response_for(editor_id).clicked()
        && matches!(value, ConstValue::FsPath(_) | ConstValue::FsPaths(_))
        && let DataType::FsPath(config) = data_type
    {
        record
            .stage
            .out
            .push_app(AppCommand::Edit(EditCommand::PickInputPath(PathPick {
                port,
                config: Arc::clone(config),
            })));
    }
    let edited = value_column(ui, port, |ui| {
        ConstEditor {
            field: Field {
                theme: node_ctx.const_editor_theme(),
                id: editor_id,
            },
            library: node_ctx.graph_ctx.library(),
            value,
            data_type,
            value_variants,
        }
        .show(ui)
    });
    if let Some(new_value) = edited {
        record
            .stage
            .out
            .push_graph(GraphIntent::set_input(port, Binding::Const(new_value)));
    }
}

/// The value column's cell of the input at `port`: filled, so every editor is the same width
/// (the column hugs to the widest editor's content). `min_size` on the editors keeps a floor.
fn value_column<R>(ui: &mut Ui, port: InputPort, body: impl FnOnce(&mut Ui) -> R) -> R {
    Panel::hstack()
        .id_salt(("val", port.port_idx))
        .grid_cell((grid_row(port.port_idx), COL_VALUE))
        .size(Sizing::FILL)
        .child_align(Align::v(VAlign::Center))
        .show(ui, body)
        .inner
}

/// Column 3: the output label and circle, right-aligned (the fill column pins it to the node's
/// right edge); the circle overhangs that edge.
fn output_cell(ui: &mut Ui, record: &mut NodeRecord<'_, '_>, output: OutputCtx<'_>) {
    let (node_ctx, draw_ctx) = (record.node, record.stage.ctx);
    let theme = node_ctx.theme();
    let port = output.port_ref();
    let data_type = output.data_type();
    let fill =
        Hover::from(draw_ctx.geometry().ports.is_hovered(port)).lift(theme.port_color(data_type));
    let tooltip = PortTooltip {
        description: output.description(),
        data_type,
    }
    .intern(ui, node_ctx);
    let wid = wid::port_circle(port);
    let margin = theme.port_margin(PortKind::Output, theme.port.radius());
    let cell = Panel::hstack()
        .id_salt(("out", port.port_idx))
        .grid_cell((grid_row(port.port_idx), COL_OUTPUT))
        .align(Align::RIGHT)
        .size((Sizing::HUG, Sizing::HUG))
        .sense(Sense::CLICK)
        .gap(theme.space.xs)
        .child_align(Align::v(VAlign::Center))
        .show(ui, |ui| {
            PortLabel {
                name: output.name(),
                tooltip,
            }
            .show(ui, theme);
            PortGlyph::circle(wid, theme.port.size, fill)
                .margin(margin)
                .tooltip(tooltip)
                .show(ui, theme)
        });
    // Right-click anywhere on the cell opens the port menu, as on the input side.
    let glyph = cell.inner;
    let menu_id = cell.response.id;
    // An output may feed many inputs, so a double-click on its circle clears each consumer. Same
    // timing as the input side: the second pass a double-click earns re-arranges every node that
    // lost a wire.
    if glyph.left.double_clicked() {
        record.stage.out.extend_graph(
            node_ctx
                .graph_ctx
                .wires()
                .filter(|(_, producer)| *producer == output.port())
                .map(|(consumer, _)| GraphIntent::set_input(consumer, None)),
        );
    }
    if cell.response.right.clicked() || glyph.right.clicked() {
        open_port_context_menu(ui, menu_id);
    }
    // "Add preview" spawns an ordinary preview node already wired to this output. Hidden when
    // the library has no preview func.
    Menu::for_id(menu_id)
        .size((Sizing::HUG, Sizing::HUG))
        .show(ui, |ui| {
            if let Some(func) = preview_sink::registered(node_ctx.graph_ctx.library())
                && MenuItem::new("Add preview").show(ui).clicked()
            {
                // Off the port once its center is known (after the first frame); otherwise at the
                // origin.
                let pos = draw_ctx
                    .geometry()
                    .ports
                    .center(port)
                    .map_or(Vec2::ZERO, |center| center + PREVIEW_SPAWN_OFFSET);
                record.stage.out.extend_graph(
                    PreviewSpawn {
                        port: output.port(),
                        pos,
                        node_id: NodeId::unique(),
                    }
                    .intents(func),
                );
            }
        });
}

/// Where a preview lands relative to the port it was added from: clear of the node body and a
/// little above.
const PREVIEW_SPAWN_OFFSET: Vec2 = Vec2::new(80.0, -60.0);

/// One event (emitter) port row: the event name plus an event-colored triangle glyph,
/// right-aligned and overhanging the node edge like a data output, in `COL_OUTPUT` below the
/// data outputs. The glyph senses drags so a wire can be pulled from it to a subscriber pin
/// (see `SubscriptionUi`).
#[derive(Debug, Clone, Copy)]
struct EventRow<'a> {
    /// The event's index among the node's events.
    idx: usize,
    /// The grid row it sits in.
    row: usize,
    event: &'a FuncEvent,
}

impl EventRow<'_> {
    fn show(self, ui: &mut Ui, record: &NodeRecord<'_, '_>) {
        let Self { idx, row, event } = self;
        let (node_ctx, draw_ctx) = (record.node, record.stage.ctx);
        let theme = node_ctx.theme();
        let node_id = node_ctx.node_id;
        let margin = theme.port_margin(PortKind::Output, theme.port.radius());
        let ev = EventPort::new(node_id, idx);
        let wid = wid::event_glyph(ev);
        let fill = Hover::from(draw_ctx.geometry().events.is_hovered(ev)).lift(theme.port.event);
        let tooltip = node_ctx
            .tooltips()
            .then(|| fmt!(ui, "event: {}", event.name));
        Panel::hstack()
            .id_salt(("event", idx))
            .grid_cell((grid_row(row), COL_OUTPUT))
            .align(Align::RIGHT)
            .size((Sizing::HUG, Sizing::HUG))
            .gap(theme.space.xs)
            .child_align(Align::v(VAlign::Center))
            .show(ui, |ui| {
                // Muted like the data-port labels.
                Text::new(event.name.as_str())
                    .style(&label_style(ui, theme))
                    .show(ui);
                PortGlyph::arrow(wid, theme.port.size, fill)
                    .margin(margin)
                    .tooltip(tooltip)
                    .show(ui, theme);
            });
    }
}

/// Human-readable type for a port tooltip: the picker mode (and accepted extensions) for a
/// path, and the library's own display name for everything else (`any` for the untyped boundary
/// placeholder, a registered type's name, or the raw id when nothing is registered).
///
/// Renders on demand, as [`PortTooltip::intern`] feeds it straight to `fmt!`.
#[derive(Debug)]
struct TypeLabel<'a> {
    library: &'a Library,
    data_type: &'a DataType,
}

impl Display for TypeLabel<'_> {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        let DataType::FsPath(config) = self.data_type else {
            // Borrowed for every type the library knows; only an unregistered id builds a string.
            return f.write_str(&self.library.type_name(self.data_type));
        };
        let mode = match config.mode {
            FsPathMode::Dir => "directory",
            FsPathMode::ExistingFile => "file",
            FsPathMode::ExistingFiles => "files",
            FsPathMode::NewFile => "save path",
        };
        write!(f, "path · {mode}")?;
        // Written one at a time, not `join`ed, which would build the intermediate string.
        if let Some((first, rest)) = config.extensions.split_first() {
            write!(f, " ({first}")?;
            for ext in rest {
                write!(f, ", {ext}")?;
            }
            f.write_str(")")?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests;
