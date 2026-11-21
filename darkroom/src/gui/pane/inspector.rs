//! The inspector pane: one node — its identity, run status, inputs, outputs and log — at UI
//! scale, in a dock tab ([`crate::core::document::tab_ref::TabRef::Inspector`]).
//!
//! The `Selection` tab follows the selection and the node header's inspect button opens it; its
//! pin opens a tab held on that one node, which closes with the node. Outputs show only their
//! label. Nothing here reads a runtime value — a preview node is how you look at one.
//!
//! - **The inspector edits.** An input's row records the node's own field, through
//!   [`input_field`], so a value is set at the interface's size at any zoom; a wired input names
//!   its producer. As Blender's Sidebar holds the properties of the selected node
//!   ([nodes](https://docs.blender.org/manual/en/latest/interface/controls/nodes/introduction.html)),
//!   and Houdini's parameter pane and Nuke's Properties do. Refused: a summary beside the node's
//!   fields, which left a 16 px field as the one place a value changed; a second set of
//!   editors, which would drift from the node's.

use std::collections::BTreeSet;

use palantir::{
    Block, Border, Chrome, ClipMode, Configure, Corners, Panel, RgbaF32, Scroll, Sense, SizeSpec,
    Sizing, Spacing, Text, TextInput, TextStyle, TextWrap, Ui, VAlign, WidgetId, fmt,
};
use scenarium::LogLevel;
use scenarium::NodeId;
use scenarium::{DataType, GraphId};

use crate::core::document::inspect_target::InspectTarget;
use crate::core::document::item_id::ItemId;
use crate::core::document::tab_ref::TabRef;
use crate::gui::frame::requests::Requests;
use crate::gui::frame::stage::Stage;
use crate::gui::graph_ctx::GraphCtx;
use crate::gui::graph_ctx::input_ctx::InputCtx;
use crate::gui::graph_ctx::node_ctx::NodeCtx;
use crate::gui::pane::graph_ui::node_ui::const_editor::Field;
use crate::gui::pane::graph_ui::node_ui::header;
use crate::gui::pane::graph_ui::node_ui::input_field::{self, InputField};
use crate::gui::pane::graph_ui::node_ui::run_status::RunStatus;
use crate::gui::pane::interface_card::{self, InterfaceCard};
use crate::gui::port_ref::PortKind;
use crate::gui::state::run_state::ExecStatus;
use crate::gui::theme::Theme;
use crate::gui::theme::color;
use crate::gui::widgets::format;
use crate::gui::widgets::format::shown_value::UNSET;
use crate::gui::widgets::icon::Icon;
use crate::gui::widgets::icon_button::IconButton;
use crate::gui::widgets::support;

/// Most recent log lines shown per node, so the pane stays bounded.
const LOG_LINE_CAP: usize = 20;

/// Record the inspector tab showing `target`. Its pin raises the tab held on the node.
pub(crate) fn show(ui: &mut Ui, stage: &mut Stage<'_, GraphCtx<'_>>, target: InspectTarget) {
    let ctx = stage.ctx;
    let theme = ctx.theme();
    Scroll::vertical()
        .id_salt(("inspector", target))
        .size(SizeSpec::FILL)
        .show(ui, |ui| {
            Panel::vstack()
                .id_salt("inspector_body")
                .size(SizeSpec::FILL_W)
                .padding(theme.pane_padding())
                .gap(theme.space.md)
                .show(ui, |ui| {
                    if let Some(node) = inspected(ctx, target) {
                        body(ui, &mut stage.with(node), target);
                    } else {
                        let style = support::muted_text(ui, theme, theme.font.body);
                        Text::new("Select one node to inspect it.")
                            .style(&style)
                            .show(ui);
                    }
                });
        });
}

