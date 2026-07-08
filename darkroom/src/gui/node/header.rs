//! Node header bar: the title plus the node's indicator chips, split into two
//! visual families so a toggle can't be mistaken for a fact. **Controls** are
//! bordered, hover-lifting chips you act on — `S` subgraph-open, `D` disable,
//! `C` cache, and the `i` inspect chip. **Markers** are flat tinted pills that
//! only describe the node — `T` terminal and `~` impure. In [`status_row`] the
//! markers sit left (by the run-time label), the controls right. Drawn as the
//! top child of each node body by [`crate::gui::node::NodeUI`].

use aperture::{
    Align, Background, Color, Configure, Corners, FontWeight, HAlign, Panel, Sense, Shape, Sizing,
    Spacing, Spinner, Stroke, Text, TextStyle, Tooltip, Ui, VAlign, WidgetId,
};
use glam::Vec2;
use scenarium::graph::{CacheMode, NodeId};

use crate::core::edit::intent::{Intent, NodeProperty};
use crate::gui::canvas::inspector::{InspectMode, inspect_badge_wid};
use crate::gui::node::port_color::event_color;
use crate::gui::node::{RecordCtx, click_intents, exec_color, node_rename_wid};
use crate::gui::run_state::ExecStatus;
use crate::gui::scene::SceneNode;
use crate::gui::theme::Theme;
use crate::gui::widgets::inline_rename::InlineRename;

/// Character cap for a node title in the inline rename editor.
const NODE_NAME_MAX_CHARS: usize = 32;

/// Side of a header indicator chip (px), and its glyph font size.
const BADGE_SIZE: f32 = 18.0;
const BADGE_FONT: f32 = 12.0;

/// Shared chip tint opacity: a marker's fill and a hollow control's hover-lift
/// both paint their color at this alpha, so the two families feel like one system.
const CHIP_TINT_ALPHA: f32 = 0.20;

/// Compact run-time label: seconds → `s` / `ms` / `µs` at the scale
/// that keeps 2–3 significant digits.
pub(crate) fn fmt_elapsed(secs: f64) -> String {
    if secs >= 1.0 {
        format!("{secs:.2}s")
    } else if secs >= 1e-3 {
        format!("{:.1}ms", secs * 1e3)
    } else {
        format!("{:.0}µs", secs * 1e6)
    }
}

/// The header bar (title + inspect chip). A terminal node also carries a
/// whole-node event-subscription pin — a white triangle overhanging the
/// top-left edge — so the header is overlaid in a zstack: the pin floats
/// over the bar's left edge without reflowing the title (zstack children
/// don't push each other). Non-terminal nodes draw the bar directly.
pub(crate) fn header(ui: &mut Ui, rcx: RecordCtx<'_>, node: &SceneNode, out: &mut Vec<Intent>) {
    if !node.terminal {
        header_bar(ui, rcx, node, out);
        return;
    }
    let theme = rcx.theme;
    Panel::zstack()
        .id_salt("header_overlay")
        .size((Sizing::FILL, Sizing::Hug))
        .show(ui, |ui| {
            header_bar(ui, rcx, node, out);
            // Highlighted while an event drag is snapped to this pin.
            let snapped = rcx.port_frame.subs.is_hovered(node.id);
            subscription_glyph(ui, theme, node.id, snapped);
        });
}

