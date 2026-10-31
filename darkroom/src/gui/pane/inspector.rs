//! The inspector pane: a read-only summary of one node — its identity, run status, inputs,
//! outputs and log — at UI scale, in a dock tab
//! ([`TabRef::Inspector`](crate::core::document::tab_ref::TabRef::Inspector)).
//!
//! The `Selection` tab follows the selection and the node header's inspect button opens it; its
//! pin opens a tab held on that one node, which closes with the node. Input lines show the
//! static binding; outputs show only their label. Neither reads a runtime value — a preview node
//! is how you look at one.

use std::borrow::Cow;
use std::collections::BTreeSet;

use palantir::{
    Align, Block, Chrome, ClipMode, Configure, Corners, DockOperation, Panel, RgbaF32, Scroll,
    Sense, Sizing, Spacing, Stroke, Text, TextInput, TextStyle, TextWrap, Ui, VAlign, WidgetId,
    fmt,
};
use scenarium::LogLevel;
use scenarium::NodeId;
use scenarium::{Binding, ConstValue, DataType, ValueVariant};

use crate::core::document::inspect_target::InspectTarget;
use crate::core::document::item_id::ItemId;
use crate::core::document::tab_ref::TabRef;
use crate::gui::frame::requests::Requests;
use crate::gui::graph_ctx::GraphCtx;
use crate::gui::graph_ctx::input_ctx::InputCtx;
use crate::gui::graph_ctx::node_ctx::NodeCtx;
use crate::gui::pane::graph_ui::node_ui::header;
use crate::gui::pane::graph_ui::node_ui::run_status::RunStatus;
use crate::gui::port_ref::PortKind;
use crate::gui::state::run_state::ExecStatus;
use crate::gui::theme::Theme;
use crate::gui::theme::color;
use crate::gui::widgets::format;
use crate::gui::widgets::format::shown_value::{ShownValue, UNSET};
use crate::gui::widgets::icon::Icon;
use crate::gui::widgets::icon_button::IconButton;
use crate::gui::widgets::support;

/// Most recent log lines shown per node, so the pane stays bounded.
const LOG_LINE_CAP: usize = 20;

