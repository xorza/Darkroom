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
use std::fmt;
use std::fmt::Display;
use std::fmt::Formatter;

use palantir::prelude::*;
use palantir::{DockOperation, FontWeight, TextInput, TextWrap};
use scenarium::DataType;
use scenarium::Library;
use scenarium::LogLevel;
use scenarium::NodeId;
use scenarium::{Binding, ConstValue, ValueVariant};

use crate::core::document::inspect_target::InspectTarget;
use crate::core::document::item_id::ItemId;
use crate::core::document::tab_ref::TabRef;
use crate::gui::frame::requests::Requests;
use crate::gui::graph_ctx::GraphCtx;
use crate::gui::graph_ctx::input_ctx::InputCtx;
use crate::gui::graph_ctx::node_ctx::NodeCtx;
use crate::gui::state::run_state::ExecStatus;
use crate::gui::theme::Theme;
use crate::gui::widgets::format::elapsed::Elapsed;
use crate::gui::widgets::icon::Icon;
use crate::gui::widgets::icon_button::IconButton;
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
                .gap(theme.space.xs)
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
    let error = match node.exec_status() {
        ExecStatus::Errored(message) => message.as_deref(),
        _ => None,
    };
    Panel::hstack()
        .id_salt("inspector_title")
        .size((Sizing::FILL, Sizing::HUG))
        .child_align(Align::v(VAlign::Center))
        .show(ui, |ui| {
            let title = if node.name().is_empty() {
                "(unnamed)"
            } else {
                node.name()
            };
            line(ui, title, title_style(theme, ui));
            support::hspacer(ui, "inspector_title_spacer");
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
    // The kind line earns its row only when it says something the title doesn't — an
    // unrenamed node repeats its func name.
    if !node.kind_label().eq_ignore_ascii_case(node.name()) {
        line(ui, node.kind_label(), muted_style(theme, ui));
    }
    // Status right under the title — the most-glanceable fact. The colored line is
    // self-labeling; no section header.
    let status_color = theme
        .exec_color(node.exec_status())
        .unwrap_or(theme.colors.text_muted);
    let status = status_text(ui, node.exec_status());
    line(
        ui,
        status,
        TextStyle {
            color: status_color,
            ..body_style(theme, ui)
        },
    );
    // The failure's cause beneath the status, in the error color: what turns a generic
    // status into an actionable message (e.g. "no light frames provided").
    if let Some(message) = error {
        line(
            ui,
            message,
            TextStyle {
                color: theme.status.error,
                ..body_style(theme, ui)
            },
        );
    }
    if node.sink() {
        line(ui, "Sink", muted_style(theme, ui));
    }
    if !node.description().is_empty() {
        line(ui, node.description(), muted_style(theme, ui));
    }

    let library = node.graph_ctx.library();
    if node.inputs().len() > 0 {
        section(ui, theme, "Inputs");
        for input in node.inputs() {
            let val = value_str(ui, input);
            InspectedPort {
                label: PortCaption::resolve(library, input.name(), input.data_type()),
                value: Some(val),
            }
            .show(ui, theme);
        }
    }
    if node.outputs().len() > 0 {
        section(ui, theme, "Outputs");
        for output in node.outputs() {
            InspectedPort {
                label: PortCaption::resolve(library, output.name(), output.data_type()),
                value: None,
            }
            .show(ui, theme);
        }
    }
    // This node's lines from the last run, level-colored, capped to the most recent few.
    if !logs.is_empty() {
        section(ui, theme, "Log");
        let start = logs.len().saturating_sub(LOG_LINE_CAP);
        for entry in &logs[start..] {
            line(
                ui,
                &entry.message,
                TextStyle {
                    color: log_color(theme, ui, entry.level),
                    ..body_style(theme, ui)
                },
            );
        }
    }
}

fn pin_wid() -> WidgetId {
    WidgetId::from_hash("darkroom.inspector.pin")
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

/// A bold section label with breathing room above it, so the groups read
/// by proximity inside the pane's tight line rhythm.
fn section(ui: &mut Ui, theme: &Theme, text: &'static str) {
    Text::new(text)
        .style(&TextStyle {
            weight: FontWeight::BOLD,
            ..muted_style(theme, ui)
        })
        .margin(Spacing::new(0.0, theme.space.md, 0.0, 0.0))
        .show(ui);
}

/// One port row, two tiers: a muted label line (the port name, plus its
/// type when the two differ), then the value on its own full-strength
/// line — so a long value wraps cleanly instead of orphaning its tail
/// tokens mid-`name: type = value` string. A `None` value renders the label
/// alone (an output).
#[derive(Debug)]
struct InspectedPort<'a> {
    label: PortCaption<'a>,
    value: Option<TextInput<'a>>,
}

impl InspectedPort<'_> {
    fn show(self, ui: &mut Ui, theme: &Theme) {
        let text = fmt!(ui, "{}", self.label);
        line(ui, text, muted_style(theme, ui));
        if let Some(v) = self.value {
            line(ui, v, body_style(theme, ui));
        }
    }
}