/// One whole-node event-subscription pin: a white triangle overhanging the
/// node's top-left corner, its apex pointing up-left toward the incoming
/// wire. Negative top *and* left margins pull it out past both edges, like a
/// port circle overhangs its edge. It's both a drop target for an emitter's
/// event wire *and* a drag source — pulling from it starts a subscription
/// wire aimed at an emitter (see `SubscriptionUI`). `hovered` (set while a
/// drag snaps to it) tints the triangle as drop feedback.
fn subscription_glyph(ui: &mut Ui, theme: &Theme, node_id: NodeId, hovered: bool) {
    let port = theme.port_size;
    // Overhang past the body's inner edge (the border-folded padding) by the
    // dot's radius on each axis, centering the glyph box on the body corner —
    // the same half-on-the-edge overhang the port circles use.
    let overhang = theme.port_radius() + theme.node_border_width * 2.0;
    // Rotate the base (left-pointing) triangle +45° about its center so the
    // apex points up-left, aligned with the wire arriving from there. The
    // rotated points are passed straight to the SDF triangle primitive; the
    // layout box is unchanged (the glyph isn't clipped to the owner rect, so
    // the rotated apex may exceed it).
    let c = Vec2::splat(port * 0.5);
    let rot = Vec2::from_angle(std::f32::consts::FRAC_PI_4);
    let tf = |v: Vec2| c + rot.rotate(v - c);
    // `CLICK | DRAG`, like the emitter glyph / port circles: `DRAG` pulls a
    // subscription wire out of the pin and captures the press so it doesn't
    // fall through to the node body's own drag. A plain click on the pin no
    // longer selects the node — the same tradeoff those glyphs already make.
    let pin = Panel::zstack()
        .id(subscription_glyph_wid(node_id))
        .size((Sizing::Fixed(port), Sizing::Fixed(port)))
        .align(Align::new(HAlign::Left, VAlign::Top))
        .margin(Spacing::new(-overhang, -overhang, 0.0, 0.0))
        .sense(Sense::CLICK | Sense::DRAG)
        .show(ui, |ui| {
            ui.add_shape(Shape::Triangle {
                a: tf(Vec2::new(port, 0.0)),
                b: tf(Vec2::new(port, port)),
                c: tf(Vec2::new(0.0, port * 0.5)),
                radius: 0.0,
                fill: event_color(theme, hovered).into(),
                stroke: Stroke::ZERO,
            });
        });
    Tooltip::for_(&pin.response.snapshot())
        .text("Event subscription — drag to an emitter, or drop an event wire here")
        .show(ui);
}

/// Stable id for a node's event-subscription pin. Keyed on the node (a
/// subscription is whole-node, not per-port), so `PortFrame` /
/// `SubscriptionUI` reconstruct it to poll the pin's geometry as a wire
/// drop target.
pub(crate) fn subscription_glyph_wid(node_id: NodeId) -> WidgetId {
    WidgetId::from_hash(("graph.node.subscription_glyph", node_id))
}