/// Record the inspector tab showing `target`. Its pin raises the tab held on the node.
pub(crate) fn show(ui: &mut Ui, ctx: GraphCtx<'_>, target: InspectTarget, out: &mut Requests) {
    let theme = ctx.theme();
    Scroll::vertical()
        .id_salt(("inspector", target))
        .size(Sizing::FILL)
        .show(ui, |ui| {
            Panel::vstack()
                .id_salt("inspector_body")
                .size((Sizing::FILL, Sizing::HUG))
                .padding(theme.pane_padding())
                .gap(theme.space.md)
                .show(ui, |ui| {
                    if let Some(node) = inspected(ctx, target) {
                        body(ui, node, target, out);
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
        InspectTarget::Node(node_id) => node_id,
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
fn body(ui: &mut Ui, node: NodeCtx<'_>, target: InspectTarget, out: &mut Requests) {
    let theme = node.theme();
    let logs = node.graph_ctx.run_state().logs(node.node_id);
    let error = error_message(node.exec_status());
    Panel::vstack()
        .id_salt("inspector_header")
        .size((Sizing::FILL, Sizing::HUG))
        .gap(theme.space.xxs)
        .show(ui, |ui| {
            // One row as tall as a port row, its chip and pin centered on the title, the kind
            // mark after the title as on the node.
            Panel::hstack()
                .id(title_wid())
                .size((Sizing::FILL, Sizing::fixed(theme.row_height())))
                .gap(theme.space.xs)
                .child_align(Align::v(VAlign::Center))
                .show(ui, |ui| {
                    Text::new(format::node_name(node.name()))
                        .id(title_wid().with("text"))
                        .style(&support::title_text(ui, theme))
                        .text_wrap(TextWrap::Ellipsis)
                        .show(ui);
                    header::kind_mark(ui, node, kind_mark_wid());
                    support::hspacer(ui, "inspector_title_spacer");
                    status_chip(ui, theme, node.exec_status());
                    if target == InspectTarget::Selection
                        && !node.stub()
                        && IconButton::press(
                            Icon::Pin,
                            pin_wid(),
                            theme.colors.accent,
                            "Pin — keep this node in its own tab",
                        )
                        .show(ui, theme)
                    {
                        out.push_view(DockOperation::OpenTab {
                            tab: TabRef::Inspector(InspectTarget::Node(node.node_id)),
                        });
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

    let library = node.graph_ctx.library();
    let inputs = node.inputs();
    if inputs.len() > 0 {
        card(ui, theme, "Inputs", inputs.len(), |ui| {
            for (idx, input) in inputs.enumerate() {
                let value = PortValue::of(ui, input);
                InspectedPort {
                    wid: port_wid(PortKind::Input, idx),
                    name: input.name(),
                    dot: if input.const_only() {
                        PortDot::BLANK
                    } else {
                        PortDot::of(theme, input.data_type())
                    },
                    type_name: library.type_name(input.data_type()),
                    value: Some(value),
                }
                .show(ui, theme);
            }
        });
    }
    let outputs = node.outputs();
    if outputs.len() > 0 {
        card(ui, theme, "Outputs", outputs.len(), |ui| {
            for (idx, output) in outputs.enumerate() {
                InspectedPort {
                    wid: port_wid(PortKind::Output, idx),
                    name: output.name(),
                    dot: PortDot::of(theme, output.data_type()),
                    type_name: library.type_name(output.data_type()),
                    value: None,
                }
                .show(ui, theme);
            }
        });
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
        .size((Sizing::FILL, Sizing::HUG))
        .padding(Spacing::xy(theme.space.md, theme.space.sm))
        .chrome(theme.card_chrome())
        .show(ui, |ui| {
            Panel::hstack()
                .id(wid.with("caption"))
                .size((Sizing::FILL, Sizing::HUG))
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
        .size((Sizing::HUG, Sizing::HUG))
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
        .size((Sizing::FILL, Sizing::HUG))
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
                .chrome(Chrome::fill(error))
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

    fn of(theme: &Theme, data_type: &DataType) -> Self {
        Self {
            color: theme.port_color(data_type),
            fill: if *data_type == DataType::Any {
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
                .with_border(Stroke::new(self.color, theme.stroke.line)),
            DotFill::Blank => Chrome::NONE,
        };
        Block::new().id(wid).size(side).chrome(chrome).show(ui);
    }
}

/// One port row on one line: the port's dot, the name in the muted ink, its type the name's
/// tooltip, and at the right the input's value in the node's field box, cut in the middle when it
/// overruns so a path keeps its file's end, or the output's type name.
#[derive(Debug)]
struct InspectedPort<'a> {
    wid: WidgetId,
    name: &'a str,
    dot: PortDot,
    type_name: Cow<'a, str>,
    value: Option<PortValue<'a>>,
}

impl InspectedPort<'_> {
    fn show(self, ui: &mut Ui, theme: &Theme) {
        let Self {
            wid,
            name,
            dot,
            type_name,
            value,
        } = self;
        Panel::hstack()
            .id(wid)
            .size((Sizing::FILL, Sizing::fixed(theme.row_height())))
            .gap(theme.space.sm)
            .child_align(Align::v(VAlign::Center))
            .show(ui, |ui| {
                dot.show(ui, theme, wid.with("dot"));
                let name = Text::new(name)
                    .style(&TextStyle {
                        color: theme.port.label,
                        ..support::body_text(ui, theme)
                    })
                    .text_wrap(TextWrap::Ellipsis)
                    .size((Sizing::FILL, Sizing::HUG))
                    .sense(Sense::HOVER)
                    .show(ui)
                    .snapshot();
                let shown_type = fmt!(ui, "{type_name}");
                support::tooltip_after(ui, &name, Some(shown_type));
                match value {
                    Some(value) => value.show(ui, theme, wid.with("value")),
                    None => {
                        Text::new(shown_type)
                            .id(wid.with("value"))
                            .style(&support::muted_text(ui, theme, theme.font.caption))
                            .text_wrap(TextWrap::EllipsisMiddle)
                            .show(ui);
                    }
                }
            });
    }
}

/// What an input's field box reads, and the face it reads in.
#[derive(Debug)]
struct PortValue<'a> {
    text: TextInput<'a>,
    face: ValueFace,
}

/// The face of a field box's text, as the node's own field shows it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ValueFace {
    /// A word, a path or a count of files, in the body face.
    Text,
    /// A number, in the mono face, so figures line up from row to row.
    Number,
    /// No value, as "—" in the muted ink.
    Unset,
}

impl<'a> PortValue<'a> {
    /// The value tier of an input row, as the node's own field shows it: a constant one of the
    /// input's named variants holds reads by that variant's label, as the node's dropdown does,
    /// and any other through [`ShownValue`], the node's formatter.
    ///
    /// The fixed answers stay borrowed and the formatted one goes straight into the text arena,
    /// so an open pane builds no owned string per input per frame.
    fn of(ui: &mut Ui, input: InputCtx<'a>) -> Self {
        let value = match input.binding() {
            None => {
                return Self {
                    text: TextInput::Borrowed(UNSET),
                    face: ValueFace::Unset,
                };
            }
            Some(Binding::Wire(_)) => {
                return Self {
                    text: TextInput::Borrowed("wired"),
                    face: ValueFace::Text,
                };
            }
            Some(Binding::Const(value)) => value,
        };
        if let Some(label) = variant_label(input.value_variants(), value) {
            return Self {
                text: TextInput::Borrowed(label),
                face: ValueFace::Text,
            };
        }
        let shown = ShownValue {
            value,
            data_type: input.data_type(),
        };
        Self {
            text: TextInput::Interned(fmt!(ui, "{shown}")),
            face: if matches!(value, ConstValue::Int(_) | ConstValue::Float(_)) {
                ValueFace::Number
            } else {
                ValueFace::Text
            },
        }
    }

    /// The value in the node's resting field box, the row's height, as the node's field fills
    /// its row.
    fn show(self, ui: &mut Ui, theme: &Theme, wid: WidgetId) {
        let style = match self.face {
            ValueFace::Text => support::body_text(ui, theme),
            ValueFace::Number => support::mono_text(ui, theme.font.body),
            ValueFace::Unset => {
                support::colored_text(ui, theme.const_editor.rest.unset, theme.font.body)
            }
        };
        let field = &theme.const_editor.rest;
        Panel::hstack()
            .id(wid)
            .size((Sizing::HUG, Sizing::FILL))
            .padding(field.rest_padding())
            .child_align(Align::v(VAlign::Center))
            .chrome(field.rest_box().clone())
            .show(ui, |ui| {
                Text::new(self.text)
                    .id(wid.with("text"))
                    .style(&style)
                    .text_wrap(TextWrap::EllipsisMiddle)
                    .show(ui);
            });
    }
}

/// The label of the variant among `variants` that holds `value`, if one does.
fn variant_label<'a>(variants: &'a [ValueVariant], value: &ConstValue) -> Option<&'a str> {
    variants
        .iter()
        .find(|variant| &variant.value == value)
        .map(ValueVariant::label)
}

#[cfg(test)]
mod tests {
    use super::*;

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
        let node_id = h.session.open.document.graph.iter().next().unwrap().node_id;
        let _relayout = h.apply(GraphIntent::SetSelection {
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
        let theme = Theme::default();
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

    /// The title row is a port row's height, its chip and pin centered on the title; an impure
    /// node's kind mark follows the title, at the node header's gap, and a pure node shows none.
    #[test]
    fn the_title_row_centers_its_chip_and_pin_and_marks_the_kind() {
        use crate::core::document::internals::DocumentFixture;
        use crate::core::edit::graph_intent::GraphIntent;
        use crate::gui::app::session::internals::SessionHarness;
        use scenarium::internals::stub_func;
        use scenarium::{FuncId, FuncInput};

        let inspect = |fixture: DocumentFixture| {
            let mut h =
                SessionHarness::new(fixture.with_tab(TabRef::Inspector(InspectTarget::Selection)));
            let node_id = h.session.open.document.graph.iter().next().unwrap().node_id;
            let _relayout = h.apply(GraphIntent::SetSelection {
                to: [ItemId::Node(node_id)].into(),
            });
            h.prime(3);
            h
        };
        let theme = Theme::default();
        let impure = stub_func(FuncId::unique(), "probe")
            .input(FuncInput::optional("a", DataType::Int))
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

    /// A port's dot wears its type's hue, and an input that takes any type is a ring in the
    /// untyped port's ink.
    #[test]
    fn a_port_dot_wears_its_type_hue() {
        let theme = Theme::default();
        let float = PortDot::of(&theme, &DataType::Float);
        assert_eq!(
            (float.color, float.fill),
            (theme.type_colors.float, DotFill::Disc)
        );
        let any = PortDot::of(&theme, &DataType::Any);
        assert_eq!((any.color, any.fill), (theme.port.input, DotFill::Ring));
        assert_ne!(float.color, any.color);
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

    /// A constant one of the input's variants holds reads by that variant's label — its display
    /// label when it has one, else its name — and any other constant has none.
    #[test]
    fn a_variant_constant_reads_by_its_label() {
        let variants = [
            ValueVariant::new("precise_ground", ConstValue::Int(1)).display("Precise Ground"),
            ValueVariant::new("wide_field", ConstValue::Int(2)),
        ];
        assert_eq!(
            variant_label(&variants, &ConstValue::Int(1)),
            Some("Precise Ground")
        );
        assert_eq!(
            variant_label(&variants, &ConstValue::Int(2)),
            Some("wide_field")
        );
        assert_eq!(variant_label(&variants, &ConstValue::Int(3)), None);
    }
}
