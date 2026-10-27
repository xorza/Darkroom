//! Node header: the title and the node's controls, in one row. Which controls —
//! run, the kind mark, disable, the cache menu and inspect — and
//! what each one does is this module's business; how one *looks* belongs to
//! [`IconButton`]. Drawn as the first child of each node body by
//! [`crate::gui::pane::graph_ui::node_ui::NodeUi`], over a 1 px rule that
//! divides it from the ports.

use std::collections::BTreeSet;
use std::f32::consts::{FRAC_PI_4, PI};

use palantir::prelude::*;
use palantir::{DockDrop, DockOperation, MenuSeparator, SplitSide};
use scenarium::{CacheMode, NodeId};

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
use crate::gui::widgets::hover::Hover;
use crate::gui::widgets::icon::Icon;
use crate::gui::widgets::icon_button::IconButton;
use crate::gui::widgets::inline_rename::{InlineRename, RenameEvent};
use crate::gui::widgets::port_glyph::PortGlyph;
use crate::gui::widgets::support;
use crate::gui::widgets::toggle::Toggle;

/// Character cap for a node title in the inline rename editor.
const NODE_NAME_MAX_CHARS: usize = 32;

/// One whole-node event-subscription pin: an event-colored triangle behind
/// the node's top-left corner, its apex pointing up-left toward the
/// incoming wire. Recorded by `NodeUi::draw_node` immediately *before* its
/// node's body, so it peeks out from behind the corner while keeping the
/// node stack's paint order (above lower nodes, below raised ones) and the
/// cull decision. The `port_glyph::HIT_SCALE`-grown box is centered on the corner
/// (world coords) while the triangle paints port-sized — the same
/// generous-hit-box treatment the port circles get. It's both a drop
/// target for an emitter's event wire *and* a drag source — pulling from
/// the *protruding* half starts a subscription wire aimed at an emitter
/// (see `SubscriptionUi`); the body-covered half yields presses to the
/// node (the body records after, so it hit-tests on top), while
/// drop-snapping (rect-based) still accepts the whole box. `hover` (over
/// while a drag snaps to it) tints the triangle as drop feedback.
pub(super) fn subscription_pin(ui: &mut Ui, node: NodeCtx<'_>, hover: Hover) {
    let theme = node.theme();
    // The emitter arrow turned half a turn — mirroring its apex from right to
    // left — plus a quarter, aiming it up-left along the wire arriving from
    // there. Placed by its grown box's center rather than in flow, so it
    // straddles the node's top-left corner.
    PortGlyph::arrow(
        wid::subscription_glyph(node.node_id),
        theme.ports.size,
        hover.lift(theme.event_color()),
    )
    .turn(PI + FRAC_PI_4)
    .centered_on(node.pos)
    .tooltip(Some(
        "Event subscription — drag to an emitter, or drop an event wire here",
    ))
    .show(ui);
}

/// The header row: the run button, the title and its kind mark, then — pinned to the right edge
/// by a `FILL` spacer — the disable button, the cache button and the inspect button. The sink
/// nodes' event-subscription pin is *not* drawn here — it records at canvas level, before the
/// node bodies, so it peeks out from behind the node's corner.
pub(super) fn show(ui: &mut Ui, record: &mut NodeRecord<'_, '_>) {
    let (theme, node) = (record.node.theme(), record.node);
    Panel::hstack()
        .id_salt("header")
        .size((Sizing::FILL, Sizing::HUG))
        .padding(card::band_padding(theme))
        .gap(theme.space.xs)
        .child_align(Align::v(VAlign::Center))
        .show(ui, |ui| {
            if node.runnable() && run_button(ui, node) {
                record
                    .out
                    .push_app(AppCommand::Run(RunCommand::Node(node.node_id)));
            }
            title(ui, record);
            kind_mark(ui, node);
            support::hspacer(ui, "header_spacer");
            if node.can_disable() {
                disable_button(ui, record);
            }
            if node.cacheable() {
                cache_button(ui, record);
            }
            if IconButton::press(
                Icon::Inspect,
                wid::inspect_button(node.node_id),
                theme.colors.badge_graph,
                "Inspect — values, status, log",
                Toggle::Off,
            )
            .show(ui, theme)
            {
                // Inspect this node: select it alone, and show the inspector that follows the
                // selection.
                record.out.push_graph(GraphIntent::SetSelection {
                    to: BTreeSet::from([ItemId::Node(node.node_id)]),
                });
                show_inspector(record);
            }
        });
    Separator::horizontal()
        .color(theme.colors.border_soft())
        .show(ui);
}

/// The one mark that says what kind of node this is, after its title: the sink icon for a sink,
/// else the impure icon, the tooltip naming every kind the node has. Nothing for a node of
/// neither kind.
fn kind_mark(ui: &mut Ui, node: NodeCtx<'_>) {
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
    IconButton::mark(
        icon,
        wid::of_node("kind_mark", node.node_id),
        theme.colors.badge_kind,
        tooltip,
    )
    .show(ui, theme);
}

