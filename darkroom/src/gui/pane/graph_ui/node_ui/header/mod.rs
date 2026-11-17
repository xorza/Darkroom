//! Node header: the title and the node's controls (run, the kind mark, disable, the cache menu
//! and inspect), in one row. What each does is this module's business; how one looks belongs
//! to [`IconButton`]. Drawn as the first child of each node body by
//! [`crate::gui::pane::graph_ui::node_ui::NodeUi`], over a rule that divides it from the ports.

use std::collections::BTreeSet;

use palantir::{
    Checkbox, Configure, DockDrop, DockOperation, EditableLabel, Menu, MenuItem, MenuSeparator,
    Modifiers, Panel, Separator, SizeSpec, SplitSide, Ui, VAlign, WidgetId,
};

use scenarium::{CacheMode, GraphId, NodeId, TriggerPort};

use crate::core::document::inspect_target::InspectTarget;
use crate::core::document::item_id::ItemId;
use crate::core::document::tab_ref::TabRef;
use crate::core::edit::graph_intent::GraphIntent;
use crate::core::edit::graph_intent::click_mode::ClickMode;
use crate::core::edit::graph_intent::node_property::NodeProperty;
use crate::gui::frame::app_command::AppCommand;
use crate::gui::frame::app_command::RunCommand;
use crate::gui::graph_ctx::node_ctx::NodeCtx;
use crate::gui::pane::graph_ui::node_ui::card;
use crate::gui::pane::graph_ui::node_ui::node_record::NodeRecord;
use crate::gui::pane::graph_ui::wid;
use crate::gui::port_ref::PortKind;
use crate::gui::theme::color;
use crate::gui::theme::hover::Hover;
use crate::gui::widgets::format;
use crate::gui::widgets::icon::Icon;
use crate::gui::widgets::icon_button::IconButton;
use crate::gui::widgets::port_glyph::PortGlyph;
use crate::gui::widgets::support;

/// The title's width floor, so a short name still presents an easy double-click target and the
/// editor does not collapse to a caret when the draft is emptied.
const NODE_NAME_MIN_WIDTH: f32 = 40.0;

/// A node's own trigger, the first item of its header row: an event-colored arrow pointing into
/// the node, hung on its left edge at the header's middle as an input port's circle hangs on its
/// row, so an event wire meets it head-on as a data wire meets an input. It is both a drop target
/// for an event wire and a drag source, which pulls a wire toward an event (see
/// `SubscriptionUi`); `hover` tints it as drop feedback. A node that lost its trigger to a ghost
/// subscription shows its ghost there instead, dimmed, where nothing starts or lands.
fn own_trigger(ui: &mut Ui, record: &NodeRecord<'_, '_>) {
    let node = record.node;
    let (theme, graph, own) = (
        node.theme(),
        node.graph_ctx.graph(),
        TriggerPort::from(node.node_id),
    );
    let geometry = record.stage.ctx.geometry();
    let (wid, ink, hovered, tooltip) = if node.has_own_trigger() {
        (
            wid::trigger_glyph(graph, own),
            theme.port.event,
            geometry.triggers.is_hovered(own),
            Some("Trigger — drag to an event, or drop an event wire here"),
        )
    } else if node.ghost_triggered() {
        (
            wid::ghost_trigger_glyph(graph, own),
            color::wash(theme.port.event, GHOST_TRIGGER_INK, theme.canvas.bg),
            geometry.ghost_triggers.is_hovered(own),
            node.tooltips().then_some(
                "A subscription kept as a ghost ends here: this node no longer has a trigger",
            ),
        )
    } else {
        return;
    };
    PortGlyph::arrow(wid, theme.port.size, Hover::from(hovered).lift(ink))
        .margin(theme.port_margin(PortKind::Input, theme.port.radius()))
        .tooltip(tooltip)
        .show(ui, theme);
}

/// How much of the event ink a ghost trigger keeps over the canvas: dimmed, as it is kept, not
/// live.
const GHOST_TRIGGER_INK: f32 = 0.55;

