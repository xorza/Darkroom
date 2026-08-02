//! Node header bar: the title plus the node's indicator chips. Which chips —
//! `G` graph-open, `D` sink-disable, `↻` evict, `R`/`↓` cache, `i` inspect,
//! and the `■`/`~` markers — and what each one means is this module's
//! business; how a chip *looks* belongs to
//! [`Badge`].
//!
//! The markers ride in the [`header`] band beside the title; the run-time
//! label (left) and the interactive controls (right) share the [`status_row`]
//! below it. Drawn as the top children of each node body by
//! [`crate::gui::pane::graph::node::NodeUI`].

use std::f32::consts::{FRAC_PI_4, PI};

use palantir::{
    Align, Color, Configure, FontFamily, FontWeight, Panel, Sizing, Spacing, Spinner, Text,
    TextStyle, Ui, VAlign, WidgetId,
};
use scenarium::{CacheMode, NodeId};

use crate::core::edit::intent::types::{GraphIntent, NodeProperty};
use crate::gui::app::commands::AppCommand;
use crate::gui::app::commands::run::RunCommand;
use crate::gui::graph_ctx::node_ctx::NodeCtx;
use crate::gui::pane::graph::ctx::DrawCtx;
use crate::gui::pane::graph::node::port_color::event_color;
use crate::gui::pane::graph::node::wid;
use crate::gui::pane::graph::node::widget::exec_color;
use crate::gui::pane::graph::paint::inspector::{InspectMode, inspect_badge_wid};
use crate::gui::requests::Requests;
use crate::gui::state::run_state::ExecStatus;
use crate::gui::theme::Theme;
use crate::gui::widgets::badge::{BADGE_FONT, BADGE_SIZE, Badge};
use crate::gui::widgets::format::fmt_elapsed;
use crate::gui::widgets::inline_rename::InlineRename;
use crate::gui::widgets::port_glyph::PortGlyph;
use crate::gui::widgets::support::{
    CARD_HEADER_PAD_X, CARD_HEADER_PAD_Y, header_background, hspacer, play_triangle,
};

/// Character cap for a node title in the inline rename editor.
const NODE_NAME_MAX_CHARS: usize = 32;

/// Width floor for the run-time label, ~7 mono glyphs at [`BADGE_FONT`]. The
/// range this covers is pinned by `format::tests`.
///
/// The node is `Hug` above a min width, so anything that changes the
/// header's measured width moves the node's right edge — and with it the
/// cached intra-node offsets of every *output* port, which is what wires
/// anchor to. A live timer re-formats every frame, so without a floor a
/// running node twitches its outgoing wires each time the digit count
/// changes (`9.99s` → `10.00s`, `999.9ms` → `1.00s`). That is not an
/// `UndoStep`, so nothing requests the settle pass that would hide it.
///
/// A floor rather than a fixed width: every value up to `999.99s` fits
/// inside it and measures identically, and the rare longer one still
/// renders rather than clipping. Generous on purpose — costing a few px
/// of header is cheaper than being one glyph short of the common case.
const RUN_TIME_MIN_WIDTH: f32 = 52.0;

/// One whole-node event-subscription pin: an event-colored triangle behind
/// the node's top-left corner, its apex pointing up-left toward the
/// incoming wire. Recorded by `NodeUI::draw_one` immediately *before* its
/// node's body, so it peeks out from behind the corner while keeping the
/// node stack's paint order (above lower nodes, below raised ones) and the
/// cull decision. The `PORT_HIT_SCALE`-grown box is centered on the corner
/// (world coords) while the triangle paints port-sized — the same
/// generous-hit-box treatment the port circles get. It's both a drop
/// target for an emitter's event wire *and* a drag source — pulling from
/// the *protruding* half starts a subscription wire aimed at an emitter
/// (see `SubscriptionUI`); the body-covered half yields presses to the
/// node (the body records after, so it hit-tests on top), while
/// drop-snapping (rect-based) still accepts the whole box. `hovered` (set
/// while a drag snaps to it) tints the triangle as drop feedback.
pub(super) fn subscription_pin(ui: &mut Ui, theme: &Theme, node: NodeCtx<'_>, hovered: bool) {
    // The emitter arrow turned half a turn — mirroring its apex from right to
    // left — plus a quarter, aiming it up-left along the wire arriving from
    // there. Placed by its grown box's center rather than in flow, so it
    // straddles the node's top-left corner.
    PortGlyph::arrow(subscription_glyph_wid(node.id), theme.ports.size)
        .turn(PI + FRAC_PI_4)
        .fill(event_color(theme, hovered))
        .centered_on(node.pos)
        .tip("Event subscription — drag to an emitter, or drop an event wire here")
        .show(ui);
}