/// Show the inspector that follows the selection: where it already sits, else in a pane split
/// off the right of the graph's, so the graph it inspects stays in view.
fn show_inspector(record: &mut NodeRecord<'_, '_>) {
    let tab = TabRef::Inspector(InspectTarget::Selection);
    let layout = &record.node.graph_ctx.document().layout;
    if layout.all_tabs().any(|open| open == tab) {
        record.out.push_view(DockOperation::ActivateTab { tab });
        return;
    }
    record.out.push_view(DockOperation::OpenTab { tab });
    if let Some(graph) = layout
        .groups()
        .find(|group| group.tabs().contains(&TabRef::Graph))
    {
        record.out.push_view(DockOperation::MoveTab {
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
        wid::of_node("run_button", node.node_id),
        theme.status.success,
        tooltip,
        Toggle::Off,
    )
    .show(ui, theme)
}

/// Exclude a sink from graph runs: on while the node is disabled.
fn disable_button(ui: &mut Ui, record: &mut NodeRecord<'_, '_>) {
    let node = record.node;
    let theme = node.theme();
    if IconButton::press(
        Icon::Disable,
        wid::of_node("disable_button", node.node_id),
        theme.colors.text_muted,
        "Disable — exclude this sink from graph runs",
        Toggle::from(node.disabled()),
    )
    .show(ui, theme)
    {
        record.out.push_graph(GraphIntent::SetNodeProperty {
            node_id: node.node_id,
            to: NodeProperty::Disabled(!node.disabled()),
        });
    }
}

/// The cache button and the menu it opens below itself: keep the output in RAM, save it to
/// disk, and clear what the node and the nodes after it hold. On while either bit is set.
fn cache_button(ui: &mut Ui, record: &mut NodeRecord<'_, '_>) {
    let node = record.node;
    let theme = node.theme();
    let wid = cache_button_wid(node.node_id);
    let cache = node.cache();
    let (ram, disk) = (cache.caches_in_ram(), cache.persists_to_disk());
    let clicked = IconButton::press(
        Icon::Cache,
        wid,
        theme.colors.badge_cache,
        "Cache — keep this node's output across runs",
        Toggle::from(ram || disk),
    )
    .show(ui, theme);
    if clicked && let Some(rect) = ui.response_for(wid).rect {
        ContextMenu::open(ui, wid, Vec2::new(rect.min.x, rect.max().y));
    }
    ContextMenu::for_id(wid).show(ui, |ui, popup| {
        let mut keep_ram = ram;
        if Checkbox::new(&mut keep_ram)
            .label("Keep in RAM")
            .show(ui)
            .changed
        {
            record.out.push_graph(GraphIntent::SetNodeProperty {
                node_id: node.node_id,
                to: NodeProperty::RuntimeCache(CacheMode::from_bits(keep_ram, disk)),
            });
        }
        let mut save_disk = disk;
        if Checkbox::new(&mut save_disk)
            .label("Save to disk")
            .show(ui)
            .changed
        {
            record.out.push_graph(GraphIntent::SetNodeProperty {
                node_id: node.node_id,
                to: NodeProperty::RuntimeCache(CacheMode::from_bits(ram, save_disk)),
            });
            // Turning the disk bit on publishes what the node already holds in RAM, without
            // which the value would reach disk only when the node next recomputed.
            if save_disk {
                record
                    .out
                    .push_app(AppCommand::Run(RunCommand::FlushCache(node.node_id)));
            }
        }
        MenuSeparator::new().show(ui);
        if MenuItem::new("Clear cache").show(ui, popup).left.clicked() {
            record
                .out
                .push_app(AppCommand::Run(RunCommand::EvictCache(node.node_id)));
        }
    });
}

fn cache_button_wid(node_id: NodeId) -> WidgetId {
    wid::of_node("cache_button", node_id)
}

/// The node title: an inline-renamable label. Double-click swaps it for
/// a `TextEdit`; commit emits [`GraphIntent::RenameNode`], single-click
/// selects (the label would otherwise swallow the body's click). A stub's
/// title is read-only: its record is a frozen snapshot no edit changes.
fn title(ui: &mut Ui, record: &mut NodeRecord<'_, '_>) {
    let node = record.node;
    let mode = ClickMode::with_shift(ui.modifiers().shift);
    // Borrowed straight off the node: an interned handle is only valid
    // within the record pass that minted it, and the widget reads the
    // name back on the label⇄editor swap to seed the draft.
    let rename = InlineRename::new(
        wid::rename(node.node_id),
        node.name(),
        &node.theme().inline_rename_title,
    )
    .max_chars(NODE_NAME_MAX_CHARS);
    let rename = if node.stub() {
        rename.read_only()
    } else {
        rename
    };
    match rename.show(ui) {
        RenameEvent::Idle => {}
        RenameEvent::Clicked => record.out.extend_graph(GraphIntent::click(
            mode,
            node.graph_ctx.selected(),
            ItemId::Node(node.node_id),
        )),
        RenameEvent::Committed(to) => record.out.push_graph(GraphIntent::RenameNode {
            node_id: node.node_id,
            to,
        }),
    }
}

#[cfg(test)]
mod tests;
