//! The ports area of a node body, laid out as a grid: input port+label
//! (col 0), the inline const editor for that input (col 1, so every value
//! lines up regardless of label width), a fill spacer (col 2), and the
//! output port+label (col 3, right-aligned against the node edge). Row `i`
//! holds input `i` and output `i`, so the two sides align. Drawn below the
//! header by [`crate::gui::pane::graph_ui::node_ui::NodeUI`]. This module is the
//! grid orchestration and per-cell rendering; the sensing glyph each cell
//! terminates a wire on — circle, event triangle, hit-box growth — is the
//! shared [`PortGlyph`] widget's.

use std::fmt;
use std::fmt::Display;
use std::fmt::Formatter;
use std::sync::Arc;

use palantir::InternedStr;
use palantir::prelude::*;
use scenarium::Binding;
use scenarium::FuncEvent;
use scenarium::InputPort;
use scenarium::Library;
use scenarium::NodeId;
use scenarium::{ConstValue, DataType, FsPathConfig, FsPathMode, Func};

use crate::core::document::port_ref::{PortKind, PortRef};
use crate::core::edit::graph_intent::GraphIntent;
use crate::core::preview_sink;
use crate::gui::app::app_command::AppCommand;
use crate::gui::app::app_command::edit_command::EditCommand;
use crate::gui::event_ref::EventRef;
use crate::gui::graph_ctx::input_ctx::InputCtx;
use crate::gui::graph_ctx::node_ctx::NodeCtx;
use crate::gui::graph_ctx::output_ctx::OutputCtx;
use crate::gui::pane::graph_ui::node_ui::node_record::NodeRecord;
use crate::gui::pane::graph_ui::node_ui::port_color;
use crate::gui::pane::graph_ui::node_ui::value_editor::{ConstEdit, Field};
use crate::gui::pane::graph_ui::node_ui::wid;
use crate::gui::state::run_state::ExecStatus;
use crate::gui::theme::Theme;
use crate::gui::widgets::hover::Hover;
use crate::gui::widgets::port_glyph;
use crate::gui::widgets::port_glyph::{PortGlyph, PortGlyphResponse};
use crate::gui::widgets::support;
use crate::gui::widgets::support::ROW_GAP;

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

/// `row_tracks` is [`NodeUI`](crate::gui::pane::graph_ui::node_ui::NodeUI)'s retained staging
/// buffer — see its doc for why the grid's rows aren't built fresh here.
pub(super) fn show(ui: &mut Ui, rec: &mut NodeRecord<'_, '_>, row_tracks: &mut Vec<Track>) {
    let (theme, node) = (rec.node.theme(), rec.node);
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
    let track = Track::fixed(ui.theme().text.font_size * PORT_ROW_HEIGHT_EM);
    if row_tracks.len() < n_rows || row_tracks.first() != Some(&track) {
        row_tracks.clear();
        row_tracks.resize(n_rows, track);
    }
    Grid::new()
        .id_salt("ports")
        .size((Sizing::FILL, Sizing::HUG))
        .cols([
            Track::HUG,
            Track::HUG.with_max(theme.const_value_editor.max_width),
            Track::FILL,
            Track::HUG,
        ])
        .rows(&row_tracks[..n_rows])
        .line_gap(theme.ports.gap)
        .gap(theme.ports.cols_gap)
        .padding(Spacing::new(
            theme.ports.col_pad_x,
            theme.ports.gap,
            theme.ports.col_pad_x,
            theme.ports.gap,
        ))
        .show(ui, |ui| {
            input_cells(ui, rec);
            output_cells(ui, rec);
        });
}

/// A port's hover tooltip: its `description` (when the func declares one,
/// empty otherwise — the resolved [`InputCtx::description`] text) above a
/// dimmer type line, else just the type, and below them the input that sets
/// this one aside, if one does.
#[derive(Debug, Clone, Copy)]
struct PortTip<'a> {
    description: &'a str,
    ty: &'a DataType,
    set_aside_by: Option<&'a str>,
}