/// The node `target` shows: the pinned one, or the selection's only node.
fn inspected(ctx: GraphCtx<'_>, target: InspectTarget) -> Option<NodeCtx<'_>> {
    let node_id = match target {
        InspectTarget::Node { addr, .. } => addr.node_id,
        InspectTarget::Selection => only_node(ctx.selected())?,
    };
    ctx.node(node_id)
}

/// The one node `selected` holds, ties aside, or `None` for none or several.
fn only_node(selected: &BTreeSet<ItemId>) -> Option<NodeId> {
    let mut nodes = selected.iter().filter_map(|item| match *item {
        ItemId::Node(node_id) => Some(node_id),
        ItemId::Tie(_) => None,
    });
    let node_id = nodes.next()?;
    nodes.next().is_none().then_some(node_id)
}

/// The node's summary: its name and kind, status, description, ports and log.
fn body(ui: &mut Ui, stage: &mut Stage<'_, NodeCtx<'_>>, target: InspectTarget) {
    let node = stage.ctx;
    let theme = node.theme();
    let logs = node
        .program_id()
        .map_or(&[][..], |id| node.graph_ctx.run_state().logs(id));
    let error = error_message(node.exec_status());
    Panel::vstack()
        .id_salt("inspector_header")
        .size(SizeSpec::FILL_W)
        .gap(theme.space.xxs)
        .show(ui, |ui| {
            // One row as tall as a port row, its chip and pin centered on the title, the kind
            // mark after the title as on the node.
            Panel::hstack()
                .id(title_wid())
                .size((Sizing::FILL, Sizing::fixed(theme.row_height())))
                .gap(theme.space.xs)
                .child_align(VAlign::Center)
                .show(ui, |ui| {
                    Text::new(format::node_name(node.name()))
                        .id(title_wid().with("text"))
                        .style(&support::title_text(ui, theme))
                        .text_wrap(TextWrap::Ellipsis)
                        .show(ui);
                    header::kind_mark(ui, node, kind_mark_wid());
                    header::compound_mark(ui, node, compound_mark_wid(), stage.out);
                    support::hspacer(ui, "inspector_title_spacer");
                    status_chip(ui, theme, node.exec_status());
                    if target == InspectTarget::Selection
                        && !node.ghost()
                        && IconButton::press(
                            Icon::Pin,
                            pin_wid(),
                            theme.colors.accent,
                            "Pin — keep this node in its own tab",
                        )
                        .show(ui, theme)
                    {
                        stage.out.push_show(TabRef::Inspector(InspectTarget::Node {
                            addr: node.addr(),
                            through: node.graph_ctx.through(),
                        }));
                    }
                });
            // The kind line earns its row only when it says something the title doesn't: an
            // unrenamed node repeats its func name.
            if !node.kind_label().eq_ignore_ascii_case(node.name()) {
                line(
                    ui,
                    node.kind_label(),
                    support::muted_text(ui, theme, theme.font.caption),
                );
            }
            // What a ghost node was, where a save kept it: its func's name and category, to find
            // the build or the plugin that has it.
            if let Some(saved) = node.saved() {
                let was = if saved.category.is_empty() {
                    fmt!(ui, "was {}", saved.name)
                } else {
                    fmt!(ui, "was {} \u{b7} {}", saved.name, saved.category)
                };
                line(ui, was, support::muted_text(ui, theme, theme.font.caption));
            }
        });
    // The failure's cause under the header: what turns a generic status into an actionable
    // message (e.g. "no light frames provided").
    if let Some(message) = error {
        error_callout(ui, theme, message);
    }
    if !node.description().is_empty() {
        line(
            ui,
            node.description(),
            TextStyle {
                color: theme.port.label,
                ..support::body_text(ui, theme)
            },
        );
    }

    let (library, graphs) = (node.graph_ctx.library(), &node.graph_ctx.document().graphs);
    let inputs = node.inputs();
    if inputs.len() > 0 {
        let out = &mut *stage.out;
        card(ui, theme, "Inputs", inputs.len(), |ui| {
            for (idx, input) in inputs.enumerate() {
                let wid = port_wid(PortKind::Input, idx);
                InspectedPort {
                    wid,
                    name: input.name(),
                    dot: if input.const_only() {
                        PortDot::BLANK
                    } else {
                        PortDot::of(node.graph_ctx, input.data_type())
                    },
                    type_name: graphs.type_name(library, input.data_type()),
                }
                .show(ui, theme, |ui| {
                    input_value(ui, out, input, wid.with("value"));
                });
            }
        });
    }
    let outputs = node.outputs();
    if outputs.len() > 0 {
        card(ui, theme, "Outputs", outputs.len(), |ui| {
            for (idx, output) in outputs.enumerate() {
                let wid = port_wid(PortKind::Output, idx);
                let type_name = graphs.type_name(library, output.data_type());
                InspectedPort {
                    wid,
                    name: output.name(),
                    dot: PortDot::of(node.graph_ctx, output.data_type()),
                    type_name,
                }
                .show(ui, theme, |ui| {
                    Text::new(type_name)
                        .id(wid.with("value"))
                        .style(&support::muted_text(ui, theme, theme.font.caption))
                        .text_wrap(TextWrap::EllipsisMiddle)
                        .show(ui);
                });
            }
        });
    }
    if let Some(compound) = node.shows_interface() {
        let document = node.graph_ctx.document();
        let card = InterfaceCard {
            theme,
            library,
            graphs: &document.graphs,
            compound,
            shared: document.sides[&compound].shared,
            read_only: document.read_only(GraphId::Compound(compound)),
        };
        interface_card::show(ui, card, stage.out);
    }
    // This node's lines from the last run, level-colored, capped to the most recent few, in a
    // card of their own so they read as the run's record, not as the node's fields.
    if !logs.is_empty() {
        let shown = &logs[logs.len().saturating_sub(LOG_LINE_CAP)..];
        card(ui, theme, "Log", logs.len(), |ui| {
            for entry in shown {
                line(
                    ui,
                    &entry.message,
                    TextStyle {
                        color: log_color(theme, entry.level),
                        ..support::figure_text(ui, theme)
                    },
                );
            }
        });
    }
}

/// One group of the pane on the node's own fill, radius and shadow, so its rows read as the
/// node's: a caption with the count of what it holds at the right, then the rows. The log's count
/// is every line, so a card that shows the last [`LOG_LINE_CAP`] says how many it left out.
fn card(
    ui: &mut Ui,
    theme: &Theme,
    caption: &'static str,
    count: usize,
    rows: impl FnOnce(&mut Ui),
) {
    let wid = card_wid(caption);
    Panel::vstack()
        .id(wid)
        .size(SizeSpec::FILL_W)
        .padding(Spacing::xy(theme.space.md, theme.space.sm))
        .chrome(theme.card_chrome())
        .show(ui, |ui| {
            Panel::hstack()
                .id(wid.with("caption"))
                .size(SizeSpec::FILL_W)
                .padding(Spacing::new(0.0, 0.0, 0.0, theme.space.xs))
                .show(ui, |ui| {
                    Text::new(caption)
                        .style(&support::caption_text(ui, theme))
                        .show(ui);
                    support::hspacer(ui, "inspector_caption_spacer");
                    let style = TextStyle {
                        color: theme.port.label,
                        ..support::figure_text(ui, theme)
                    };
                    Text::new(fmt!(ui, "{count}"))
                        .id(wid.with("count"))
                        .style(&style)
                        .show(ui);
                });
            rows(ui);
        });
}

/// The row of the `kind` port at `idx`.
fn port_wid(kind: PortKind, idx: usize) -> WidgetId {
    WidgetId::from_hash(("darkroom.inspector.port", kind, idx))
}

/// The card captioned `caption`.
fn card_wid(caption: &'static str) -> WidgetId {
    WidgetId::from_hash(("darkroom.inspector.card", caption))
}

fn title_wid() -> WidgetId {
    WidgetId::from_hash("darkroom.inspector.title")
}

fn kind_mark_wid() -> WidgetId {
    WidgetId::from_hash("darkroom.inspector.kind_mark")
}

fn compound_mark_wid() -> WidgetId {
    WidgetId::from_hash("darkroom.inspector.compound_mark")
}

fn pin_wid() -> WidgetId {
    WidgetId::from_hash("darkroom.inspector.pin")
}

fn status_wid() -> WidgetId {
    WidgetId::from_hash("darkroom.inspector.status")
}

fn error_wid() -> WidgetId {
    WidgetId::from_hash("darkroom.inspector.error")
}

/// The failure's cause the callout shows: a failed node's message, when one came.
fn error_message(status: &ExecStatus) -> Option<&str> {
    match status {
        ExecStatus::Errored(message) => message.as_deref(),
        _ => None,
    }
}

/// The last run's word on the node as a chip: the node footer's own [`RunStatus`] on a soft wash
/// of its ink.
fn status_chip(ui: &mut Ui, theme: &Theme, status: &ExecStatus) {
    Panel::hstack()
        .id(status_wid())
        .size(SizeSpec::HUG)
        .padding(Spacing::xy(theme.space.sm, theme.space.xxs))
        .chrome(Chrome::rounded(
            color::wash(theme.exec_ink(status), theme.tint.soft, theme.canvas.bg),
            Corners::all(theme.radius.control),
        ))
        .show(ui, |ui| RunStatus::new(status).show(ui, theme));
}

/// A failure's message in the body ink on a wash of the error ink, a bar of it at the left: the
/// cause reads as an alert, not as the loudest text in the window. It wraps anywhere, so a path
/// does not push the pane wide.
fn error_callout(ui: &mut Ui, theme: &Theme, message: &str) {
    let error = theme.status.error;
    let pad = theme.space.sm;
    Panel::hstack()
        .id(error_wid())
        .size(SizeSpec::FILL_W)
        .padding(Spacing::new(0.0, 0.0, pad, 0.0))
        .gap(pad)
        .chrome(Chrome::rounded(
            color::wash(error, theme.tint.faint, theme.canvas.bg),
            Corners::all(theme.radius.card),
        ))
        // The bar runs the callout's height, and its rounded clip curves the bar's ends with it.
        .clip(ClipMode::Rounded)
        .show(ui, |ui| {
            Block::new()
                .id_salt("inspector_error_bar")
                .size((Sizing::fixed(theme.stroke.line), Sizing::FILL))
                .chrome(Chrome::new(error))
                .show(ui);
            Text::new(message)
                .id(error_wid().with("message"))
                .style(&support::body_text(ui, theme))
                .text_wrap(TextWrap::Wrap)
                .margin(Spacing::new(0.0, pad, 0.0, pad))
                .show(ui);
        });
}

/// A log line's ink by level: info in the body ink, warn and error in their status inks.
const fn log_color(theme: &Theme, level: LogLevel) -> RgbaF32 {
    match level {
        LogLevel::Info => theme.ink.text,
        LogLevel::Warn => theme.status.warning,
        LogLevel::Error => theme.status.error,
    }
}

fn line<'a>(ui: &mut Ui, text: impl Into<TextInput<'a>>, style: TextStyle) {
    Text::new(text)
        .style(&style)
        .text_wrap(TextWrap::Wrap)
        .show(ui);
}