/// Stable id for a node's event-subscription pin. Keyed on the node (a
/// subscription is whole-node, not per-port), so `CanvasGeometry` /
/// `SubscriptionUI` reconstruct it to poll the pin's geometry as a wire
/// drop target.
pub(crate) fn subscription_glyph_wid(node_id: NodeId) -> WidgetId {
    wid::node("subscription_glyph", node_id)
}

/// The header bar: the node title (left) and the descriptive cluster (right) —
/// the markers (`■`/`~`), then the inspect chip. A `FILL` spacer between them
/// pins the cluster to the right edge (the run-time label and the interactive
/// controls ride in [`status_row`] below). The sink nodes' event-
/// subscription pin is *not* drawn here — it records at canvas level, before the
/// node bodies, so it peeks out from behind the node's corner.
/// Reports whether the inspect chip was clicked — cycling a panel needs
/// `&mut Inspectors`, which the draw holds only shared, so the caller applies
/// it once the draw is over.
pub(super) fn header(ui: &mut Ui, ncx: NodeCtx<'_>, dcx: DrawCtx<'_>, out: &mut Requests) -> bool {
    let (theme, node) = (ncx.theme(), ncx);
    // The header sits inside the body's border stroke (the layout folds
    // the stroke width into the body's padding), so it must round to the
    // stroke's *inner* radius, not the card's outer `corner_radius` —
    // see `Theme::card_inner_radius`.
    let r = theme.card.inner_radius();
    Panel::hstack()
        .id_salt("header")
        .size((Sizing::FILL, Sizing::HUG))
        .padding(Spacing::xy(CARD_HEADER_PAD_X, CARD_HEADER_PAD_Y))
        .gap(4.0)
        .child_align(Align::v(VAlign::Center))
        .background(header_background(theme, r))
        .show(ui, |ui| {
            // The run affordance leads the band, ahead of the title — the
            // one control that *does* something with the node's output
            // rather than configuring it. Only on nodes that resolve as a
            // run seed.
            if node.runnable() && play_chip(ui, theme, node) {
                // Run this node's cone — the same command the context menu's
                // "Run to this node" resolves to.
                out.push_app(AppCommand::Run(RunCommand::Node(node.id)));
            }
            title(ui, ncx, out);
            // Splits the title (left) from the descriptive cluster
            // (right): the markers, then inspect.
            hspacer(ui, "header_spacer");
            // Read-only markers — what the node *is* (flat tinted pills, not
            // interactive, so they read as labels). They ride here beside the
            // title; the interactive controls stay in `status_row` below.
            if node.sink() {
                Badge::marker(
                    "badge_sink",
                    "■",
                    theme.colors.badge_sink,
                    "Sink — runs for its effect, not for a value",
                )
                .show(ui);
            }
            if node.impure() {
                Badge::marker(
                    "badge_impure",
                    "~",
                    theme.colors.badge_impure,
                    "Impure — holds work that recomputes every run, never cached",
                )
                .show(ui);
            }
            // Inspect toggle: filled (checked) when pinned, accent outline
            // when open, muted-grey outline (`text_muted`) when closed.
            let mode = dcx.inspectors().mode(node.id);
            let color = if mode.is_some() {
                theme.colors.badge_graph
            } else {
                theme.colors.text_muted
            };
            Badge::control(
                "i",
                color,
                mode == Some(InspectMode::Pinned),
                inspect_badge_wid(node.id),
                "Inspect — values, status, log",
            )
            .show(ui)
        })
        .inner
}