impl PortTip<'_> {
    /// The tip for a port of `ncx`, interned straight into this pass's text
    /// arena, so a hovered node's ports author their tips with no `String`
    /// between them and the widget.
    ///
    /// Built only for the node under the pointer — see [`show`]. `None` off
    /// that node, which [`support::tooltip_after`] and [`PortLabel`] both take
    /// as "no tooltip".
    fn intern(self, ui: &mut Ui, ncx: NodeCtx<'_>) -> Option<InternedStr> {
        let Self {
            description,
            ty,
            set_aside_by,
        } = self;
        if !ncx.tips() {
            return None;
        }
        let type_label = TypeLabel {
            library: ncx.graph_ctx.library(),
            ty,
        };
        Some(match (description.is_empty(), set_aside_by) {
            (true, None) => fmt!(ui, "{type_label}"),
            (false, None) => fmt!(ui, "{description}\n{type_label}"),
            (true, Some(by)) => fmt!(ui, "{type_label}\nSet aside: \"{by}\" overrides it."),
            (false, Some(by)) => {
                fmt!(
                    ui,
                    "{description}\n{type_label}\nSet aside: \"{by}\" overrides it."
                )
            }
        })
    }
}

/// How a port's label inks: the ports' muted label ink, or fainter still for
/// an input another sets aside.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum LabelInk {
    Plain,
    SetAside,
}

/// A port's label, `name`, with `tip` (the port's data type) as its hover
/// tooltip; `None` means no tooltip, as [`PortTip::intern`] answers off the
/// hovered node.
#[derive(Debug, Clone, Copy)]
struct PortLabel<'a> {
    name: &'a str,
    tip: Option<InternedStr>,
    ink: LabelInk,
}

impl PortLabel<'_> {
    /// Opts into [`Sense::HOVER`] rather than capturing clicks: the label needs
    /// a trigger anchor for the tooltip, but the node body below it owns
    /// selection and drag, so the press has to fall through. Muted ink — the
    /// value column is each row's strong element, not the label.
    fn show(self, ui: &mut Ui, theme: &Theme) {
        let snapshot = Text::new(self.name)
            .style(&label_style(ui, theme, self.ink))
            .sense(Sense::HOVER)
            .show(ui)
            .snapshot();
        support::tooltip_after(ui, &snapshot, self.tip);
    }
}

fn input_cells(ui: &mut Ui, rec: &mut NodeRecord<'_, '_>) {
    for input in rec.node.inputs() {
        input_label_cell(ui, rec, input);
        value_cell(ui, rec, input);
    }
}

fn output_cells(ui: &mut Ui, rec: &mut NodeRecord<'_, '_>) {
    let node = rec.node;
    let output_count = node.port_count(PortKind::Output);
    for output in node.outputs() {
        output_cell(ui, rec, output);
    }
    // Events emit from the same (right) side; list them in the rows directly
    // below the data outputs.
    for (index, event) in node.events().iter().enumerate() {
        EventRow {
            index,
            row: output_count + index,
            event,
        }
        .show(ui, rec);
    }
}

/// The grid row of port or event row `index`.
fn grid_row(index: usize) -> u16 {
    u16::try_from(index).expect("a node's rows fit the grid's u16 index")
}

pub(crate) fn port_circle_wid(port: PortRef) -> WidgetId {
    wid::port("port_circle", port)
}

/// An input port's inline const editor (text field, checkbox, or file-pick
/// button).
fn const_editor_wid(input: InputPort) -> WidgetId {
    wid::port("const_editor", input.into())
}

/// A click on an `FsPath` input's inline pick button: the port to set, and the
/// picker config to open the dialog with.
///
/// A named payload rather than the two values loose, because it travels as an
/// `AppCommand` through `App` and back — the dialog can only run once the
/// record pass is over, so the click and the effect are frames apart.
#[derive(Clone, Debug)]
pub(crate) struct PathPick {
    pub(crate) port: InputPort,
    /// Type-level metadata, taken from the port's `DataType` — the value
    /// carries only the selected path strings.
    pub(crate) config: Arc<FsPathConfig>,
}