/// A port's dot as the node paints it: a disc in its type's hue, a ring for an input that takes
/// any type, or a blank for an input that takes only a constant, which the node draws no port
/// for; the blank keeps the names in one column.
#[derive(Clone, Copy, Debug)]
struct PortDot {
    color: RgbaF32,
    fill: DotFill,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum DotFill {
    Disc,
    Ring,
    Blank,
}

impl PortDot {
    const BLANK: Self = Self {
        color: RgbaF32::TRANSPARENT,
        fill: DotFill::Blank,
    };

    fn of(graph_ctx: GraphCtx<'_>, data_type: DataType) -> Self {
        Self {
            color: graph_ctx.port_color(data_type),
            fill: if data_type == DataType::Any {
                DotFill::Ring
            } else {
                DotFill::Disc
            },
        }
    }

    fn show(self, ui: &mut Ui, theme: &Theme, wid: WidgetId) {
        let side = theme.port.row_dot;
        let corners = Corners::all(side * 0.5);
        let chrome = match self.fill {
            DotFill::Disc => Chrome::rounded(self.color, corners),
            DotFill::Ring => Chrome::rounded(RgbaF32::TRANSPARENT, corners)
                .with_border(Border::new(self.color, theme.stroke.line)),
            DotFill::Blank => Chrome::NONE,
        };
        Block::new().id(wid).size(side).chrome(chrome).show(ui);
    }
}

/// One port row on one line: the port's dot, the name in the muted ink, its type the name's
/// tooltip, and at the right what the caller records there: an input's value, an output's type.
#[derive(Debug)]
struct InspectedPort<'a> {
    wid: WidgetId,
    name: &'a str,
    dot: PortDot,
    type_name: &'a str,
}