/// The header bar proper: the node title plus the right-aligned inspect
/// chip. Indicator chips + the run-time label live in [`status_row`] below
/// it so adding/removing the time label never reflows the title.
fn header_bar(ui: &mut Ui, rcx: RecordCtx<'_>, node: &SceneNode, out: &mut Vec<Intent>) {
    let theme = rcx.theme;
    // The header sits inside the body's border stroke (the layout folds
    // the stroke width into the body's padding). Its top corners must
    // follow the stroke's *inner* radius — `node_corner_radius` minus the
    // stroke width (the node draws a `2 × node_border_width` stroke) —
    // otherwise the header's rounder corner leaves a wedge of body
    // `node_fill` showing between it and the (selection-lit) stroke.
    let r = (theme.node_corner_radius - theme.node_border_width * 2.0).max(0.0);
    Panel::hstack()
        .id_salt("header")
        .size((Sizing::FILL, Sizing::Hug))
        .padding(Spacing::xy(8.0, 4.0))
        .gap(4.0)
        .child_align(Align::v(VAlign::Center))
        .background(Background {
            fill: theme.colors.header_fill.into(),
            corners: Corners::new(r, r, 0.0, 0.0),
            ..Default::default()
        })
        .show(ui, |ui| {
            title(ui, rcx, node, out);
            // FILL spacer pushes the inspect chip to the header's right
            // edge, opposite the title.
            Panel::hstack()
                .id_salt("header_spacer")
                .size((Sizing::FILL, Sizing::Hug))
                .show(ui, |_| {});
            // Inspect toggle: filled (checked) when pinned, accent outline
            // when open, muted-grey outline (`text_muted`) when closed —
            // visible on the header without competing with the accent
            // S/T/C badges. The click is consumed in `Inspectors::apply`
            // via this chip's deterministic id, so the returned flag is
            // ignored here. Hidden on boundary nodes — they're pure
            // routing, no runtime values / status worth inspecting.
            if !node.boundary {
                let mode = rcx.inspectors.mode(node.id);
                let color = if mode.is_some() {
                    theme.colors.badge_subgraph
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
                .show(ui, theme);
            }
        });
}

/// The status strip under the header: the last-run time label and the read-only
/// markers (`T` terminal, `~` impure) on the left, then a `FILL` spacer, then
/// the interactive controls (`S` subgraph-open, `D` disable, `C` cache) on the
/// right. Its own row so the time appearing/disappearing doesn't resize the
/// header; the disable chip always shows, so the row's height is reserved
/// regardless.
pub(crate) fn status_row(ui: &mut Ui, rcx: RecordCtx<'_>, node: &SceneNode, out: &mut Vec<Intent>) {
    let theme = rcx.theme;
    Panel::hstack()
        .id_salt("status_row")
        .size((Sizing::FILL, Sizing::Hug))
        // Extra top padding sets the markers/controls off from the header bar
        // (the body vstack has no gap between rows). Order: left, top, right, bottom.
        .padding(Spacing::new(8.0, 7.0, 8.0, 2.0))
        .gap(4.0)
        .child_align(Align::v(VAlign::Center))
        .show(ui, |ui| {
            // Run time in the node's status color so it ties to the glow:
            // the final time once executed, or live elapsed-so-far while
            // running (`App::frame` repaints so it ticks).
            let elapsed = match node.exec_status {
                ExecStatus::Executed(secs) => Some(secs),
                ExecStatus::Running(at) => Some(at.elapsed().as_secs_f64()),
                _ => None,
            };
            if let Some(secs) = elapsed {
                let color = exec_color(theme, node.exec_status).unwrap_or(ui.theme.text.color);
                // A comet spinner while computing, just left of the live time,
                // so glow + spin + ticking time read as one "running" cue.
                if matches!(node.exec_status, ExecStatus::Running(_)) {
                    Spinner::new().size(BADGE_FONT).color(color).show(ui);
                }
                Text::new(fmt_elapsed(secs))
                    .style(TextStyle {
                        color,
                        font_size_px: BADGE_FONT,
                        ..ui.theme.text
                    })
                    .show(ui);
            }
            // Read-only markers (left cluster, beside the time label): what
            // the node *is*. Flat tinted pills, not interactive — so they read
            // as labels rather than the toggles on the right.
            if node.terminal {
                Badge::marker(
                    "badge_t",
                    "T",
                    theme.colors.badge_terminal,
                    "Terminal — output sink",
                )
                .show(ui, theme);
            }
            if node.impure {
                Badge::marker(
                    "badge_impure",
                    "~",
                    theme.colors.badge_impure,
                    "Impure — recomputes every run, never cached",
                )
                .show(ui, theme);
            }
            // FILL spacer splits the markers (left) from the controls (right).
            Panel::hstack()
                .id_salt("badge_spacer")
                .size((Sizing::FILL, Sizing::Hug))
                .show(ui, |_| {});
            // Interactive controls (right cluster): what you can *do* to the
            // node. Bordered chips that lift on hover.
            //
            // Subgraph chip is the open-in-tab affordance. We only *draw* it
            // here (with its stable id); the click is read next frame in
            // `emit_subgraph_opens` (prepass) so the open applies before the
            // record — letting the subgraph record a pass earlier and its
            // connections draw with no first-frame gap.
            if node.subgraph.is_some() {
                Badge::control(
                    "S",
                    theme.colors.badge_subgraph,
                    true,
                    subgraph_badge_wid(node.id),
                    "Open subgraph",
                )
                .show(ui, theme);
            }
            // Disable toggle: filled when the node is excluded from
            // execution. Muted swatch (it's "off", not an alarm).
            let disable_toggled = Badge::control(
                "D",
                theme.colors.text_muted,
                node.disabled,
                disable_badge_wid(node.id),
                "Disable — exclude from the run",
            )
            .show(ui, theme);
            if disable_toggled {
                out.push(Intent::SetNodeProperty {
                    node_id: node.id,
                    to: NodeProperty::Disabled(!node.disabled),
                });
            }
            // Cache toggles: the two independent bits of the node's `CacheMode` —
            // an `R` chip (keep the output resident in RAM, reused across runs) and
            // a `↓` chip (persist it to the on-disk store, surviving a reopen). Each
            // chip is filled when its bit is set; clicking flips just that bit.
            // Muted swatch, like the disable toggle — a config flip, not an alarm.
            // Both suppressed where caching can't apply: boundary nodes (pure
            // routing), self-caching nodes like the file-cache passthrough
            // (`uncacheable`), terminal sinks (no downstream consumer to serve a
            // cached output to), and impure nodes (no content digest, so no mode is
            // honored — the `~` marker shows why instead).
            if !node.uncacheable && !node.terminal && !node.impure {
                let ram = node.cache.caches_in_ram();
                let disk = node.cache.persists_to_disk();
                if Badge::control(
                    "R",
                    theme.colors.badge_cache,
                    ram,
                    ram_badge_wid(node.id),
                    "Cache in RAM — keep the output resident, reused across runs this session",
                )
                .show(ui, theme)
                {
                    out.push(Intent::SetNodeProperty {
                        node_id: node.id,
                        to: NodeProperty::Cache(CacheMode::from_bits(!ram, disk)),
                    });
                }
                if Badge::control(
                    "↓",
                    theme.colors.badge_cache,
                    disk,
                    disk_badge_wid(node.id),
                    "Cache to disk — persist the output across runs and reopens",
                )
                .show(ui, theme)
                {
                    out.push(Intent::SetNodeProperty {
                        node_id: node.id,
                        to: NodeProperty::Cache(CacheMode::from_bits(ram, !disk)),
                    });
                }
            }
        });
}

/// The node title: an inline-renamable label. Double-click swaps it for
/// a `TextEdit`; commit emits [`Intent::RenameNode`], single-click
/// selects (the label would otherwise swallow the body's click). Same
/// widget + style as the boundary-port rename in
/// [`crate::gui::node::port_rename`].
fn title(ui: &mut Ui, rcx: RecordCtx<'_>, node: &SceneNode, out: &mut Vec<Intent>) {
    let shift = ui.modifiers().shift;
    let id = node_rename_wid(node.id);
    let ev = InlineRename::new(id, node.name.clone())
        .theme(&rcx.theme.inline_rename)
        .max_chars(NODE_NAME_MAX_CHARS)
        .style(TextStyle {
            weight: FontWeight::Bold,
            ..ui.theme.text
        })
        .show(ui);
    if ev.clicked {
        click_intents(shift, rcx.scene, node.id, out);
    }
    if let Some(to) = ev.committed {
        out.push(Intent::RenameNode {
            node_id: node.id,
            to,
        });
    }
}

/// Stable id for a node's clickable enable/disable chip.
fn disable_badge_wid(node_id: NodeId) -> WidgetId {
    WidgetId::from_hash(("graph.node.disable_badge", node_id))
}

/// Stable id for a node's clickable RAM-cache chip.
fn ram_badge_wid(node_id: NodeId) -> WidgetId {
    WidgetId::from_hash(("graph.node.ram_badge", node_id))
}

/// Stable id for a node's clickable disk-cache chip.
fn disk_badge_wid(node_id: NodeId) -> WidgetId {
    WidgetId::from_hash(("graph.node.disk_badge", node_id))
}

/// Stable id for a subgraph node's clickable open-in-tab chip.
pub(crate) fn subgraph_badge_wid(node_id: NodeId) -> WidgetId {
    WidgetId::from_hash(("graph.node.subgraph_badge", node_id))
}

/// Which visual family a chip belongs to, plus the per-family data — split in
/// here so a control can't carry a (dead) marker salt nor a marker a
/// (meaningless) `filled`. The two read very differently so the user can tell a
/// toggle from a fact at a glance.
#[derive(Debug, Clone, Copy)]
enum BadgeKind {
    /// Interactive: a bordered square that lifts a faint fill on hover
    /// (pressable). `wid` makes it clickable; `filled` is its solid "on" swatch.
    /// Toggles (`C`/`D`) and actions (`S`/`i`).
    Control { wid: WidgetId, filled: bool },
    /// Read-only descriptor: a borderless, tinted pill with its glyph inked in
    /// its own color. Never clickable — `salt` just gives it a stable id for the
    /// tooltip. Markers (`T` terminal, `~` impure).
    Marker { salt: &'static str },
}

/// One header indicator chip; the two families render differently (see
/// [`BadgeKind`]). Build one with [`Badge::control`] / [`Badge::marker`], then
/// [`show`](Badge::show) — which returns whether a control was clicked this
/// frame (always `false` for a marker).
#[derive(Debug)]
struct Badge {
    glyph: &'static str,
    color: Color,
    tip: &'static str,
    kind: BadgeKind,
}

impl Badge {
    /// An interactive chip (`filled` = its "on" state; `wid` makes it clickable).
    fn control(
        glyph: &'static str,
        color: Color,
        filled: bool,
        wid: WidgetId,
        tip: &'static str,
    ) -> Self {
        Badge {
            glyph,
            color,
            tip,
            kind: BadgeKind::Control { wid, filled },
        }
    }

    /// A read-only descriptor pill. `salt` is its stable id for the tooltip.
    fn marker(salt: &'static str, glyph: &'static str, color: Color, tip: &'static str) -> Self {
        Badge {
            glyph,
            color,
            tip,
            kind: BadgeKind::Marker { salt },
        }
    }

    fn show(self, ui: &mut Ui, theme: &Theme) -> bool {
        let Badge {
            glyph,
            color,
            tip,
            kind,
        } = self;
        // Background + glyph ink + width diverge by family: a marker is a flat
        // tinted pill (glyph in its own color, hugging its glyph); a control is a
        // fixed square whose hollow form lifts a faint fill of its color on hover,
        // and whose filled form is a solid swatch with a dark (header-fill) glyph.
        let (background, glyph_color, width) = match kind {
            BadgeKind::Marker { .. } => (
                Background {
                    fill: color.with_alpha(CHIP_TINT_ALPHA).into(),
                    corners: Corners::all(BADGE_SIZE * 0.5),
                    ..Default::default()
                },
                color,
                Sizing::Hug,
            ),
            BadgeKind::Control { filled: true, .. } => (
                Background {
                    fill: color.into(),
                    corners: Corners::all(3.0),
                    ..Default::default()
                },
                theme.colors.header_fill,
                Sizing::Fixed(BADGE_SIZE),
            ),
            BadgeKind::Control { wid, filled: false } => {
                let mut bg = Background {
                    stroke: Stroke::solid(color, 1.0),
                    corners: Corners::all(3.0),
                    ..Default::default()
                };
                // Last-frame hover (`response_for`) lifts a faint fill so the
                // chip reads as pressable — the same trick the tab-strip chips use.
                if ui.response_for(wid).hovered {
                    bg.fill = color.with_alpha(CHIP_TINT_ALPHA).into();
                }
                (bg, color, Sizing::Fixed(BADGE_SIZE))
            }
        };
        let mut panel = Panel::zstack()
            .size((width, Sizing::Fixed(BADGE_SIZE)))
            .child_align(Align::CENTER)
            .background(background);
        // A marker hugs its glyph into a pill (horizontal padding) and senses
        // only `HOVER`, so a click falls through to select the node; a control
        // captures the click.
        panel = match kind {
            BadgeKind::Marker { salt } => panel
                .id_salt(salt)
                .sense(Sense::HOVER)
                .padding(Spacing::xy(5.0, 0.0)),
            BadgeKind::Control { wid, .. } => panel.id(wid).sense(Sense::CLICK),
        };
        let chip = panel.show(ui, |ui| {
            Text::new(glyph)
                .style(TextStyle {
                    color: glyph_color,
                    font_size_px: BADGE_FONT,
                    weight: FontWeight::Bold,
                    ..ui.theme.text
                })
                .show(ui);
        });
        // Take the owned snapshot + click result so the chip's `ui` borrow
        // ends before the tooltip records into `ui`.
        let snapshot = chip.response.snapshot();
        let clicked = chip.response.clicked();
        if !tip.is_empty() {
            // `tip` is `&'static str`, so it rides into the tooltip as a
            // borrowed `Cow` — no per-frame allocation.
            Tooltip::for_(&snapshot).text(tip).show(ui);
        }
        clicked
    }
}