/// An input port's cell (circle + label). Answers a double-click on the
/// *label* area — the circle has its own [`port_circle_wid`] and consumes
/// hits over its own rect.
fn input_cell_wid(port: PortRef) -> WidgetId {
    wid::port("input_cell", port)
}

/// Open `menu_id`'s context menu at the pointer — shared by the input and
/// output cells, on a frame the cell or its port circle was secondary-clicked.
///
/// The click is read by the caller before this runs: the cell's `Response`
/// borrows `ui`, and this needs `ui` mutably. It is the cell's right-click
/// *or* the port glyph's — the glyph senses its own `Sense::CLICK` and
/// consumes hits over its rect, so the cell alone misses a right-click landed
/// on the circle (no bubbling).
fn open_port_context_menu(ui: &mut Ui, menu_id: WidgetId) {
    if let Some(p) = ui.pointer_pos() {
        ContextMenu::open(ui, menu_id, p);
    }
}

/// The ink a port or event label wears: the ports' muted label ink, fainter
/// still for an input another sets aside.
fn label_style(ui: &Ui, theme: &Theme, ink: LabelInk) -> TextStyle {
    let color = match ink {
        LabelInk::SetAside => theme.colors.text_muted,
        LabelInk::Plain => theme.ports.label,
    };
    TextStyle {
        color,
        ..ui.theme().text
    }
}