/// The label tier of a port row. Skips the type when it repeats the port
/// name (`Image · Image`, the dominant case; `Path · path` likewise) —
/// otherwise `name · type`, so even a valueless port announces its type.
///
/// Renders on demand into the record pass's text arena, which is where the
/// name was headed anyway: an open pane builds no owned string per port per
/// frame, in either case.
#[derive(Debug)]
struct PortCaption<'a> {
    name: &'a str,
    /// The declared type's display name, or `None` when it merely repeats
    /// `name` and the row would read as a stutter.
    type_name: Option<Cow<'a, str>>,
}

impl<'a> PortCaption<'a> {
    fn resolve(library: &'a Library, name: &'a str, data_type: &DataType) -> Self {
        let type_name = library.type_name(data_type);
        let type_name = (!type_name.eq_ignore_ascii_case(name)).then_some(type_name);
        Self { name, type_name }
    }
}

impl Display for PortCaption<'_> {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        f.write_str(self.name)?;
        match &self.type_name {
            Some(type_name) => write!(f, " · {type_name}"),
            None => Ok(()),
        }
    }
}

fn title_style(theme: &Theme, ui: &Ui) -> TextStyle {
    support::sized_text(ui, theme.text.title)
}

/// The pane's de-emphasized ink: a port label's, so a port row reads as it does on the node.
fn muted_style(theme: &Theme, ui: &Ui) -> TextStyle {
    support::colored_text(ui, theme.ports.label, theme.text.label)
}

fn body_style(theme: &Theme, ui: &Ui) -> TextStyle {
    support::sized_text(ui, theme.text.body)
}

/// The value tier of an input row: a constant one of the input's named variants holds reads by
/// that variant's label, as the node's own dropdown shows it.
///
/// The fixed answers stay borrowed and the formatted one goes straight into the text arena, so
/// an open pane builds no owned string per input per frame.
fn value_str<'a>(ui: &mut Ui, input: InputCtx<'a>) -> TextInput<'a> {
    match input.binding() {
        None => TextInput::Borrowed("—"),
        Some(Binding::Wire(_)) => TextInput::Borrowed("wired"),
        Some(Binding::Const(v)) => match variant_label(input.value_variants(), v) {
            Some(label) => TextInput::Borrowed(label),
            // `ConstValue::Display` — the same formatter a preview node's
            // value goes through via `DynamicValue::Display`, so a static
            // binding and a live one can't render one float two ways.
            None => TextInput::Interned(fmt!(ui, "{v}")),
        },
    }
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

    #[test]
    fn port_label_dedups_the_type_and_reuses_the_scenes_own_handle() {
        use scenarium::FsPathConfig;
        use std::sync::Arc;

        let lib = Library::default();
        let path_type = DataType::FsPath(Arc::new(FsPathConfig::default()));

        // Type repeats the name (case-insensitively) → the name alone, no
        // "Image · Image" stutter; distinct type → "name · type". A path
        // port announces its type when the name differs, with no value
        // shown or with one.
        let cases: [(&str, &DataType, &str); 4] = [
            ("Float", &DataType::Float, "Float"),
            ("Brightness", &DataType::Float, "Brightness · float"),
            ("Path", &path_type, "Path"),
            ("Output", &path_type, "Output · path"),
        ];
        for (name, data_type, expected) in cases {
            let label = PortCaption::resolve(&lib, name, data_type);
            assert_eq!(label.to_string(), expected, "label for {name}");
            // The deduped case is the dominant one, and it must resolve to the
            // declaration's own `&str` with nothing appended: that is the whole
            // reason an open panel renders a port row straight from the scene's
            // own handles. Only the differing case carries a second piece.
            let deduped = expected == name;
            assert_eq!(
                label.type_name.is_none(),
                deduped,
                "{name}: a deduped label is the declared name alone, a \
                 combined one carries the type beside it",
            );
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
