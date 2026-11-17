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

use glam::Vec2;
use palantir::{
    Align, Configure, Grid, InternedStr, Menu, MenuItem, Panel, Sense, SizeSpec, Spacing, Text,
    TextStyle, TextWrap, Track, Ui, VAlign, WidgetId, fmt,
};
use scenarium::Binding;
use scenarium::NodeId;
use scenarium::{ConstValue, DataType};
use scenarium::{GhostBinding, GhostSource, InputState};
use scenarium::{GraphSet, Library};

use crate::core::edit::graph_intent::GraphIntent;
use crate::core::preview_sink;
use crate::gui::frame::app_command::AppCommand;
use crate::gui::frame::app_command::{EditCommand, PathPick};
use crate::gui::graph_ctx::ghost_port::{GhostEvent, GhostPort};
use crate::gui::graph_ctx::input_ctx::InputCtx;
use crate::gui::graph_ctx::node_ctx::NodeCtx;
use crate::gui::graph_ctx::output_ctx::OutputCtx;
use crate::gui::pane::graph_ui::gesture::preview_drag::PreviewSpawn;
use crate::gui::pane::graph_ui::node_ui::const_editor::{ConstEditor, Field};
use crate::gui::pane::graph_ui::node_ui::node_record::NodeRecord;
use crate::gui::pane::graph_ui::wid;
use crate::gui::path_type::PathType;
use crate::gui::port_ref::PortKind;
use crate::gui::theme::Theme;
use crate::gui::theme::color;
use crate::gui::theme::hover::Hover;
use crate::gui::widgets::port_glyph;
use crate::gui::widgets::port_glyph::PortGlyph;
use crate::gui::widgets::support;
use scenarium::{EventPort, EventRef, TriggerPort};

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
    // Events list under the outputs in the same column: a row per output and per event. The
    // ghost ports list under the declared ones on their side.
    let row_count = (node.port_count(PortKind::Input)
        + node.trigger_rows().len()
        + node.ghost_inputs().count())
    .max(
        node.port_count(PortKind::Output)
            + node.events().len()
            + node.ghost_outputs().count()
            + node.ghost_events().count(),
    );
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
        .size(SizeSpec::FILL_W)
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
    data_type: DataType,
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
            graphs: &node_ctx.graph_ctx.document().graphs,
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
    let node = record.node;
    for input in node.inputs() {
        input_label_cell(ui, record, input);
        value_cell(ui, record, input);
    }
    // Triggers land on the same (left) side, in the rows directly below the data inputs.
    let input_count = node.port_count(PortKind::Input);
    for (idx, trigger) in node.trigger_rows().iter().enumerate() {
        TriggerRow {
            row: input_count + idx,
            trigger: TriggerPort::new(node.node_id, trigger.id),
            name: &trigger.name,
        }
        .show(ui, record);
    }
    let declared = input_count + node.trigger_rows().len();
    for (idx, ghost) in node.ghost_inputs().enumerate() {
        GhostRow {
            row: declared + idx,
            ghost: GhostRowPort::Data(ghost),
        }
        .show(ui, record);
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
            row: output_count + idx,
            event,
        }
        .show(ui, record);
    }
    let declared = output_count + node.events().len();
    let ghosts = node
        .ghost_outputs()
        .map(GhostRowPort::Data)
        .chain(node.ghost_events().map(GhostRowPort::Event));
    for (idx, ghost) in ghosts.enumerate() {
        GhostRow {
            row: declared + idx,
            ghost,
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
        Menu::open_at(ui, menu_id, p);
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
        .lift(node_ctx.graph_ctx.port_color(input.data_type()));
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
    let wid = wid::port_circle(record.node.graph_ctx.graph(), port);
    // The label area answers a double-click of its own, as the circle has its own id; also the
    // context-menu anchor.
    let cell = Panel::hstack()
        .id(wid::input_cell(record.node.graph_ctx.graph(), port))
        .grid_cell((grid_row(input.idx()), COL_INPUT))
        .align(Align::LEFT)
        .size(SizeSpec::HUG)
        .sense(Sense::CLICK)
        .gap(theme.space.xs)
        .child_align(VAlign::Center)
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
                if input.data_type() == DataType::Any {
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
    // Double-click on the circle or the label toggles the binding: clear it, a ghost included,
    // or seed the default const when unbound.
    //
    // Adding or removing a `Const` resizes the node. The intent drains at the end of this record
    // pass, and a double-click is action input, which always earns a second pass, so the body
    // re-arranges at its settled size with its wires re-anchored before the frame paints.
    if cell.response.left.double_clicked() || glyph.is_some_and(|glyph| glyph.left.double_clicked())
    {
        match input.binding() {
            None if input.ghost().is_some() => record.stage.out.push_graph(
                record.node.graph_ctx.graph(),
                GraphIntent::set_input(input.port(), None),
            ),
            // Boundary ports route the interface — no const affordance, so an
            // unbound one has nothing to seed (its label double-click renames).
            None => {
                if let Some(default) = input.default() {
                    record.stage.out.push_graph(
                        record.node.graph_ctx.graph(),
                        GraphIntent::set_input(input.port(), Binding::Const(default)),
                    );
                }
            }
            Some(_) => record.stage.out.push_graph(
                record.node.graph_ctx.graph(),
                GraphIntent::set_input(input.port(), None),
            ),
        }
    }
    Menu::for_id(menu_id).size(SizeSpec::HUG).show(ui, |ui| {
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
            record.stage.out.push_graph(
                record.node.graph_ctx.graph(),
                GraphIntent::set_input(input.port(), Binding::Const(value)),
            );
        }
        // A ghost the graph can hold again, as its loop is gone, comes back as the wire or the
        // constant it was, through the gate as any bind.
        if let Some(ghost) = input.ghost() {
            let graph_ctx = record.node.graph_ctx;
            let restorable = graph_ctx
                .body()
                .admit_ghost(graph_ctx.scope(), input.port())
                .is_ok();
            if MenuItem::new("Restore")
                .disabled(!restorable)
                .show(ui)
                .left
                .clicked()
            {
                record.stage.out.push_graph(
                    graph_ctx.graph(),
                    GraphIntent::set_input(input.port(), ghost.binding()),
                );
            }
        }
        if MenuItem::new("Clear binding")
            .disabled(input.binding().is_none() && input.ghost().is_none())
            .show(ui)
            .left
            .clicked()
        {
            record.stage.out.push_graph(
                record.node.graph_ctx.graph(),
                GraphIntent::set_input(input.port(), None),
            );
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
    let editor_id = wid::const_editor(record.node.graph_ctx.graph(), port);
    // The one owner of the "only Const bindings get an inline editor" filter: a wired input's
    // value is its wire, and one that takes only a wire has no field to leave empty, so neither
    // renders a value cell.
    // An input a ghost sits on reads as wired, a ghost wire, or shows what it kept, a ghost
    // constant: either is replaced by a bind and cleared by a double-click.
    if input.binding().is_none()
        && let Some(ghost) = input.ghost()
    {
        if let GhostSource::Const(value) = &ghost.source {
            value_column(ui, input, |ui| kept_value(ui, node_ctx.theme(), value));
        }
        return;
    }
    let value = match input.binding() {
        Some(Binding::Const(value)) => value,
        Some(Binding::Wire(_)) => return,
        None if !input.takes_const() => return,
        None => {
            value_column(ui, input, |ui| {
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
    // A path editor draws a pick button; clicking it raises a blocking file dialog, which
    // only `App` can run, so this is the app tier. Whether a click means "pick a path" is a
    // question about the port's type, which decided to draw the button, so it is answered here
    // rather than by a canvas-level sweep.
    if ui.response_for(editor_id).clicked()
        && matches!(value, ConstValue::FsPath(_) | ConstValue::FsPaths(_))
        && let Some(path_type) = PathType::of(data_type)
    {
        record
            .stage
            .out
            .push_app(AppCommand::Edit(EditCommand::PickInputPath(PathPick {
                graph: record.node.graph_ctx.graph(),
                port,
                path_type,
            })));
    }
    let edited = value_column(ui, input, |ui| {
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
        record.stage.out.push_graph(
            record.node.graph_ctx.graph(),
            GraphIntent::set_input(port, Binding::Const(new_value)),
        );
    }
}

/// The value column's cell of `input`: filled, so every editor is the same width
/// (the column hugs to the widest editor's content). `min_size` on the editors keeps a floor.
fn value_column<R>(ui: &mut Ui, input: InputCtx<'_>, body: impl FnOnce(&mut Ui) -> R) -> R {
    Panel::hstack()
        .id_salt(("val", input.port().port))
        .grid_cell((grid_row(input.idx()), COL_VALUE))
        .size(SizeSpec::FILL)
        .child_align(VAlign::Center)
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
    let fill = Hover::from(draw_ctx.geometry().ports.is_hovered(port))
        .lift(node_ctx.graph_ctx.port_color(data_type));
    let tooltip = PortTooltip {
        description: output.description(),
        data_type,
    }
    .intern(ui, node_ctx);
    let wid = wid::port_circle(record.node.graph_ctx.graph(), port);
    let margin = theme.port_margin(PortKind::Output, theme.port.radius());
    let cell = Panel::hstack()
        .id_salt(("out", port.port))
        .grid_cell((grid_row(output.idx()), COL_OUTPUT))
        .align(Align::RIGHT)
        .size(SizeSpec::HUG)
        .sense(Sense::CLICK)
        .gap(theme.space.xs)
        .child_align(VAlign::Center)
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
            record.node.graph_ctx.graph(),
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
    Menu::for_id(menu_id).size(SizeSpec::HUG).show(ui, |ui| {
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
                record.node.graph_ctx.graph(),
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
    /// The grid row it sits in.
    row: usize,
    event: EventRef<'a>,
}

impl EventRow<'_> {
    fn show(self, ui: &mut Ui, record: &NodeRecord<'_, '_>) {
        let Self { row, event } = self;
        let (node_ctx, draw_ctx) = (record.node, record.stage.ctx);
        let theme = node_ctx.theme();
        let node_id = node_ctx.node_id;
        let margin = theme.port_margin(PortKind::Output, theme.port.radius());
        let ev = EventPort::new(node_id, event.id);
        let wid = wid::event_glyph(node_ctx.graph_ctx.graph(), ev);
        let fill = Hover::from(draw_ctx.geometry().events.is_hovered(ev)).lift(theme.port.event);
        let tooltip = node_ctx
            .tooltips()
            .then(|| fmt!(ui, "event: {}", event.name));
        Panel::hstack()
            .id_salt(("event", event.id))
            .grid_cell((grid_row(row), COL_OUTPUT))
            .align(Align::RIGHT)
            .size(SizeSpec::HUG)
            .gap(theme.space.xs)
            .child_align(VAlign::Center)
            .show(ui, |ui| {
                // Muted like the data-port labels.
                Text::new(event.name)
                    .style(&label_style(ui, theme))
                    .show(ui);
                PortGlyph::arrow(wid, theme.port.size, fill)
                    .margin(margin)
                    .tooltip(tooltip)
                    .show(ui, theme);
            });
    }
}

/// One trigger an `Outputs` node hears, in the input column: the event arrow pointing in, where
/// a subscription inside the compound passes out as the interface event of its name.
#[derive(Debug)]
struct TriggerRow<'a> {
    /// The grid row it sits in.
    row: usize,
    trigger: TriggerPort,
    name: &'a str,
}

impl TriggerRow<'_> {
    fn show(self, ui: &mut Ui, record: &NodeRecord<'_, '_>) {
        let Self { row, trigger, name } = self;
        let (node_ctx, draw_ctx) = (record.node, record.stage.ctx);
        let theme = node_ctx.theme();
        let margin = theme.port_margin(PortKind::Input, theme.port.radius());
        let wid = wid::trigger_glyph(node_ctx.graph_ctx.graph(), trigger);
        let fill =
            Hover::from(draw_ctx.geometry().triggers.is_hovered(trigger)).lift(theme.port.event);
        let tooltip = node_ctx.tooltips().then(|| fmt!(ui, "trigger: {name}"));
        Panel::hstack()
            .id_salt(("trigger", trigger.trigger))
            .grid_cell((grid_row(row), COL_INPUT))
            .align(Align::LEFT)
            .size(SizeSpec::HUG)
            .gap(theme.space.xs)
            .child_align(VAlign::Center)
            .show(ui, |ui| {
                PortGlyph::arrow(wid, theme.port.size, fill)
                    .margin(margin)
                    .tooltip(tooltip)
                    .show(ui, theme);
                Text::new(name).style(&label_style(ui, theme)).show(ui);
            });
    }
}

/// A ghost row's port: a data port's, an input or an output, or an event's.
#[derive(Debug, Clone, Copy)]
enum GhostRowPort<'a> {
    Data(GhostPort<'a>),
    Event(GhostEvent<'a>),
}

/// One row for a port its node shows and its declaration does not: a hollow dot in the hue of
/// the type it had, where a ghost wire ends, and its name dimmed, or its number where no save
/// kept the name; an input's kept constant beside it. Nothing wires to it: a double-click on an
/// input's clears what it holds, as on a declared input.
#[derive(Debug, Clone, Copy)]
struct GhostRow<'a> {
    /// The grid row it sits in.
    row: usize,
    ghost: GhostRowPort<'a>,
}

impl GhostRow<'_> {
    fn show(self, ui: &mut Ui, record: &mut NodeRecord<'_, '_>) {
        let Self { row, ghost } = self;
        let (node_ctx, draw_ctx) = (record.node, record.stage.ctx);
        let (graph_ctx, theme) = (node_ctx.graph_ctx, node_ctx.theme());
        let graph = graph_ctx.graph();
        let geometry = draw_ctx.geometry();
        let ink = color::wash(theme.port.label, GHOST_INK, theme.card.fill);
        let (side, wid, hovered, name, number, data_type, held) = match ghost {
            GhostRowPort::Data(port) => (
                port.port.kind,
                wid::ghost_port(graph, port.port),
                geometry.ghost_ports.is_hovered(port.port),
                port.name,
                port.port.port.get(),
                port.data_type,
                port.held,
            ),
            GhostRowPort::Event(event) => (
                PortKind::Output,
                wid::ghost_event(graph, event.event),
                geometry.ghost_events.is_hovered(event.event),
                event.name,
                event.event.event.get(),
                None,
                None,
            ),
        };
        let dot = match ghost {
            GhostRowPort::Data(_) => graph_ctx.ghost_color(data_type),
            GhostRowPort::Event(_) => theme.port.event,
        };
        let fill = Hover::from(hovered).lift(dot);
        let tooltip = node_ctx
            .tooltips()
            .then(|| match (data_type, node_ctx.ghost()) {
                (Some(data_type), _) => {
                    let type_label = TypeLabel {
                        graphs: &graph_ctx.document().graphs,
                        library: graph_ctx.library(),
                        data_type,
                    };
                    fmt!(ui, "{type_label}\nkept as a ghost")
                }
                (None, true) => fmt!(ui, "kept while the node is a ghost"),
                (None, false) => fmt!(ui, "no longer a port of this node, kept as a ghost"),
            });
        let label = |ui: &mut Ui| {
            let style = TextStyle {
                color: ink,
                ..ui.theme().text
            };
            match name {
                Some(name) => Text::new(name).style(&style).show(ui),
                None => Text::new(fmt!(ui, "#{number}")).style(&style).show(ui),
            };
        };
        let margin = theme.port_margin(side, theme.port.radius());
        let glyph = |ui: &mut Ui| {
            let glyph = match ghost {
                GhostRowPort::Data(_) => PortGlyph::circle(wid, theme.port.size, fill).hollow(),
                GhostRowPort::Event(_) => PortGlyph::arrow(wid, theme.port.size, fill),
            };
            glyph.margin(margin).tooltip(tooltip).show(ui, theme)
        };
        let (col, align) = match side {
            PortKind::Input => (COL_INPUT, Align::LEFT),
            PortKind::Output => (COL_OUTPUT, Align::RIGHT),
        };
        // Keyed by domain as the glyph is: an output and an event of one number are two rows.
        let salt = match ghost {
            GhostRowPort::Data(_) => ("ghost_port", side as u8, number),
            GhostRowPort::Event(_) => ("ghost_event", side as u8, number),
        };
        let cell = Panel::hstack()
            .id_salt(salt)
            .grid_cell((grid_row(row), col))
            .align(align)
            .size(SizeSpec::HUG)
            .gap(theme.space.xs)
            .child_align(VAlign::Center)
            .show(ui, |ui| match side {
                PortKind::Input => {
                    let glyph = glyph(ui);
                    label(ui);
                    glyph
                }
                PortKind::Output => {
                    label(ui);
                    glyph(ui)
                }
            });
        if let Some(GhostBinding {
            source: GhostSource::Const(value),
            ..
        }) = held
        {
            Panel::hstack()
                .id_salt(("ghost_value", number))
                .grid_cell((grid_row(row), COL_VALUE))
                .size(SizeSpec::FILL)
                .child_align(VAlign::Center)
                .show(ui, |ui| kept_value(ui, theme, value));
        }
        let (GhostRowPort::Data(port), Some(held)) = (ghost, held) else {
            return;
        };
        let Some(input) = port.port.input_port() else {
            return;
        };
        if cell.inner.left.double_clicked() {
            record
                .stage
                .out
                .push_graph(graph, GraphIntent::set_input(input, None));
        }
        let menu_id = cell.response.id;
        if cell.response.right.clicked() || cell.inner.right.clicked() {
            open_port_context_menu(ui, menu_id);
        }
        Menu::for_id(menu_id).size(SizeSpec::HUG).show(ui, |ui| {
            // Re-home: what the ghost held moves to an input of the node that takes it, in one
            // entry. Only the inputs the gate admits it on are offered.
            for target in rehome_targets(node_ctx, held) {
                let binding = held.binding();
                if MenuItem::new(fmt!(ui, "Move to {}", target.name()))
                    .show(ui)
                    .left
                    .clicked()
                {
                    record.stage.out.extend_graph(
                        graph,
                        [
                            GraphIntent::set_input(target.port(), binding),
                            GraphIntent::set_input(input, None),
                        ],
                    );
                }
            }
            if MenuItem::new("Clear").show(ui).left.clicked() {
                record
                    .stage
                    .out
                    .push_graph(graph, GraphIntent::set_input(input, None));
            }
        });
    }
}

/// The inputs of `node` that would take what `ghost` held, a wire or a constant: each declared
/// input that holds nothing and that the edit gate's admission passes, a wire also closing no
/// loop and reading a node, not a ghost node. What a ghost port's menu offers to move it to.
fn rehome_targets<'a>(
    node: NodeCtx<'a>,
    ghost: &'a GhostBinding,
) -> impl Iterator<Item = InputCtx<'a>> + 'a {
    let graph_ctx = node.graph_ctx;
    let (body, scope) = (graph_ctx.body(), graph_ctx.scope());
    let binding = ghost.binding();
    // A wire from a ghost node goes nowhere until the node is back.
    let producer_held = ghost
        .output()
        .is_none_or(|output| body.find(output.node_id).is_some());
    node.inputs().filter(move |target| {
        let port = target.port();
        producer_held
            && body.input_state(port) == InputState::Unbound
            && body.admit(scope, port, &binding).is_ok()
            && !ghost
                .output()
                .is_some_and(|output| body.produces_cycle(output.node_id, port.node_id))
    })
}

/// How far a ghost row's ink pulls from the label ink toward the card: it reads as kept, under
/// the declared rows.
const GHOST_INK: f32 = 0.55;

/// A ghost constant as its row shows it, dimmed: what was set, which nothing reads now.
fn kept_value(ui: &mut Ui, theme: &Theme, value: &ConstValue) {
    let style = TextStyle {
        color: color::wash(theme.port.label, GHOST_INK, theme.card.fill),
        ..ui.theme().text
    };
    Text::new(fmt!(ui, "kept {}", value.value_text()))
        .style(&style)
        .text_wrap(TextWrap::Ellipsis)
        .show(ui);
}

/// Human-readable type for a port tooltip: a path's kind, mode and extensions
/// ([`PathType::label`]), and the set's name for everything else ([`GraphSet::type_name`]: `any`
/// for the untyped boundary placeholder, a registered type's name, the name a file gave a type
/// the library lacks, or "unknown type").
///
/// Renders on demand, as [`PortTooltip::intern`] feeds it straight to `fmt!`.
#[derive(Debug)]
struct TypeLabel<'a> {
    graphs: &'a GraphSet,
    library: &'a Library,
    data_type: DataType,
}

impl Display for TypeLabel<'_> {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        match PathType::of(self.data_type) {
            Some(path_type) => path_type.label(self.graphs, self.library).fmt(f),
            None => f.write_str(self.graphs.type_name(self.library, self.data_type)),
        }
    }
}

#[cfg(test)]
mod tests;
