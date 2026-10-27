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

use palantir::prelude::*;
use palantir::{ClipMode, DockOperation, FontWeight, TextInput, TextWrap};
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
use crate::gui::port_ref::PortKind;
use crate::gui::state::run_state::ExecStatus;
use crate::gui::theme::Theme;
use crate::gui::widgets::format::elapsed::Elapsed;
use crate::gui::widgets::format::shown_value::{ShownValue, UNSET};
use crate::gui::widgets::icon::Icon;
use crate::gui::widgets::icon_button::IconButton;
use crate::gui::widgets::port_glyph;
use crate::gui::widgets::support;
use crate::gui::widgets::toggle::Toggle;

/// Most recent log lines shown per node, so the pane stays bounded.
const LOG_LINE_CAP: usize = 20;

/// Record the inspector tab showing `target`. Its pin raises the tab held on the node.
pub(crate) fn show(ui: &mut Ui, ctx: GraphCtx<'_>, target: InspectTarget, out: &mut Requests) {
    let theme = ctx.theme();
    Scroll::vertical()
        .id_salt(("inspector", target))
        .size((Sizing::FILL, Sizing::FILL))
        .show(ui, |ui| {
            Panel::vstack()
                .id_salt("inspector_body")
                .size((Sizing::FILL, Sizing::HUG))
                .padding(Spacing::xy(theme.space.lg, theme.space.md))
                .gap(theme.space.md)
                .show(ui, |ui| {
                    if let Some(node) = inspected(ctx, target) {
                        body(ui, node, target, out);
                    } else {
                        let style = support::muted_text(ui, theme, theme.text.body);
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
    Panel::hstack()
        .id_salt("inspector_title")
        .size((Sizing::FILL, Sizing::HUG))
        .gap(theme.space.sm)
        .child_align(Align::v(VAlign::Top))
        .show(ui, |ui| {
            Panel::vstack()
                .id_salt("inspector_names")
                .size((Sizing::FILL, Sizing::HUG))
                .gap(NAME_KIND_GAP)
                .show(ui, |ui| {
                    let title = if node.name().is_empty() {
                        "(unnamed)"
                    } else {
                        node.name()
                    };
                    line(ui, title, title_style(theme, ui));
                    // The kind line earns its row only when it says something the title doesn't:
                    // an unrenamed node repeats its func name.
                    if !node.kind_label().eq_ignore_ascii_case(node.name()) {
                        line(ui, node.kind_label(), muted_style(theme, ui));
                    }
                });
            status_chip(ui, theme, node.exec_status());
            if target == InspectTarget::Selection
                && !node.stub()
                && IconButton::press(
                    Icon::Pin,
                    pin_wid(),
                    theme.colors.badge_graph,
                    "Pin — keep this node in its own tab",
                    Toggle::Off,
                )
                .show(ui, theme)
            {
                out.push_view(DockOperation::OpenTab {
                    tab: TabRef::Inspector(InspectTarget::Node(node.node_id)),
                });
            }
        });
    // The failure's cause under the header: what turns a generic status into an actionable
    // message (e.g. "no light frames provided").
    if let Some(message) = error {
        error_callout(ui, theme, message);
    }
    if node.sink() {
        line(ui, "Sink", muted_style(theme, ui));
    }
    if !node.description().is_empty() {
        line(
            ui,
            node.description(),
            TextStyle {
                color: theme.ports.label,
                ..body_style(theme, ui)
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
                        color: log_color(theme, ui, entry.level),
                        ..support::mono_text(ui, theme.text.label)
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
        .chrome(
            Chrome::rounded(theme.card.fill, Corners::all(theme.radius.card))
                .with_shadow(theme.card.resting_shadow()),
        )
        .show(ui, |ui| {
            Panel::hstack()
                .id(wid.with("caption"))
                .size((Sizing::FILL, Sizing::HUG))
                .padding(Spacing::new(0.0, 0.0, 0.0, theme.space.xs))
                .show(ui, |ui| {
                    let style = TextStyle {
                        weight: FontWeight::SEMI_BOLD,
                        ..muted_style(theme, ui)
                    };
                    Text::new(caption).style(&style).show(ui);
                    support::hspacer(ui, "inspector_caption_spacer");
                    let style = TextStyle {
                        color: theme.ports.label,
                        ..support::mono_text(ui, theme.text.label)
                    };
                    Text::new(fmt!(ui, "{count}"))
                        .id(wid.with("count"))
                        .style(&style)
                        .show(ui);
                });
            rows(ui);
        });
}

/// A port row's height: a field box, a body line and 2 px above and below it, with a gutter
/// around it, so the rows of a card do not touch.
const PORT_ROW_HEIGHT: f32 = 28.0;

/// The side of a port's dot in a row: under the node's port, which is sized for a wire to land on
/// and a press to grab, and a row has neither.
const PORT_DOT_SIDE: f32 = 10.0;

/// The row of the `kind` port at `idx`.
fn port_wid(kind: PortKind, idx: usize) -> WidgetId {
    WidgetId::from_hash(("darkroom.inspector.port", kind, idx))
}

/// The card captioned `caption`.
fn card_wid(caption: &'static str) -> WidgetId {
    WidgetId::from_hash(("darkroom.inspector.card", caption))
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

/// The name and the kind under it sit closer than the pane's gap, so the kind reads as the
/// name's subtitle.
const NAME_KIND_GAP: f32 = 2.0;

/// The status chip's icon and word sit as a node footer's do.
const STATUS_GAP: f32 = 6.0;

/// The status chip's fill: a [`Theme::wash`] of its ink, light enough that the word reads on it.
const STATUS_FILL_ALPHA: f32 = 0.14;

/// The error callout's fill: a [`Theme::wash`] of the error ink, under the status chip's, as the
/// callout is the larger surface.
const ERROR_FILL_ALPHA: f32 = 0.10;

/// The error callout's bar at its left edge.
const ERROR_BAR_WIDTH: f32 = 2.0;

/// The failure's cause the callout shows: a failed node's message, when one came.
fn error_message(status: &ExecStatus) -> Option<&str> {
    match status {
        ExecStatus::Errored(message) => message.as_deref(),
        _ => None,
    }
}

/// The ink and icon of a status chip: the status ink, muted where a status has none, and the icon
/// the node's footer shows: a cross for a failure, a check for a run.
#[derive(Clone, Copy, Debug, PartialEq)]
struct StatusLook {
    color: RgbaF32,
    icon: Option<Icon>,
}

impl StatusLook {
    const fn of(theme: &Theme, status: &ExecStatus) -> Self {
        let color = match theme.exec_color(status) {
            Some(color) => color,
            None => theme.colors.text_muted,
        };
        let icon = match status {
            ExecStatus::Errored(_) => Some(Icon::Cross),
            ExecStatus::Executed(_) => Some(Icon::Check),
            _ => None,
        };
        Self { color, icon }
    }
}

/// The last run's word on the node as a chip in its [`StatusLook`].
fn status_chip(ui: &mut Ui, theme: &Theme, status: &ExecStatus) {
    let StatusLook { color, icon } = StatusLook::of(theme, status);
    let word = status_text(ui, status);
    Panel::hstack()
        .id(status_wid())
        .size((Sizing::HUG, Sizing::HUG))
        // 2 above and below the label tier, 8 after the word, and 6 before the icon, whose stroke
        // sits 2 inside its box, so both ends read as 8.
        .padding(Spacing::new(6.0, 2.0, 8.0, 2.0))
        .gap(STATUS_GAP)
        .child_align(Align::v(VAlign::Center))
        .chrome(Chrome::rounded(
            theme.wash(color, STATUS_FILL_ALPHA),
            Corners::all(theme.radius.control),
        ))
        .show(ui, |ui| {
            if let Some(icon) = icon {
                let side = theme.text.label;
                Panel::zstack()
                    .id_salt("inspector_status_icon")
                    .size((Sizing::fixed(side), Sizing::fixed(side)))
                    .show(ui, |ui| {
                        icon.draw(ui, Rect::new(0.0, 0.0, side, side), color);
                    });
            }
            Text::new(word)
                .id(status_wid().with("word"))
                .style(&TextStyle {
                    weight: FontWeight::MEDIUM,
                    ..support::colored_text(ui, color, theme.text.label)
                })
                .show(ui);
        });
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
            theme.wash(error, ERROR_FILL_ALPHA),
            Corners::all(theme.radius.control),
        ))
        // The bar runs the callout's height, and its rounded clip curves the bar's ends with it.
        .clip(ClipMode::Rounded)
        .show(ui, |ui| {
            Block::new()
                .id_salt("inspector_error_bar")
                .size((Sizing::fixed(ERROR_BAR_WIDTH), Sizing::FILL))
                .chrome(Chrome::fill(error))
                .show(ui);
            Text::new(message)
                .id(error_wid().with("message"))
                .style(&body_style(theme, ui))
                .text_wrap(TextWrap::Wrap)
                .margin(Spacing::new(0.0, pad, 0.0, pad))
                .show(ui);
        });
}

/// A log line's ink by level: info in the body ink, warn and error in their status inks.
fn log_color(theme: &Theme, ui: &Ui, level: LogLevel) -> RgbaF32 {
    match level {
        LogLevel::Info => ui.theme().text.color,
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

    fn show(self, ui: &mut Ui, wid: WidgetId) {
        let corners = Corners::all(PORT_DOT_SIDE * 0.5);
        let chrome = match self.fill {
            DotFill::Disc => Chrome::rounded(self.color, corners),
            DotFill::Ring => Chrome::rounded(RgbaF32::TRANSPARENT, corners)
                .with_border(Stroke::new(self.color, port_glyph::HOLLOW_WIDTH)),
            DotFill::Blank => Chrome::NONE,
        };
        Block::new()
            .id(wid)
            .size((Sizing::fixed(PORT_DOT_SIDE), Sizing::fixed(PORT_DOT_SIDE)))
            .chrome(chrome)
            .show(ui);
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
            .size((Sizing::FILL, Sizing::fixed(PORT_ROW_HEIGHT)))
            .gap(theme.space.sm)
            .child_align(Align::v(VAlign::Center))
            .show(ui, |ui| {
                dot.show(ui, wid.with("dot"));
                let name = Text::new(name)
                    .style(&TextStyle {
                        color: theme.ports.label,
                        ..body_style(theme, ui)
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
                            .style(&muted_style(theme, ui))
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

    /// The value in the node's resting field box.
    fn show(self, ui: &mut Ui, theme: &Theme, wid: WidgetId) {
        let style = match self.face {
            ValueFace::Text => body_style(theme, ui),
            ValueFace::Number => support::mono_text(ui, theme.text.body),
            ValueFace::Unset => {
                support::colored_text(ui, theme.const_editor.unset, theme.text.body)
            }
        };
        let fill = theme
            .const_editor
            .drag_value
            .chip
            .looks
            .normal
            .chrome
            .fill
            .clone();
        Panel::hstack()
            .id(wid)
            .size((Sizing::HUG, Sizing::HUG))
            .padding(Spacing::xy(theme.space.sm, FIELD_INSET))
            .chrome(Chrome::rounded(fill, Corners::all(theme.radius.control)))
            .show(ui, |ui| {
                Text::new(self.text)
                    .id(wid.with("text"))
                    .style(&style)
                    .text_wrap(TextWrap::EllipsisMiddle)
                    .show(ui);
            });
    }
}

/// A field box's inset above and below its text, as the node's field has.
const FIELD_INSET: f32 = 2.0;

fn title_style(theme: &Theme, ui: &Ui) -> TextStyle {
    TextStyle {
        weight: FontWeight::SEMI_BOLD,
        ..support::sized_text(ui, theme.text.title)
    }
}

/// The pane's de-emphasized ink: a port label's, so a port row reads as it does on the node.
fn muted_style(theme: &Theme, ui: &Ui) -> TextStyle {
    support::colored_text(ui, theme.ports.label, theme.text.label)
}

fn body_style(theme: &Theme, ui: &Ui) -> TextStyle {
    support::sized_text(ui, theme.text.body)
}

/// The label of the variant among `variants` that holds `value`, if one does.
fn variant_label<'a>(variants: &'a [ValueVariant], value: &ConstValue) -> Option<&'a str> {
    variants
        .iter()
        .find(|variant| &variant.value == value)
        .map(ValueVariant::label)
}

fn status_text(ui: &mut Ui, status: &ExecStatus) -> TextInput<'static> {
    match status {
        ExecStatus::None => TextInput::Borrowed("Not run"),
        ExecStatus::Cached => TextInput::Borrowed("Cached"),
        ExecStatus::Executed(secs) => {
            TextInput::Interned(fmt!(ui, "Ran in {}", Elapsed::new(*secs)))
        }
        ExecStatus::Running(at) => TextInput::Interned(fmt!(
            ui,
            "Running… {}",
            Elapsed::new(at.elapsed().as_secs_f64())
        )),
        ExecStatus::MissingInputs(_) => TextInput::Borrowed("Missing inputs"),
        ExecStatus::Errored(_) => TextInput::Borrowed("Failed"),
        ExecStatus::Skipped => TextInput::Borrowed("Skipped: a node before it failed"),
    }
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
        assert_eq!(row.size.h, PORT_ROW_HEIGHT);
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
            (PORT_DOT_SIDE, PORT_DOT_SIDE, 0.0),
            "the dot opens the row"
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
        assert_eq!((any.color, any.fill), (theme.ports.input, DotFill::Ring));
        assert_ne!(float.color, any.color);
    }

    /// The status chip wears each status's ink, the muted ink where a status has none, and the
    /// footer's icon for a failure and a run alone; the callout shows a failure's message, and
    /// nothing for a failure without one or any other status.
    #[test]
    fn the_status_chip_and_the_callout_follow_the_status() {
        let theme = Theme::default();
        let s = &theme.status;
        let muted = theme.colors.text_muted;
        let failed = ExecStatus::Errored(Some("no light frames".to_owned()));
        let cases = [
            (ExecStatus::None, muted, None, None),
            (ExecStatus::Cached, s.info, None, None),
            (
                ExecStatus::Executed(0.5),
                s.success,
                Some(Icon::Check),
                None,
            ),
            (
                ExecStatus::Running(std::time::Instant::now()),
                s.busy,
                None,
                None,
            ),
            (ExecStatus::MissingInputs(Vec::new()), s.warning, None, None),
            (ExecStatus::Errored(None), s.error, Some(Icon::Cross), None),
            (failed, s.error, Some(Icon::Cross), Some("no light frames")),
            (ExecStatus::Skipped, muted, None, None),
        ];
        for (status, color, icon, message) in &cases {
            assert_eq!(
                StatusLook::of(&theme, status),
                StatusLook {
                    color: *color,
                    icon: *icon
                },
                "{status:?}"
            );
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