/// The strip under the header: the run-time label left-aligned, a `FILL`
/// spacer, then the interactive chips right-aligned — `G` graph-open, `D`
/// sink-disable, `↻` evict, and `R`/`↓` cache. The controls group apart from the title's
/// identity (header above); the run-time reads as the row's status
/// counterweight.
pub(super) fn status_row(ui: &mut Ui, ncx: NodeCtx<'_>, out: &mut Requests) {
    let (theme, node) = (ncx.theme(), ncx);
    Panel::hstack()
        .id_salt("status_row")
        .size((Sizing::FILL, Sizing::HUG))
        // Extra top padding sets the controls off from the header bar (the body
        // vstack has no gap between rows). Order: left, top, right, bottom.
        .padding(Spacing::new(8.0, 7.0, 8.0, 2.0))
        .gap(4.0)
        .child_align(Align::v(VAlign::Center))
        .show(ui, |ui| {
            // Last-run time leads the row, tied to the node's status color —
            // the final time once executed, or live elapsed-so-far while
            // running (`App::record` repaints so it ticks). Mono/tabular so it
            // holds a column across a stack of nodes.
            let elapsed = match node.exec_status() {
                ExecStatus::Executed(secs) => Some(secs),
                ExecStatus::Running(at) => Some(at.elapsed().as_secs_f64()),
                _ => None,
            };
            if let Some(secs) = elapsed {
                let color = exec_color(theme, node.exec_status()).unwrap_or(ui.theme.text.color);
                // A comet spinner while computing, just left of the live time,
                // so glow + spin + ticking time read as one "running" cue.
                if matches!(node.exec_status(), ExecStatus::Running(_)) {
                    Spinner::new()
                        .diameter(BADGE_FONT)
                        .color(color)
                        .show(ui);
                }
                let elapsed = ui.fmt(format_args!("{}", fmt_elapsed(secs)));
                Text::new(elapsed)
                    .style(&TextStyle {
                        color,
                        font_size_px: BADGE_FONT,
                        family: FontFamily::Mono,
                        ..ui.theme.text.clone()
                    })
                    .min_size((RUN_TIME_MIN_WIDTH, 0.0))
                    .show(ui);
            }
            // Pushes the controls to the right edge, keeping the
            // run-time label pinned left.
            hspacer(ui, "ctrl_spacer");
            // Interactive controls: what you can *do* to the node. Bordered
            // chips that lift on hover.
            //
            // Only runnable sinks can be disabled from Darkroom, so running a
            // disabled node can still evaluate its ordinary upstream cone.
            if node.can_disable() {
                property_chip(
                    ui,
                    theme,
                    node,
                    PropertyChip {
                        glyph: "D",
                        // Never takes an accent: disabling is a suppression,
                        // not a stored value the way a cache bit is.
                        on_color: theme.colors.text_muted,
                        on: node.disabled(),
                        tag: "disable_badge",
                        tip: "Disable — exclude this sink from graph runs",
                        to: NodeProperty::Disabled(!node.disabled()),
                        then: None,
                    },
                    out,
                );
            }
            if node.can_evict_cache()
                && Badge::control(
                    "↻",
                    theme.colors.badge_cache,
                    false,
                    wid::node("cache_eviction_badge", node.id),
                    "Drop this node and downstream caches from RAM and disk",
                )
                .show(ui)
            {
                out.push_app(AppCommand::Run(RunCommand::EvictCache(node.id)));
            }
            // RuntimeCache toggles: the two independent bits of the node's `CacheMode` —
            // an `R` chip (keep the output resident in RAM, reused across runs) and
            // a `↓` chip (persist it to the on-disk store, surviving a reopen). Each
            // chip is filled when its bit is set; clicking flips just that bit.
            //
            // Quiet at rest: a chip inks muted grey until its bit is *on*, when it
            // takes the cache accent (amber). So an idle node's controls stay
            // monochrome and only an active cache carries color — the type-colored
            // ports and the status glow keep the stage.
            //
            // Shown only where direct storage controls can apply — see
            // `NodeCtx::cache_controls`.
            // (An impure node still paints the `~` marker below to say why.)
            if node.cache_controls() {
                let ram = node.cache().caches_in_ram();
                let disk = node.cache().persists_to_disk();
                // The two bits are the same chip twice — only which one the
                // click flips differs.
                for chip in [
                    PropertyChip {
                        glyph: "R",
                        on_color: theme.colors.badge_cache,
                        on: ram,
                        tag: "ram_badge",
                        tip: "RuntimeCache in RAM — keep the output resident, reused across runs this session",
                        to: NodeProperty::RuntimeCache(CacheMode::from_bits(!ram, disk)),
                        then: None,
                    },
                    PropertyChip {
                        glyph: "↓",
                        on_color: theme.colors.badge_cache,
                        on: disk,
                        tag: "disk_badge",
                        tip: "RuntimeCache to disk — persist the output across runs and reopens",
                        to: NodeProperty::RuntimeCache(CacheMode::from_bits(ram, !disk)),
                        // Turning the bit *on* publishes whatever is already in
                        // RAM, right now. Without it the value would reach disk
                        // only when the node next recomputes: a run that reuses
                        // a resident value writes no blob, so an unchanged node
                        // would sit "disk-cached" with nothing on disk.
                        then: (!disk)
                            .then_some(AppCommand::Run(RunCommand::FlushCache(node.id))),
                    },
                ] {
                    property_chip(ui, theme, node, chip, out);
                }
            }
        });
}

