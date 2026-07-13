//! Node header bar: the title plus the node's indicator chips, split into two
//! visual families so a toggle can't be mistaken for a fact. **Controls** are
//! bordered, hover-lifting chips you act on — `S` subgraph-open, `D` disable,
//! `R`/`↓` cache, and the `i` inspect chip. **Markers** are flat tinted pills
//! that only describe the node — `■` sink and `~` impure. The markers ride
//! in the [`header`] band beside the title; the run-time label (left) and the
//! interactive controls (right) share the [`status_row`] below it. Drawn as
//! the top children of each node body by [`crate::gui::node::NodeUI`].

use aperture::{
    Align, Background, Color, Configure, Corners, FontFamily, FontWeight, Panel, Sense, Shape,
    Sizing, Spacing, Spinner, Stroke, Text, TextStyle, Tooltip, Ui, VAlign, WidgetId,
};
use glam::Vec2;
use scenarium::graph::{CacheMode, NodeId};

use crate::core::document::SelectionKey;
use crate::core::edit::intent::{Intent, NodeProperty};
use crate::gui::canvas::inspector::{InspectMode, inspect_badge_wid};
use crate::gui::node::port_color::event_color;
use crate::gui::node::port_row::{EVENT_TRIANGLE_RADIUS, PORT_HIT_SCALE};
use crate::gui::node::{RecordCtx, click_intents, exec_color, node_rename_wid};
use crate::gui::run_state::ExecStatus;
use crate::gui::scene::SceneNode;
use crate::gui::theme::Theme;
use crate::gui::widgets::inline_rename::InlineRename;
use crate::gui::widgets::support::{
    CARD_HEADER_PAD_X, CARD_HEADER_PAD_Y, header_background, hspacer,
};

/// Character cap for a node title in the inline rename editor.
const NODE_NAME_MAX_CHARS: usize = 32;

/// Side of a header indicator chip (px), and its glyph font size.
const BADGE_SIZE: f32 = 18.0;
const BADGE_FONT: f32 = 12.0;

/// Shared chip tint opacity: a marker's fill and a hollow control's hover-lift
/// both paint their color at this alpha, so the two families feel like one system.
const CHIP_TINT_ALPHA: f32 = 0.20;

/// Fill alpha of a toggled-on control chip (hover lifts it). Deliberately a
/// tint, not a solid swatch: cache/disable state is passive configuration, and
/// solid saturated fills are reserved for live status (the exec glows).
const CHIP_ON_ALPHA: f32 = 0.35;
const CHIP_ON_HOVER_ALPHA: f32 = 0.50;

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
pub(crate) fn subscription_pin(ui: &mut Ui, theme: &Theme, node: &SceneNode, hovered: bool) {
    let port = theme.port_size;
    let hit = port * PORT_HIT_SCALE;
    let inset = (hit - port) * 0.5;
    // Rotate the base (left-pointing) triangle +45° about its center so the
    // apex points up-left, aligned with the wire arriving from there. The
    // rotated points are passed straight to the SDF triangle primitive; the
    // layout box is unchanged (the glyph isn't clipped to the owner rect, so
    // the rotated apex may exceed it). The base triangle is inset by the
    // corner radius: the SDF rounds by *dilating* (`sdf - radius`), so the
    // rounded result grows back to the port box instead of past it.
    let r = EVENT_TRIANGLE_RADIUS;
    let c = Vec2::splat(hit * 0.5);
    let rot = Vec2::from_angle(std::f32::consts::FRAC_PI_4);
    let tf = |v: Vec2| c + rot.rotate(v - c);
    let pin = Panel::zstack()
        .id(subscription_glyph_wid(node.id))
        .position(node.pos - Vec2::splat(hit * 0.5))
        .size((Sizing::Fixed(hit), Sizing::Fixed(hit)))
        .sense(Sense::CLICK | Sense::DRAG)
        .show(ui, |ui| {
            ui.add_shape(
                Shape::triangle(
                    tf(Vec2::new(inset + port - r, inset + r)),
                    tf(Vec2::new(inset + port - r, inset + port - r)),
                    tf(Vec2::new(inset + r, inset + port * 0.5)),
                )
                .radius(r)
                .fill(event_color(theme, hovered)),
            );
        });
    Tooltip::on(&pin.response.snapshot())
        .text("Event subscription — drag to an emitter, or drop an event wire here")
        .show(ui);
}