/// Column 0: the input port circle + label, plus the right-click binding
/// menu (anchored here, so right-clicking the circle or label opens it).
/// The circle's `WidgetId` is the deterministic `port_circle_wid(port)`, so
/// `CanvasGeometry`/snap/draw reconstruct it from domain coords.
fn input_label_cell(ui: &mut Ui, rec: &mut NodeRecord<'_, '_>, input: InputCtx<'_>) {
    let (ncx, dcx) = (rec.node, rec.draw);
    let (theme, node) = (ncx.theme(), ncx);
    let port = input.port_ref();
    let set_aside_by = input.overridden_by();
    let ink = if set_aside_by.is_some() {
        LabelInk::SetAside
    } else {
        LabelInk::Plain
    };
    let tip = PortTip {
        description: input.description(),
        ty: input.ty(),
        set_aside_by,
    }
    .intern(ui, ncx);
    let label = PortLabel {
        name: input.name(),
        tip,
        ink,
    };
    // Flag a port only once a run actually failed on it — not on every unbound edit — so
    // the port keeps its data-type color while editing instead of flipping as you
    // bind/unbind. The run named the exact ports it could not feed, so only those light
    // up; the node-level check is what stops a live re-run's stale verdict from lingering
    // once the node reaches a new status.
    let missing = matches!(node.exec_status(), ExecStatus::MissingInputs) && input.missing();
    let fill = if missing {
        theme.status.warning
    } else {
        Hover::from(dcx.geometry().ports.is_hovered(port)).lift(port_color::data(
            theme,
            input.ty(),
            PortKind::Input,
        ))
    };
    // A required input's port reads as bigger — its total footprint matches
    // a bound output's circle-plus-ring, so "important port" carries the
    // same visual weight on either side. An optional input instead gets a
    // muted outline, so "not required" reads at a glance without needing
    // the bigger required-input footprint.
    let diameter = if input.required() {
        port_glyph::enlarged_diameter(theme.ports.size)
    } else {
        theme.ports.size
    };
    // Matches the node body itself — the ring reads as the node's own surface
    // wrapping around the port, rather than a separate accent.
    let outline = (!input.required()).then_some(theme.card.fill);
    let radius = diameter * 0.5;
    let overhang = theme.port_overhang_for(radius);
    let margin = Spacing::new(-overhang, 0.0, 0.0, 0.0);
    let wid = port_circle_wid(port);
    // Stable cell id so the label area answers a double-click of its own (the
    // circle has its own `port_circle_wid`); also the context-menu anchor.
    let cell = Panel::hstack()
        .id(input_cell_wid(port))
        .grid_cell((grid_row(port.port_idx), COL_INPUT))
        .align(Align::new(HAlign::Left, VAlign::Center))
        .size((Sizing::HUG, Sizing::HUG))
        .sense(Sense::CLICK)
        .gap(ROW_GAP)
        .child_align(Align::v(VAlign::Center))
        .show(ui, |ui| {
            // A const-only input can't be wired, so it has no connection anchor
            // — render just the label (+ its inline const editor).
            if input.const_only() {
                label.show(ui, theme);
                PortGlyphResponse::default()
            } else {
                let mut circle = PortGlyph::circle(wid, diameter, fill)
                    .margin(margin)
                    .tip(tip);
                if let Some(color) = outline {
                    circle = circle.outline(color);
                }
                let glyph = circle.show(ui);
                label.show(ui, theme);
                glyph
            }
        });
    // Open on right-click anywhere on the cell — circle or label. Pulled into
    // locals so the response's `&Ui` borrow ends before the reads below.
    let glyph = cell.inner;
    let (menu_id, cell_secondary, cell_double) = (
        cell.response.id,
        cell.response.right.clicked(),
        cell.response.left.double_clicked(),
    );
    if cell_secondary || glyph.secondary_clicked {
        open_port_context_menu(ui, menu_id);
    }
    // Double-click on the circle or the label toggles the binding: clear it,
    // or seed the default const when unbound.
    //
    // Adding or removing a `Const` adds or removes this input's inline editor
    // and so resizes the node. The intent drains at the end of this record
    // pass, and a double-click is action input — which always earns a second
    // pass — so the body re-arranges at its settled size with its wires
    // re-anchored before the frame paints.
    if cell_double || glyph.double_clicked {
        match input.binding() {
            // Boundary ports route the interface — no const affordance, so an
            // unbound one has nothing to seed (its label double-click renames).
            None => {
                if let Some(default) = input.default() {
                    rec.out
                        .push_graph(GraphIntent::set_input(port, Binding::Const(default)));
                }
            }
            Some(_) => rec.out.push_graph(GraphIntent::set_input(port, None)),
        }
    }
    ContextMenu::for_id(menu_id)
        .size((Sizing::HUG, Sizing::HUG))
        .show(ui, |ui, popup| {
            // Resolved once for both the enable test and the value the pick
            // pushes — the fallback literal is built on read, not stored.
            let default = input.default();
            let can_set = !matches!(input.binding(), Some(Binding::Const(_))) && default.is_some();
            if MenuItem::new("Set constant")
                .disabled(!can_set)
                .show(ui, popup)
                .left
                .clicked()
                && let Some(value) = default
            {
                rec.out
                    .push_graph(GraphIntent::set_input(port, Binding::Const(value)));
            }
            if MenuItem::new("Clear binding")
                .disabled(input.binding().is_none())
                .show(ui, popup)
                .left
                .clicked()
            {
                rec.out.push_graph(GraphIntent::set_input(port, None));
            }
        });
}