impl InspectedPort<'_> {
    fn show(self, ui: &mut Ui, theme: &Theme, value: impl FnOnce(&mut Ui)) {
        let Self {
            wid,
            name,
            dot,
            type_name,
        } = self;
        Panel::hstack()
            .id(wid)
            .size((Sizing::FILL, Sizing::fixed(theme.row_height())))
            .gap(theme.space.sm)
            .child_align(VAlign::Center)
            .show(ui, |ui| {
                dot.show(ui, theme, wid.with("dot"));
                let name = Text::new(name)
                    .style(&TextStyle {
                        color: theme.port.label,
                        ..support::body_text(ui, theme)
                    })
                    .text_wrap(TextWrap::Ellipsis)
                    .size(SizeSpec::FILL_W)
                    .sense(Sense::HOVER)
                    .show(ui);
                let shown_type = fmt!(ui, "{type_name}");
                support::tooltip_after(ui, &name, Some(shown_type));
                value(ui);
            });
    }
}

/// An input's value at the right of its row: a constant in the node's own field, which edits it,
/// the empty field where one could sit, the producer a wire reads from, and a kept ghost's value
/// dimmed. An input that takes no edit records its field disabled, its face kept.
fn input_value(ui: &mut Ui, out: &mut Requests, input: InputCtx<'_>, wid: WidgetId) {
    let theme = input.node().theme();
    let muted = support::muted_text(ui, theme, theme.font.body);
    let field = Field {
        theme: &theme.const_editor.rest,
        id: wid.with("editor"),
    };
    let value = match InputField::of(input) {
        InputField::Const(value) => Some(value),
        InputField::Empty => None,
        InputField::Wired(source) => {
            let graphs = &input.node().graph_ctx.document().graphs;
            let scope = graphs.scope(input.library(), input.graph());
            let producer = graphs
                .graph(input.graph())
                .find(source.node_id)
                .map_or("", |node| format::node_name(scope.title(node)));
            Text::new(fmt!(ui, "\u{2190} {producer}"))
                .id(wid)
                .style(&muted)
                .text_wrap(TextWrap::Ellipsis)
                .show(ui);
            return;
        }
        InputField::KeptConst(value) => {
            Text::new(fmt!(ui, "kept {}", value.value_text()))
                .id(wid)
                .style(&muted)
                .text_wrap(TextWrap::Ellipsis)
                .show(ui);
            return;
        }
        InputField::Unwired | InputField::KeptWire => {
            Text::new(UNSET).id(wid).style(&muted).show(ui);
            return;
        }
    };
    Panel::hstack()
        .id(wid)
        .size((Sizing::HUG, Sizing::FILL))
        .disabled(input.read_only())
        .child_align(VAlign::Center)
        .show(ui, |ui| input_field::record(ui, out, input, value, field));
}