/// One control chip that writes a node property when clicked — the disable
/// toggle and the two cache bits, which differ only in glyph, tag, tip, and
/// which property the click sets.
///
/// Quiet at rest: the chip inks muted grey until its state is *on*, when it
/// takes `on_color`. So an idle node's controls stay monochrome and only an
/// active setting carries color — the type-coloured ports and the status
/// glow keep the stage.
#[derive(Debug)]
struct PropertyChip {
    glyph: &'static str,
    on_color: Color,
    on: bool,
    tag: &'static str,
    tip: &'static str,
    /// What a click sets — already carries the flipped value.
    to: NodeProperty,
    /// A side effect the click raises alongside the property, for a bit whose
    /// new value the runtime has to be *told* about rather than merely read on
    /// the next run. Raised only when the click flips the bit in the direction
    /// that needs it, so the field is `None` on both the chip and the flip that
    /// don't.
    then: Option<AppCommand>,
}

fn property_chip(
    ui: &mut Ui,
    theme: &Theme,
    node: NodeCtx<'_>,
    chip: PropertyChip,
    out: &mut Requests,
) {
    let color = if chip.on {
        chip.on_color
    } else {
        theme.colors.text_muted
    };
    if Badge::control(
        chip.glyph,
        color,
        chip.on,
        wid::node(chip.tag, node.id),
        chip.tip,
    )
    .show(ui)
    {
        out.push_graph(GraphIntent::SetNodeProperty {
            node_id: node.id,
            to: chip.to,
        });
        // After the intent, and it matters: the document tier drains before the
        // app tier runs, so the command compiles a graph that already carries
        // the property this click just set.
        if let Some(command) = chip.then {
            out.push_app(command);
        }
    }
}

/// The header's play chip: run the graph up to this node and keep its
/// outputs for preview — the same command as the context menu's "Run to
/// this node". Control-family framing (bordered [`BADGE_SIZE`] square,
/// hover-lifted tint), but the glyph is the SDF play triangle rather than
/// a font glyph, echoing the ports' triangle vocabulary and staying
/// optically centered at any zoom. Quiet at rest — muted ink like the
/// other idle controls — and takes the palette's success green
/// (`exec_executed_glow`) on hover: "go", pointing at the outcome the
/// click delivers.
///
/// Reports its own click, like every other chip in this file: the widget is
/// built here, so its response is read here rather than rediscovered by a
/// canvas-level sweep that would have to respell this id to find it.
fn play_chip(ui: &mut Ui, theme: &Theme, node: NodeCtx<'_>) -> bool {
    let tooltip = if node.disabled() {
        "Run to this node once — temporarily override its disabled flag"
    } else {
        "Run to this node — execute its upstream cone and keep the output for preview"
    };
    Badge::action(
        wid::node("play_badge", node.id),
        tooltip,
        draw_play_triangle,
        theme.colors.text_muted,
    )
    .hover_color(theme.status.success)
    .show(ui)
}

/// Play triangle about the chip center, nudged right — a play mark's
/// visual center sits left of its bounding box's. Points are inset by
/// the rounding radius: the SDF rounds by dilating, so the glyph grows
/// back out to the intended extents.
fn draw_play_triangle(ui: &mut Ui, color: Color) {
    play_triangle(ui, BADGE_SIZE, 0.5, color);
}