/// Column 1: the inline const editor for an input bound to a `Const`. A
/// hug-sized column, so every editor starts at the same x.
fn value_cell(ui: &mut Ui, rec: &mut NodeRecord<'_, '_>, input: InputCtx<'_>) {
    let ncx = rec.node;
    // The one owner of the "only Const bindings get an inline editor"
    // filter — wired and unbound inputs render no value cell.
    let Some(Binding::Const(value)) = input.binding() else {
        return;
    };
    let port = input.port_ref();
    let data_type = input.ty();
    let value_variants = input.value_variants();
    let editor_id = const_editor_wid(input.port());
    // An `FsPath` editor draws a pick button; clicking it raises a blocking
    // file dialog, which only `App` can run — so this is the app tier.
    //
    // Every const editor shares one widget family, so "which editor was
    // clicked" would be all a canvas-level sweep could say; whether the click
    // means *pick a path* is a question about the port's type. Answering it
    // here costs nothing, because the type is what decided to draw the button
    // in the first place.
    if ui.response_for(editor_id).left.clicked()
        && matches!(value, ConstValue::FsPath(_) | ConstValue::FsPaths(_))
        && let DataType::FsPath(config) = data_type
    {
        rec.out
            .push_app(AppCommand::Edit(EditCommand::PickInputPath(PathPick {
                port: input.port(),
                config: Arc::clone(config),
            })));
    }
    // Fill the value column so every editor is the same width (the column
    // hugs to the widest editor's content). `min_size` on the editors keeps
    // a sensible floor; the editor fills this cell, this cell fills the col.
    // A knob another input sets aside is not what a run reads, so it does
    // not take edits until the override is gone.
    let edited = Panel::hstack()
        .id_salt(("val", port.port_idx))
        .grid_cell((grid_row(port.port_idx), COL_VALUE))
        .size((Sizing::FILL, Sizing::FILL))
        .disabled(input.overridden_by().is_some())
        .child_align(Align::v(VAlign::Center))
        .show(ui, |ui| {
            ConstEdit {
                field: Field {
                    theme: ncx.sve(),
                    id: editor_id,
                },
                library: ncx.graph_ctx.library(),
                value,
                data_type,
                value_variants,
            }
            .show(ui)
        });
    if let Some(new_value) = edited.inner {
        rec.out
            .push_graph(GraphIntent::set_input(port, Binding::Const(new_value)));
    }
}

/// Column 3: the output label + circle, right-aligned (the fill column
/// pins it to the node's right edge); the circle overhangs that edge. (A dragged
/// satellite can end up anywhere on the canvas, not just overhanging this
/// node).
fn output_cell(ui: &mut Ui, rec: &mut NodeRecord<'_, '_>, output: OutputCtx<'_>) {
    let (ncx, dcx) = (rec.node, rec.draw);
    let theme = ncx.theme();
    let port = output.port_ref();
    let ty = output.ty();
    let fill = Hover::from(dcx.geometry().ports.is_hovered(port)).lift(port_color::data(
        theme,
        ty,
        PortKind::Output,
    ));
    let tip = PortTip {
        description: output.description(),
        ty,
        set_aside_by: None,
    }
    .intern(ui, ncx);
    let wid = port_circle_wid(port);
    let overhang = theme.port_overhang();
    let cell = Panel::hstack()
        .id_salt(("out", port.port_idx))
        .grid_cell((grid_row(port.port_idx), COL_OUTPUT))
        .align(Align::new(HAlign::Right, VAlign::Center))
        .size((Sizing::HUG, Sizing::HUG))
        .sense(Sense::CLICK)
        .gap(ROW_GAP)
        .child_align(Align::v(VAlign::Center))
        .show(ui, |ui| {
            PortLabel {
                name: output.name(),
                tip,
                ink: LabelInk::Plain,
            }
            .show(ui, theme);
            PortGlyph::circle(wid, theme.ports.size, fill)
                .margin(Spacing::new(0.0, 0.0, -overhang, 0.0))
                .tip(tip)
                .show(ui)
        });
    // Right-click anywhere on the cell (circle or label) opens the port menu —
    // mirrors the input side's binding menu. Pulled into locals so the
    // response's `&Ui` borrow ends before the read below.
    let glyph = cell.inner;
    let (menu_id, cell_secondary) = (cell.response.id, cell.response.right.clicked());
    // An output may feed many inputs, so a double-click on its circle clears
    // each consumer. Same pre-paint timing as the input side: the intents drain
    // at the end of this pass, and the second pass a double-click earns
    // re-arranges every node that lost a wire.
    if glyph.double_clicked {
        rec.out.extend_graph(
            ncx.graph_ctx
                .connections()
                .filter(|(_, producer)| {
                    producer.node_id == port.node_id && producer.port_idx == port.port_idx
                })
                .map(|(consumer, _)| GraphIntent::set_input(consumer.into(), None)),
        );
    }
    if cell_secondary || glyph.secondary_clicked {
        open_port_context_menu(ui, menu_id);
    }
    // "Add preview" — spawn a preview node already wired to this output. What it
    // creates is an ordinary node the user can move, delete, and undo like any
    // other. Hidden when the library has no preview func.
    ContextMenu::for_id(menu_id)
        .size((Sizing::HUG, Sizing::HUG))
        .show(ui, |ui, popup| {
            if let Some(func) = preview_sink::registered(ncx.graph_ctx.library())
                && MenuItem::new("Add preview").show(ui, popup).left.clicked()
            {
                // Positioned off the port when its center is known (it is,
                // after the first frame); otherwise the node lands at the
                // origin and the user drags it.
                let pos = dcx
                    .geometry()
                    .ports
                    .center(port)
                    .map_or(Vec2::ZERO, |center| center + PREVIEW_SPAWN_OFFSET);
                rec.out.extend_graph(
                    PreviewSpawn {
                        port,
                        pos,
                        node_id: NodeId::unique(),
                    }
                    .intents(func),
                );
            }
        });
}