#[cfg(test)]
mod tests {
    use super::*;
    use palantir::ColorScheme;

    /// A port row is 28 px high, its value boxed at the row's right edge, which is the card's
    /// right padding: a probe node's input, read in the inspector tab over the selected node.
    #[test]
    fn a_port_row_is_one_line_with_its_value_at_the_right() {
        use crate::core::document::internals::DocumentFixture;
        use crate::core::edit::graph_intent::GraphIntent;
        use crate::gui::app::session::internals::SessionHarness;

        let fixture =
            DocumentFixture::probes(1).with_tab(TabRef::Inspector(InspectTarget::Selection));
        let mut h = SessionHarness::new(fixture);
        let node_id = h
            .session
            .open
            .document()
            .graphs
            .main()
            .iter()
            .next()
            .unwrap()
            .node_id;
        h.apply(GraphIntent::SetSelection {
            to: [ItemId::Node(node_id)].into(),
        });
        h.prime(3);
        let row =
            h.ui.rect(port_wid(PortKind::Input, 0))
                .expect("the input row recorded");
        let value =
            h.ui.rect(port_wid(PortKind::Input, 0).with("value"))
                .expect("the value recorded");
        let card =
            h.ui.rect(card_wid("Inputs"))
                .expect("the inputs card recorded");
        let theme = Theme::new(ColorScheme::Dark);
        assert_eq!(row.size.h, theme.row_height());
        assert_eq!(
            value.size.h,
            theme.row_height(),
            "the field fills the row, as on the node"
        );
        assert_eq!(value.max().x, row.max().x, "the value ends the row");
        assert_eq!(
            card.max().x - row.max().x,
            theme.space.md,
            "the row ends at the card's right padding"
        );
        let dot =
            h.ui.rect(port_wid(PortKind::Input, 0).with("dot"))
                .expect("the dot recorded");
        assert_eq!(
            (dot.size.w, dot.size.h, dot.min.x - row.min.x),
            (theme.port.row_dot, theme.port.row_dot, 0.0),
            "the dot opens the row"
        );
    }