/// The node title: an inline-renamable label. Double-click swaps it for
/// a `TextEdit`; commit emits [`GraphIntent::RenameNode`], single-click
/// selects (the label would otherwise swallow the body's click).
fn title(ui: &mut Ui, ncx: NodeCtx<'_>, out: &mut Requests) {
    let node = ncx;
    let shift = ui.modifiers().shift;
    let id = wid::rename(node.id);
    // Interned here rather than carried on the node: the widget holds the
    // handle across the label⇄editor swap, and this is the one place the
    // name is drawn.
    let name = ui.intern(node.name());
    let ev = InlineRename::new(id, name, &ncx.theme().inline_rename)
        .max_chars(NODE_NAME_MAX_CHARS)
        .style(&TextStyle {
            weight: FontWeight::Bold,
            ..ui.theme.text.clone()
        })
        .show(ui);
    if ev.clicked {
        out.extend_graph(GraphIntent::click(shift, ncx.graph_ctx.selected(), node.id));
    }
    if let Some(to) = ev.committed {
        out.push_graph(GraphIntent::RenameNode {
            node_id: node.id,
            to,
        });
    }
}

#[cfg(test)]
mod tests {
    use scenarium::{CacheMode, NodeId};

    use crate::core::document::harness::DocFixture;
    use crate::core::edit::intent::types::{GraphIntent, NodeProperty};
    use crate::gui::app::commands::AppCommand;
    use crate::gui::app::commands::run::RunCommand;
    use crate::gui::pane::graph::harness::CanvasHarness;
    use crate::gui::pane::graph::node::wid;

    /// What one click on a chip raised, in the two tiers it can reach.
    #[derive(Debug)]
    struct ChipClick {
        node_id: NodeId,
        intents: Vec<GraphIntent>,
        commands: Vec<AppCommand>,
    }

    /// Click the `↓` chip on the fixture's only node, over a document whose
    /// node already sits at `from`.
    fn click_disk_chip(from: CacheMode) -> ChipClick {
        let mut h = CanvasHarness::new(DocFixture::probes(1));
        let node_id = h.node(0);
        h.doc_mut().graph.find_mut(node_id).unwrap().cache = from;
        // Two frames so the node body — and with it the chip — has recorded
        // and carries a rect to aim at.
        h.prime(2);
        h.ui.click_at(h.ui.center_of(wid::node("disk_badge", node_id)));
        let intents = h.frame();
        ChipClick {
            node_id,
            intents,
            commands: std::mem::take(&mut h.commands),
        }
    }

    /// Turning the disk bit **on** raises two things: the property edit, and
    /// the flush that publishes whatever the node already holds in RAM —
    /// without which the value would reach disk only when the node next
    /// recomputed. Turning it **off** raises the edit alone: nothing is left to
    /// publish, and re-sending the program would only cost an install.
    ///
    /// Both tiers are asserted per case, since the flush is only correct
    /// *because* the edit travels with it: the document tier drains first, so
    /// the command compiles a graph already carrying the new mode.
    #[test]
    fn disk_chip_flushes_the_cache_only_when_it_turns_the_bit_on() {
        let cases = [
            (CacheMode::None, CacheMode::Disk, true),
            (CacheMode::Ram, CacheMode::Both, true),
            (CacheMode::Disk, CacheMode::None, false),
            (CacheMode::Both, CacheMode::Ram, false),
        ];

        for (from, to, flushes) in cases {
            let click = click_disk_chip(from);
            let edits: Vec<&NodeProperty> = click
                .intents
                .iter()
                .filter_map(|intent| match intent {
                    GraphIntent::SetNodeProperty { node_id, to } if *node_id == click.node_id => {
                        Some(to)
                    }
                    _ => None,
                })
                .collect();
            assert!(
                matches!(edits[..], [NodeProperty::RuntimeCache(mode)] if *mode == to),
                "{from:?} + one click must set exactly {to:?}, got {edits:?}"
            );

            let flushed = click.commands.iter().any(|command| {
                matches!(command, AppCommand::Run(RunCommand::FlushCache(id)) if *id == click.node_id)
            });
            assert_eq!(
                flushed, flushes,
                "{from:?} → {to:?} raised {:?}",
                click.commands
            );
        }
    }
}