/// The header row: the node's own trigger on its left edge, the run button, the title and its
/// kind mark, then, pinned to the right edge by a `FILL` spacer, the disable button, the cache
/// button and the inspect button.
pub(super) fn show(ui: &mut Ui, record: &mut NodeRecord<'_, '_>) {
    let (theme, node) = (record.node.theme(), record.node);
    Panel::hstack()
        .id_salt("header")
        .size(SizeSpec::FILL_W)
        .padding(card::band_padding(theme))
        .gap(theme.space.xs)
        .child_align(VAlign::Center)
        .show(ui, |ui| {
            own_trigger(ui, record);
            if let Some(program_id) = node.runnable()
                && run_button(ui, node)
            {
                record
                    .stage
                    .out
                    .push_app(AppCommand::Run(RunCommand::Node(program_id)));
            }
            title(ui, record);
            kind_mark(
                ui,
                node,
                wid::of_node("kind_mark", node.graph_ctx.graph(), node.node_id),
            );
            support::hspacer(ui, "header_spacer");
            if node.can_disable() {
                disable_button(ui, record);
            }
            if node.cacheable() {
                cache_button(ui, record);
            }
            if IconButton::press(
                Icon::Inspect,
                wid::inspect_button(node.graph_ctx.graph(), node.node_id),
                theme.colors.accent,
                "Inspect — values, status, log",
            )
            .show(ui, theme)
            {
                // Inspect this node: select it alone, and show the inspector that follows the
                // selection.
                record.stage.out.push_graph(
                    record.node.graph_ctx.graph(),
                    GraphIntent::SetSelection {
                        to: BTreeSet::from([ItemId::Node(node.node_id)]),
                    },
                );
                show_inspector(record);
            }
        });
    Separator::horizontal().show(ui);
}

/// The one mark that says what kind of node this is, after its title on the node and in the
/// inspector: the sink icon for a sink, else the impure icon, the tooltip naming every kind the
/// node has. Nothing for a node of neither kind.
pub(crate) fn kind_mark(ui: &mut Ui, node: NodeCtx<'_>, wid: WidgetId) {
    let (icon, tooltip) = match (node.sink(), node.impure()) {
        (true, true) => (
            Icon::Sink,
            "Sink \u{b7} impure — runs for its effect and recomputes every run",
        ),
        (true, false) => (Icon::Sink, "Sink — runs for its effect, not for a value"),
        (false, true) => (Icon::Impure, "Impure — recomputes every run, never cached"),
        (false, false) => return,
    };
    let theme = node.theme();
    IconButton::mark(icon, wid, theme.badge.kind, tooltip).show(ui, theme);
}

/// Show the inspector that follows the selection: where it already sits, else in a pane split
/// off the right of the node's graph's, so the graph it inspects stays in view.
fn show_inspector(record: &mut NodeRecord<'_, '_>) {
    let tab = TabRef::Inspector(InspectTarget::Selection);
    let layout = &record.node.graph_ctx.document().layout;
    if layout.all_tabs().any(|open| open == tab) {
        record
            .stage
            .out
            .push_view(DockOperation::ActivateTab { tab });
        return;
    }
    record.stage.out.push_view(DockOperation::OpenTab { tab });
    if let Some(graph) = layout.groups().find(|group| {
        group
            .tabs()
            .contains(&TabRef::Graph(record.node.graph_ctx.graph()))
    }) {
        record.stage.out.push_view(DockOperation::MoveTab {
            tab,
            to: DockDrop::Split {
                group: graph.id(),
                side: SplitSide::Right,
            },
        });
    }
}

/// Run to this node: muted at rest, the success ink under the pointer, as the press runs it.
fn run_button(ui: &mut Ui, node: NodeCtx<'_>) -> bool {
    let tooltip = if node.disabled() {
        "Run to this node once — temporarily override its disabled flag"
    } else {
        "Run to this node — execute its upstream cone and keep the output for preview"
    };
    let theme = node.theme();
    IconButton::press(
        Icon::Run,
        wid::of_node("run_button", node.graph_ctx.graph(), node.node_id),
        theme.status.success,
        tooltip,
    )
    .show(ui, theme)
}