    /// An input's field in the inspector edits its constant as the node's does: a typed value
    /// lands one `SetInput`, which the node's own field then shows. A wired input names its
    /// producer instead of a field.
    #[test]
    fn the_inspector_edits_a_constant_as_the_node_does() {
        use palantir::{Key, Modifiers};
        use scenarium::{Binding, ConstValue, InputPort, InputState, PortId};

        use crate::core::document::internals::DocumentFixture;
        use crate::core::edit::graph_intent::GraphIntent;
        use crate::gui::app::session::internals::SessionHarness;
        use crate::gui::pane::graph_ui::wid;

        // In the sidebar, so the graph shows beside it.
        let mut h = SessionHarness::new(DocumentFixture::probes(2));
        h.requests
            .push_show(TabRef::Inspector(InspectTarget::Selection));
        h.drain();
        let nodes: Vec<NodeId> = h
            .session
            .open
            .document()
            .graphs
            .main()
            .iter()
            .map(|node| node.node_id)
            .collect();
        let port = InputPort::new(nodes[1], PortId::new(1));
        h.apply(GraphIntent::set_input(
            port,
            Binding::Const(ConstValue::Int(3)),
        ));
        h.apply(GraphIntent::SetSelection {
            to: [ItemId::Node(nodes[1])].into(),
        });
        h.prime(3);
        let editor = port_wid(PortKind::Input, 0).with("value").with("editor");
        h.ui.click(h.ui.rect(editor).expect("the field records").center());
        h.frame();
        h.ui.hold_modifiers(Modifiers::CTRL);
        h.ui.key(Key::Char('a'));
        h.ui.hold_modifiers(Modifiers::NONE);
        h.ui.type_text("7");
        h.ui.key(Key::Enter);
        h.frame();
        h.drain();
        let state = h.session.open.document().graphs.main().input_state(port);
        assert_eq!(
            format!("{state:?}"),
            format!("{:?}", InputState::Live(Binding::Const(ConstValue::Int(7))))
        );
        h.prime(2);
        let on_node: Vec<String> =
            h.ui.widgets(None)
                .into_iter()
                .filter(|widget| widget.id == wid::const_editor(GraphId::Main, port))
                .map(|widget| widget.text)
                .collect();
        assert_eq!(
            on_node,
            ["7"],
            "the node's field shows the inspector's edit"
        );

        h.apply(GraphIntent::set_input(
            port,
            Binding::wire(nodes[0], PortId::new(1)),
        ));
        h.prime(2);
        let wired = h.ui.widgets(Some("\u{2190} probe"));
        assert!(!wired.is_empty(), "a wired input names its producer");
    }

    /// The title row is a port row's height, its chip and pin centered on the title; an impure
    /// node's kind mark follows the title, at the node header's gap, and a pure node shows none.
    #[test]
    fn the_title_row_centers_its_chip_and_pin_and_marks_the_kind() {
        use crate::core::document::internals::DocumentFixture;
        use crate::core::edit::graph_intent::GraphIntent;
        use crate::gui::app::session::internals::SessionHarness;
        use scenarium::internals::stub_func;
        use scenarium::{FuncId, InputDecl};

        let inspect = |fixture: DocumentFixture| {
            let mut h =
                SessionHarness::new(fixture.with_tab(TabRef::Inspector(InspectTarget::Selection)));
            let node_id = h
                .session
                .open
                .document()
                .graphs
                .main()
                .iter()
                .next()
                .unwrap()
                .node_id;
            h.apply(GraphIntent::SetSelection {
                to: [ItemId::Node(node_id)].into(),
            });
            h.prime(3);
            h
        };
        let theme = Theme::new(ColorScheme::Dark);
        let impure = stub_func(FuncId::unique(), "probe")
            .input(1, InputDecl::optional("a", DataType::Int))
            .build();
        let mut fixture = DocumentFixture::default();
        fixture.add(&impure);
        let h = inspect(fixture);
        let rect = |wid: WidgetId| h.ui.rect(wid).expect("recorded");
        let row = rect(title_wid());
        assert_eq!(row.size.h, theme.row_height());
        let title = rect(title_wid().with("text"));
        let mark = rect(kind_mark_wid());
        assert_eq!(
            mark.min.x,
            title.max().x + theme.space.xs,
            "the mark follows the title"
        );
        for (what, wid) in [
            ("title", title_wid().with("text")),
            ("chip", status_wid()),
            ("pin", pin_wid()),
            ("mark", kind_mark_wid()),
        ] {
            assert_eq!(
                rect(wid).center().y,
                row.center().y,
                "the {what} centers in the row"
            );
        }

        let pure = inspect(DocumentFixture::probes(1));
        assert_eq!(
            pure.ui.rect(kind_mark_wid()),
            None,
            "a pure node shows no mark"
        );
    }