/// Stable id for a node's event-subscription pin. Keyed on the node (a
/// subscription is whole-node, not per-port), so `CanvasGeometry` /
/// `SubscriptionUI` reconstruct it to poll the pin's geometry as a wire
/// drop target.
pub(crate) fn subscription_glyph_wid(node_id: NodeId) -> WidgetId {
    WidgetId::from_hash(("graph.node.subscription_glyph", node_id))
}

/// The header bar: the node title (left) and the descriptive cluster (right) —
/// the markers (`■`/`~`), then the inspect chip. A `FILL` spacer between them
/// pins the cluster to the right edge (the run-time label and the interactive
/// controls ride in [`status_row`] below). The sink nodes' event-
/// subscription pin is *not* drawn here — it records at canvas level, before the
/// node bodies, so it peeks out from behind the node's corner.
pub(crate) fn header(ui: &mut Ui, rcx: RecordCtx<'_>, node: &SceneNode, out: &mut Vec<Intent>) {
    let theme = rcx.theme;
    // The header sits inside the body's border stroke (the layout folds
    // the stroke width into the body's padding), so it must round to the
    // stroke's *inner* radius, not the card's outer `node_corner_radius` —
    // see `Theme::card_inner_radius`.
    let r = theme.card_inner_radius();
    Panel::hstack()
        .id_salt("header")
        .size((Sizing::FILL, Sizing::Hug))
        .padding(Spacing::xy(CARD_HEADER_PAD_X, CARD_HEADER_PAD_Y))
        .gap(4.0)
        .child_align(Align::v(VAlign::Center))
        .background(header_background(theme, r))
        .show(ui, |ui| {
            // The run affordance leads the band, ahead of the title — the
            // one control that *does* something with the node's output
            // rather than configuring it. Only on nodes that resolve as a
            // run seed.
            if node.runnable() {
                play_chip(ui, theme, node);
            }
            title(ui, rcx, node, out);
            // Splits the title (left) from the descriptive cluster
            // (right): the markers, then inspect.
            hspacer(ui, "header_spacer");
            // Read-only markers — what the node *is* (flat tinted pills, not
            // interactive, so they read as labels). They ride here beside the
            // title; the interactive controls stay in `status_row` below.
            if node.sink {
                Badge::marker(
                    "badge_sink",
                    "■",
                    theme.colors.badge_sink,
                    "Sink — no downstream consumers",
                )
                .show(ui);
            }
            if node.impure {
                Badge::marker(
                    "badge_impure",
                    "~",
                    theme.colors.badge_impure,
                    "Impure — recomputes every run, never cached",
                )
                .show(ui);
            }
            // Inspect toggle: filled (checked) when pinned, accent outline
            // when open, muted-grey outline (`text_muted`) when closed. The
            // click is consumed in `Inspectors::apply` via this chip's
            // deterministic id, so the returned flag is ignored here. Hidden on
            // boundary nodes — pure routing, no runtime values worth inspecting.
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
                .show(ui);
            }
        });
}