/// Where a preview lands relative to the port it was added from: clear of the
/// node body and a little above, so it doesn't cover what it is watching.
const PREVIEW_SPAWN_OFFSET: Vec2 = Vec2::new(80.0, -60.0);

/// A preview node `node_id` to spawn at `pos`, already reading `port`.
#[derive(Debug, Clone, Copy)]
pub(crate) struct PreviewSpawn {
    pub(crate) port: PortRef,
    pub(crate) pos: Vec2,
    pub(crate) node_id: NodeId,
}

impl PreviewSpawn {
    /// The two intents that spawn the preview `func` declares. Emitted
    /// together so one undo removes node *and* wire.
    pub(crate) fn intents(self, func: &Func) -> [GraphIntent; 2] {
        let Self { port, pos, node_id } = self;
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
}

/// One event (emitter) port row: the event name plus an event-colored triangle
/// glyph, right-aligned and overhanging the node edge like a data output. Sits in
/// `COL_OUTPUT` at `row` (below the data outputs). The glyph senses drags so a
/// wire can be pulled from it to a subscriber pin (see `SubscriptionUI`).
#[derive(Debug, Clone, Copy)]
struct EventRow<'a> {
    /// The event's index among the node's events.
    index: usize,
    /// The grid row it sits in.
    row: usize,
    event: &'a FuncEvent,
}

impl EventRow<'_> {
    fn show(self, ui: &mut Ui, rec: &NodeRecord<'_, '_>) {
        let Self { index, row, event } = self;
        let (ncx, dcx) = (rec.node, rec.draw);
        let theme = ncx.theme();
        let node_id = ncx.id;
        let overhang = theme.port_overhang();
        let ev = EventRef {
            node_id,
            event_idx: index,
        };
        let wid = event_glyph_wid(ev);
        let fill = Hover::from(dcx.geometry().events.is_hovered(ev)).lift(port_color::event(theme));
        let tip = ncx.tips().then(|| fmt!(ui, "event: {}", event.name));
        Panel::hstack()
            .id_salt(("event", index))
            .grid_cell((grid_row(row), COL_OUTPUT))
            .align(Align::new(HAlign::Right, VAlign::Center))
            .size((Sizing::HUG, Sizing::HUG))
            .gap(ROW_GAP)
            .child_align(Align::v(VAlign::Center))
            .show(ui, |ui| {
                // Muted like the data-port labels.
                Text::new(event.name.as_str())
                    .style(&label_style(ui, theme, LabelInk::Plain))
                    .show(ui);
                PortGlyph::arrow(wid, theme.ports.size, fill)
                    .margin(Spacing::new(0.0, 0.0, -overhang, 0.0))
                    .tip(tip)
                    .show(ui);
            });
    }
}