/// Exclude a sink from graph runs: on while the node is disabled.
fn disable_button(ui: &mut Ui, record: &mut NodeRecord<'_, '_>) {
    let node = record.node;
    let theme = node.theme();
    if IconButton::press(
        Icon::Disable,
        wid::of_node("disable_button", node.graph_ctx.graph(), node.node_id),
        theme.ink.muted,
        "Disable — exclude this sink from graph runs",
    )
    .selected(node.disabled())
    .show(ui, theme)
    {
        record.stage.out.push_graph(
            record.node.graph_ctx.graph(),
            GraphIntent::SetNodeProperty {
                node_id: node.node_id,
                to: NodeProperty::Disabled(!node.disabled()),
            },
        );
    }
}

/// The cache button and the menu it opens below itself: keep the output in RAM, save it to
/// disk, and clear what the node and the nodes after it hold. On while either bit is set.
fn cache_button(ui: &mut Ui, record: &mut NodeRecord<'_, '_>) {
    let node = record.node;
    let theme = node.theme();
    let wid = cache_button_wid(node.graph_ctx.graph(), node.node_id);
    let cache = node
        .cache()
        .expect("a header shows the cache button of a node with a body");
    let (ram, disk) = (cache.caches_in_ram(), cache.persists_to_disk());
    IconButton::press(
        Icon::Cache,
        wid,
        theme.badge.cache,
        "Cache — keep this node's output across runs",
    )
    .selected(ram || disk)
    .show(ui, theme);
    let trigger = ui.response_for(wid);
    Menu::below(&trigger).show(ui, |ui| {
        let mut keep_ram = ram;
        if Checkbox::new(&mut keep_ram)
            .label("Keep in RAM")
            .show(ui)
            .changed
        {
            record.stage.out.push_graph(
                record.node.graph_ctx.graph(),
                GraphIntent::SetNodeProperty {
                    node_id: node.node_id,
                    to: NodeProperty::RuntimeCache(CacheMode::from_bits(keep_ram, disk)),
                },
            );
        }
        let mut save_disk = disk;
        if Checkbox::new(&mut save_disk)
            .label("Save to disk")
            .show(ui)
            .changed
        {
            record.stage.out.push_graph(
                record.node.graph_ctx.graph(),
                GraphIntent::SetNodeProperty {
                    node_id: node.node_id,
                    to: NodeProperty::RuntimeCache(CacheMode::from_bits(ram, save_disk)),
                },
            );
            // Turning the disk bit on publishes what the node already holds in RAM, without
            // which the value would reach disk only when the node next recomputed.
            if save_disk {
                record
                    .stage
                    .out
                    .push_app(AppCommand::Run(RunCommand::FlushCache(node.addr())));
            }
        }
        MenuSeparator::new().show(ui);
        if MenuItem::new("Clear cache").show(ui).clicked() {
            record
                .stage
                .out
                .push_app(AppCommand::Run(RunCommand::EvictCache(node.addr())));
        }
    });
}

fn cache_button_wid(graph: GraphId, node_id: NodeId) -> WidgetId {
    wid::of_node("cache_button", graph, node_id)
}

/// The node title: an inline-renamable label. Double-click swaps it for
/// a `TextEdit`; commit emits [`GraphIntent::RenameNode`], single-click
/// selects (the label would otherwise swallow the body's click). A ghost node's
/// title is read-only: its record is a frozen snapshot no edit changes.
fn title(ui: &mut Ui, record: &mut NodeRecord<'_, '_>) {
    let node = record.node;
    let mode = ClickMode::with_shift(ui.modifiers().contains(Modifiers::SHIFT));
    let title = EditableLabel::new(node.name())
        .id(wid::rename(node.graph_ctx.graph(), node.node_id))
        .style(&node.theme().flat_text_edit)
        .bold()
        .max_chars(format::NAME_MAX_CHARS)
        .editable(!node.ghost())
        .min_size((NODE_NAME_MIN_WIDTH, 0.0))
        .show(ui);
    if let Some(to) = title.committed_text {
        record.stage.out.push_graph(
            record.node.graph_ctx.graph(),
            GraphIntent::RenameNode {
                node_id: node.node_id,
                to,
            },
        );
    } else if !title.editing && title.response.clicked() {
        record.stage.out.extend_graph(
            record.node.graph_ctx.graph(),
            GraphIntent::click(mode, node.graph_ctx.selected(), ItemId::Node(node.node_id)),
        );
    }
}

#[cfg(test)]
mod tests;