    /// A port's dot wears its type's hue, an input that takes any type is a ring in the untyped
    /// port's ink, and a type the library does not register is a disc in that neutral ink.
    /// An instance's inspector wears its compound mark after the title, as the node does; a
    /// func's node wears none.
    #[test]
    fn an_instances_inspector_wears_its_compound_mark() {
        use scenarium::{Compound, CompoundId, Interface};

        use crate::core::document::internals::{DocumentFixture, insert_compound};
        use crate::core::edit::graph_intent::GraphIntent;
        use crate::gui::app::session::internals::SessionHarness;

        let mut fixture = DocumentFixture::probes(1);
        let probe = fixture.node(0);
        let id = CompoundId::from_u128(1);
        insert_compound(
            &mut fixture.document,
            id,
            Compound::new("inner", Interface::default()),
        );
        let instance = fixture.add_instance(id);
        let mut h =
            SessionHarness::new(fixture.with_tab(TabRef::Inspector(InspectTarget::Selection)));
        let shows_mark = |h: &mut SessionHarness, node| {
            h.apply(GraphIntent::SetSelection {
                to: [ItemId::Node(node)].into(),
            });
            h.prime(3);
            h.ui.rect(compound_mark_wid()).is_some()
        };
        assert!(shows_mark(&mut h, instance));
        assert!(!shows_mark(&mut h, probe));
    }

    #[test]
    fn a_port_dot_wears_its_type_hue() {
        use scenarium::CustomTypeId;

        use crate::gui::graph_ctx::internals::GraphCtxFixture;

        let fixture = GraphCtxFixture::default();
        let (ctx, theme) = (fixture.graph_ctx(), &fixture.theme);
        let float = PortDot::of(ctx, DataType::Float);
        assert_eq!(
            (float.color, float.fill),
            (theme.type_colors.float, DotFill::Disc)
        );
        let any = PortDot::of(ctx, DataType::Any);
        assert_eq!((any.color, any.fill), (theme.port.input, DotFill::Ring));
        assert_ne!(float.color, any.color);
        let unknown = DataType::Custom(CustomTypeId::from_u128(77));
        assert_ne!(
            theme.port_color(unknown),
            theme.port.input,
            "premise: it has a hue"
        );
        let unknown = PortDot::of(ctx, unknown);
        assert_eq!(
            (unknown.color, unknown.fill),
            (theme.port.input, DotFill::Disc)
        );
    }

    /// The callout shows a failure's message, and nothing for a failure without one or any
    /// other status; the chip's ink is `Theme::exec_ink`'s.
    #[test]
    fn the_callout_shows_a_failures_message_alone() {
        let failed = ExecStatus::Errored(Some("no light frames".to_owned()));
        let cases = [
            (ExecStatus::None, None),
            (ExecStatus::Cached, None),
            (ExecStatus::Executed(0.5), None),
            (ExecStatus::Running(std::time::Instant::now()), None),
            (ExecStatus::MissingInputs(Vec::new()), None),
            (ExecStatus::Errored(None), None),
            (failed, Some("no light frames")),
            (ExecStatus::Skipped, None),
        ];
        for (status, message) in &cases {
            assert_eq!(error_message(status), *message, "{status:?}");
        }
    }

    /// The selection's inspector shows its one node, ties aside; none or several nodes show the
    /// hint.
    #[test]
    fn the_selection_inspector_shows_the_one_selected_node() {
        use crate::core::document::tie::TieId;

        let [a, b] = [NodeId::unique(), NodeId::unique()];
        let tie = TieId::unique();
        let set = |items: &[ItemId]| items.iter().copied().collect::<BTreeSet<_>>();
        assert_eq!(only_node(&set(&[])), None);
        assert_eq!(only_node(&set(&[ItemId::Node(a)])), Some(a));
        assert_eq!(
            only_node(&set(&[ItemId::Node(a), ItemId::Tie(tie)])),
            Some(a)
        );
        assert_eq!(only_node(&set(&[ItemId::Node(a), ItemId::Node(b)])), None);
        assert_eq!(only_node(&set(&[ItemId::Tie(tie)])), None);
    }
}