/// An event port glyph. A separate id space from data ports
/// ([`port_circle_wid`]) because events are indexed independently of outputs.
pub(crate) fn event_glyph_wid(event: EventRef) -> WidgetId {
    wid::event("event_glyph", event)
}

/// Human-readable type for a port tooltip: the picker mode (and accepted
/// extensions) for a path, and the library's own display name for everything
/// else — `any` for the untyped boundary placeholder, a registered
/// `Custom`/`Enum` type's name, or the raw id when nothing is registered
/// under it.
///
/// Renders on demand rather than into a `String`: a hovered node builds one
/// of these per port per frame, and [`PortTip::intern`] feeds it straight to `fmt!`.
#[derive(Debug)]
struct TypeLabel<'a> {
    library: &'a Library,
    ty: &'a DataType,
}

impl Display for TypeLabel<'_> {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        let DataType::FsPath(cfg) = self.ty else {
            // Borrowed for every type the library knows; only an id with no
            // entry behind it builds a string, and that one is scenarium's.
            return f.write_str(&self.library.type_name(self.ty));
        };
        let mode = match cfg.mode {
            FsPathMode::Directory => "directory",
            FsPathMode::ExistingFile => "file",
            FsPathMode::ExistingFiles => "files",
            FsPathMode::NewFile => "save path",
        };
        write!(f, "path · {mode}")?;
        // Written through one at a time rather than `join`ed — a joined list
        // is exactly the intermediate string this type exists to avoid.
        if let Some((first, rest)) = cfg.extensions.split_first() {
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
mod tests {
    use super::*;
    use scenarium::{NodeKind, OutputPort};

    /// The type line of a port tooltip. A path spells out its picker mode and
    /// the extensions it accepts; everything else defers to the library's own
    /// name for the type, so a tooltip and a palette entry can't disagree.
    #[test]
    fn type_label_spells_out_a_paths_mode_and_extension_list() {
        let library = Library::default();
        let label = |ty: &DataType| {
            TypeLabel {
                library: &library,
                ty,
            }
            .to_string()
        };
        let path = |mode, exts: &[&str]| {
            DataType::FsPath(Arc::new(FsPathConfig::with_extensions(
                mode,
                exts.iter().map(|e| (*e).to_owned()).collect(),
            )))
        };

        // Scalars and the untyped boundary placeholder are the library's names.
        assert_eq!(label(&DataType::Any), "any");
        assert_eq!(label(&DataType::Float), "float");

        // Every mode names itself, and an empty list adds no parenthetical.
        assert_eq!(label(&path(FsPathMode::Directory, &[])), "path · directory");
        assert_eq!(label(&path(FsPathMode::ExistingFile, &[])), "path · file");
        assert_eq!(label(&path(FsPathMode::ExistingFiles, &[])), "path · files");
        assert_eq!(label(&path(FsPathMode::NewFile, &[])), "path · save path");

        // One extension, then several: the separator appears only *between* them.
        assert_eq!(
            label(&path(FsPathMode::ExistingFile, &["fit"])),
            "path · file (fit)"
        );
        assert_eq!(
            label(&path(FsPathMode::ExistingFiles, &["fit", "fits", "raf"])),
            "path · files (fit, fits, raf)"
        );
    }

    /// "Add preview" spawns a node already reading the port it was raised
    /// from, offset clear of it, as one batch — so a single undo removes both
    /// the node and its wire.
    #[test]
    fn add_preview_spawns_a_node_already_wired_to_the_port() {
        let func = preview_sink::func(Arc::default());
        let producer = NodeId::unique();
        let port = PortRef::output(producer, 2);
        let center = Vec2::new(100.0, 40.0);

        let node_id = NodeId::unique();
        let [add, bind] = PreviewSpawn {
            port,
            pos: center + PREVIEW_SPAWN_OFFSET,
            node_id,
        }
        .intents(&func);

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