/// The strip under the header: the run-time label left-aligned, a `FILL`
/// spacer, then the interactive chips right-aligned — `S` subgraph-open, `D`
/// disable, `R`/`↓` cache. The controls group apart from the title's identity
/// (header above); the run-time reads as the row's status counterweight. The
/// disable chip always shows, so the row's height is reserved regardless.
pub(crate) fn status_row(ui: &mut Ui, rcx: RecordCtx<'_>, node: &SceneNode, out: &mut Vec<Intent>) {
    let theme = rcx.theme;
    Panel::hstack()
        .id_salt("status_row")
        .size((Sizing::FILL, Sizing::Hug))
        // Extra top padding sets the controls off from the header bar (the body
        // vstack has no gap between rows). Order: left, top, right, bottom.
        .padding(Spacing::new(8.0, 7.0, 8.0, 2.0))
        .gap(4.0)
        .child_align(Align::v(VAlign::Center))
        .show(ui, |ui| {
            // Last-run time leads the row, tied to the node's status color —
            // the final time once executed, or live elapsed-so-far while
            // running (`App::frame` repaints so it ticks). Mono/tabular so it
            // holds a column across a stack of nodes.
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
                        family: FontFamily::Mono,
                        ..ui.theme.text
                    })
                    .show(ui);
            }
            // Pushes the controls to the right edge, keeping the
            // run-time label pinned left.
            hspacer(ui, "ctrl_spacer");
            // Interactive controls: what you can *do* to the node. Bordered
            // chips that lift on hover.
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
                .show(ui);
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
            .show(ui);
            if disable_toggled {
                out.push(Intent::SetNodeProperty {
                    node_id: node.id,
                    to: NodeProperty::Disabled(!node.disabled),
                });
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
            // Shown only where caching can apply — see `SceneNode::cacheable`,
            // which folds out boundary/self-caching/output-less/impure nodes.
            // (An impure node still paints the `~` marker below to say why.)
            if node.cacheable {
                let ram = node.cache.caches_in_ram();
                let disk = node.cache.persists_to_disk();
                let ram_color = if ram {
                    theme.colors.badge_cache
                } else {
                    theme.colors.text_muted
                };
                if Badge::control(
                    "R",
                    ram_color,
                    ram,
                    ram_badge_wid(node.id),
                    "RuntimeCache in RAM — keep the output resident, reused across runs this session",
                )
                .show(ui)
                {
                    out.push(Intent::SetNodeProperty {
                        node_id: node.id,
                        to: NodeProperty::RuntimeCache(CacheMode::from_bits(!ram, disk)),
                    });
                }
                let disk_color = if disk {
                    theme.colors.badge_cache
                } else {
                    theme.colors.text_muted
                };
                if Badge::control(
                    "↓",
                    disk_color,
                    disk,
                    disk_badge_wid(node.id),
                    "RuntimeCache to disk — persist the output across runs and reopens",
                )
                .show(ui)
                {
                    out.push(Intent::SetNodeProperty {
                        node_id: node.id,
                        to: NodeProperty::RuntimeCache(CacheMode::from_bits(ram, !disk)),
                    });
                }
            }
        });
}

/// The header's play chip: run the graph up to this node and keep its
/// outputs for preview — the same command as the context menu's "Run to
/// this node". Control-family framing (bordered [`BADGE_SIZE`] square,
/// hover-lifted tint), but the glyph is the SDF play triangle rather than
/// a font glyph, echoing the ports' triangle vocabulary and staying
/// optically centered at any zoom. Quiet at rest — muted ink like the
/// other idle controls — and takes the palette's success green
/// (`exec_executed_glow`) on hover: "go", pointing at the outcome the
/// click delivers. The click is read at canvas level via
/// [`play_badge_wid`] and translated into the run command there (node
/// code never names `AppCommand`).
fn play_chip(ui: &mut Ui, theme: &Theme, node: &SceneNode) {
    let wid = play_badge_wid(node.id);
    // The whole chip — border, glyph, hover fill — swings to the "go"
    // glow together, so the hover-dependent color is picked out here
    // rather than by `Badge`'s fixed-color scheme.
    let hovered = ui.response_for(wid).hovered;
    let color = if hovered {
        theme.colors.exec_executed_glow
    } else {
        theme.colors.text_muted
    };
    Badge::control_drawn(
        draw_play_triangle,
        color,
        false,
        wid,
        "Run to this node — execute its upstream cone and keep the output for preview",
    )
    .show(ui);
}

/// Play triangle about the chip center, nudged right — a play mark's
/// visual center sits left of its bounding box's. Points are inset by
/// the rounding radius: the SDF rounds by dilating, so the glyph grows
/// back out to the intended extents.
fn draw_play_triangle(ui: &mut Ui, color: Color) {
    const R: f32 = 1.5;
    const HALF_W: f32 = 3.75;
    const HALF_H: f32 = 4.5;
    const NUDGE: f32 = 0.75;
    let c = Vec2::splat(BADGE_SIZE * 0.5);
    ui.add_shape(
        Shape::triangle(
            c + Vec2::new(NUDGE - HALF_W + R, R - HALF_H),
            c + Vec2::new(NUDGE - HALF_W + R, HALF_H - R),
            c + Vec2::new(NUDGE + HALF_W - R, 0.0),
        )
        .radius(R)
        .fill(color),
    );
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
        click_intents(shift, rcx.scene, SelectionKey::Node(node.id), out);
    }
    if let Some(to) = ev.committed {
        out.push(Intent::RenameNode {
            node_id: node.id,
            to,
        });
    }
}

/// Stable id for a node's clickable run-to-node play chip. `pub(crate)` so
/// the canvas-level scan ([`crate::gui::node::emit_play_clicks`]) can poll
/// the click from last frame's response.
pub(crate) fn play_badge_wid(node_id: NodeId) -> WidgetId {
    WidgetId::from_hash(("graph.node.play_badge", node_id))
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
    /// (pressable). `wid` makes it clickable; `filled` deepens the tint for
    /// the "on" state. Toggles (`C`/`D`) and actions (`S`/`i`).
    Control { wid: WidgetId, filled: bool },
    /// Read-only descriptor: a borderless, tinted pill with its glyph inked in
    /// its own color. Never clickable — `salt` just gives it a stable id for the
    /// tooltip. Markers (`■` sink, `~` impure).
    Marker { salt: &'static str },
}

/// A chip's icon: a bold font character (the common case) or a caller-
/// drawn vector glyph painted into the `BADGE_SIZE` box in the chip's
/// ink (the play triangle). A plain `fn` pointer keeps `Badge`
/// non-generic.
#[derive(Debug, Clone, Copy)]
enum BadgeGlyph {
    Char(&'static str),
    Drawn(fn(&mut Ui, Color)),
}

/// One header indicator chip; the two families render differently (see
/// [`BadgeKind`]). Build one with [`Badge::control`] /
/// [`Badge::control_drawn`] / [`Badge::marker`], then
/// [`show`](Badge::show) — which returns whether a control was clicked this
/// frame (always `false` for a marker).
#[derive(Debug)]
struct Badge {
    glyph: BadgeGlyph,
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
            glyph: BadgeGlyph::Char(glyph),
            color,
            tip,
            kind: BadgeKind::Control { wid, filled },
        }
    }

    /// [`Self::control`] with a vector glyph instead of a font character.
    fn control_drawn(
        draw: fn(&mut Ui, Color),
        color: Color,
        filled: bool,
        wid: WidgetId,
        tip: &'static str,
    ) -> Self {
        Badge {
            glyph: BadgeGlyph::Drawn(draw),
            color,
            tip,
            kind: BadgeKind::Control { wid, filled },
        }
    }

    /// A read-only descriptor pill. `salt` is its stable id for the tooltip.
    fn marker(salt: &'static str, glyph: &'static str, color: Color, tip: &'static str) -> Self {
        Badge {
            glyph: BadgeGlyph::Char(glyph),
            color,
            tip,
            kind: BadgeKind::Marker { salt },
        }
    }

    fn show(self, ui: &mut Ui) -> bool {
        let Badge {
            glyph,
            color,
            tip,
            kind,
        } = self;
        // Background + width diverge by family — the glyph always inks in the
        // chip's own color. A marker is a flat tinted pill hugging its glyph; a
        // control is a bordered square whose "on" state deepens the tint (never
        // a solid swatch — that weight belongs to live status, not config).
        let (background, width) = match kind {
            BadgeKind::Marker { .. } => (
                Background::rounded(
                    color.with_alpha(CHIP_TINT_ALPHA),
                    Corners::all(BADGE_SIZE * 0.5),
                ),
                Sizing::Hug,
            ),
            BadgeKind::Control { wid, filled } => {
                let mut bg = Background {
                    stroke: Stroke::solid(color, 1.0),
                    corners: Corners::all(3.0),
                    ..Default::default()
                };
                // Last-frame hover (`response_for`) lifts the fill so the chip
                // reads as pressable — the same trick the tab-strip chips use.
                let hovered = ui.response_for(wid).hovered;
                let alpha = match (filled, hovered) {
                    (true, true) => CHIP_ON_HOVER_ALPHA,
                    (true, false) => CHIP_ON_ALPHA,
                    (false, true) => CHIP_TINT_ALPHA,
                    (false, false) => 0.0,
                };
                if alpha > 0.0 {
                    bg.fill = color.with_alpha(alpha).into();
                }
                (bg, Sizing::Fixed(BADGE_SIZE))
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
        let chip = panel.show(ui, |ui| match glyph {
            BadgeGlyph::Char(glyph) => {
                Text::new(glyph)
                    .style(TextStyle {
                        color,
                        font_size_px: BADGE_FONT,
                        weight: FontWeight::Bold,
                        ..ui.theme.text
                    })
                    .show(ui);
            }
            BadgeGlyph::Drawn(draw) => draw(ui, color),
        });
        // Take the owned snapshot + click result so the chip's `ui` borrow
        // ends before the tooltip records into `ui`.
        let snapshot = chip.response.snapshot();
        let clicked = chip.response.clicked();
        if !tip.is_empty() {
            // `tip` is `&'static str`, so it rides into the tooltip as a
            // borrowed `Cow` — no per-frame allocation.
            Tooltip::on(&snapshot).text(tip).show(ui);
        }
        clicked
    }
}
