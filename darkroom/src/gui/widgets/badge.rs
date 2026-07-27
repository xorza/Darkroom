//! The indicator chip shared by the node headers and the pin cards: a small
//! square or pill carrying one glyph, in one of two visual families so a
//! toggle can't be mistaken for a fact.
//!
//! **Controls** are bordered, hover-lifting chips you act on; **markers** are
//! flat tinted pills that only describe what something *is*. Domain-agnostic
//! like the rest of `widgets` — a caller supplies the glyph, colour, id, and
//! tooltip, and maps the returned click onto its own intent.

use palantir::{
    Align, Background, Color, Configure, Corners, FontWeight, Panel, Sense, Sizing, Spacing,
    Stroke, Text, TextStyle, Ui, WidgetId,
};

use crate::gui::theme::Theme;
use crate::gui::widgets::support::tooltip_after;

/// Side of an indicator chip (px), and its glyph font size. Public because a
/// caller drawing a vector glyph ([`Badge::go`]) paints into this box, and the
/// node header sizes its run-time label and spinner to match.
pub(crate) const BADGE_SIZE: f32 = 18.0;
pub(crate) const BADGE_FONT: f32 = 12.0;

/// Shared chip tint opacity: a marker's fill and a hollow control's hover-lift
/// both paint their color at this alpha, so the two families feel like one system.
const CHIP_TINT_ALPHA: f32 = 0.20;

/// Fill alpha of a toggled-on control chip (hover lifts it). Deliberately a
/// tint, not a solid swatch: cache/disable state is passive configuration, and
/// solid saturated fills are reserved for live status (the exec glows).
const CHIP_ON_ALPHA: f32 = 0.35;
const CHIP_ON_HOVER_ALPHA: f32 = 0.50;

/// Which visual family a chip belongs to, plus the per-family data — split in
/// here so a control can't carry a (dead) marker salt nor a marker a
/// (meaningless) `filled`. The two read very differently so the user can tell a
/// toggle from a fact at a glance.
#[derive(Debug, Clone, Copy)]
enum BadgeKind {
    /// Interactive: a bordered square that lifts a faint fill on hover
    /// (pressable). `wid` makes it clickable; `filled` deepens the tint for
    /// the "on" state. Toggles (`C`/`D`) and actions (`G`/`i`).
    Control { wid: WidgetId, filled: bool },
    /// Read-only descriptor: a borderless, tinted pill with its glyph inked in
    /// its own color. Never clickable — `salt` just gives it a stable id for the
    /// tooltip. Markers (`■` sink, `~` impure).
    Marker { salt: &'static str },
}

/// A "go" chip awaiting the theme it resolves its hover colour from — see
/// [`Badge::go`]. Built by the call site (which knows the id, tip, and
/// glyph), coloured and recorded by [`Self::show`].
#[derive(Debug)]
pub(crate) struct GoBadge {
    wid: WidgetId,
    tip: &'static str,
    draw: fn(&mut Ui, Color),
}

impl GoBadge {
    pub(crate) fn show(self, ui: &mut Ui, theme: &Theme) {
        // Last frame's hover, like every other chip's fill lift.
        let color = if ui.response_for(self.wid).hovered {
            theme.colors.exec_executed_glow
        } else {
            theme.colors.text_muted
        };
        Badge {
            glyph: BadgeGlyph::Drawn(self.draw),
            color,
            tip: self.tip,
            kind: BadgeKind::Control {
                wid: self.wid,
                filled: false,
            },
        }
        .show(ui);
    }
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
/// frame (always `false` for a marker). `pub(crate)` so
/// [`crate::gui::canvas::pin_preview`]'s refresh chip can reuse the same
/// framing rather than re-deriving the chip look.
#[derive(Debug)]
pub(crate) struct Badge {
    glyph: BadgeGlyph,
    color: Color,
    tip: &'static str,
    kind: BadgeKind,
}

impl Badge {
    /// An interactive chip (`filled` = its "on" state; `wid` makes it clickable).
    pub(crate) fn control(
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

    /// A "go" chip: a vector-glyph control that runs something rather than
    /// configuring it — a node header's play chip and a pin card's refresh
    /// chip. Quiet at rest (muted ink like every other idle control) and
    /// swinging whole — border, glyph, hover fill — to the palette's success
    /// green when hovered, pointing at the outcome the click delivers.
    ///
    /// The colour is resolved by [`Self::show_go`] rather than passed in,
    /// because it depends on last frame's hover: written out at each call
    /// site, the two chips were the same four lines twice.
    pub(crate) fn go(wid: WidgetId, tip: &'static str, draw: fn(&mut Ui, Color)) -> GoBadge {
        GoBadge { wid, tip, draw }
    }

    /// A read-only descriptor pill. `salt` is its stable id for the tooltip.
    #[allow(clippy::self_named_constructors)]
    pub(crate) fn marker(
        salt: &'static str,
        glyph: &'static str,
        color: Color,
        tip: &'static str,
    ) -> Self {
        Badge {
            glyph: BadgeGlyph::Char(glyph),
            color,
            tip,
            kind: BadgeKind::Marker { salt },
        }
    }

    pub(crate) fn show(self, ui: &mut Ui) -> bool {
        let Badge {
            glyph,
            color,
            tip,
            kind,
        } = self;
        // The two families diverge on every axis but the glyph's ink, so one
        // match settles all of them: a marker is a flat tinted pill hugging
        // its glyph, sensing only `HOVER` so a click falls through to select
        // the node; a control is a fixed bordered square that captures the
        // click and whose "on" state deepens the tint (never a solid swatch —
        // that weight belongs to live status, not config).
        let panel = Panel::zstack()
            .size((
                match kind {
                    BadgeKind::Marker { .. } => Sizing::HUG,
                    BadgeKind::Control { .. } => Sizing::fixed(BADGE_SIZE),
                },
                Sizing::fixed(BADGE_SIZE),
            ))
            .child_align(Align::CENTER);
        let panel = match kind {
            BadgeKind::Marker { salt } => panel
                .background(Background::rounded(
                    color.with_alpha(CHIP_TINT_ALPHA),
                    Corners::all(BADGE_SIZE * 0.5),
                ))
                .id_salt(salt)
                .sense(Sense::HOVER)
                .padding(Spacing::xy(5.0, 0.0)),
            BadgeKind::Control { wid, filled } => {
                // Last-frame hover (`response_for`) lifts the fill so the chip
                // reads as pressable — the same trick the tab-strip chips use.
                let alpha = match (filled, ui.response_for(wid).hovered) {
                    (true, true) => CHIP_ON_HOVER_ALPHA,
                    (true, false) => CHIP_ON_ALPHA,
                    (false, true) => CHIP_TINT_ALPHA,
                    (false, false) => 0.0,
                };
                let mut background = Background {
                    stroke: Stroke::solid(color, 1.0),
                    corners: Corners::all(3.0),
                    ..Default::default()
                };
                // Left at the default (no fill at all) rather than a
                // fully-transparent one, so a resting chip paints no quad.
                if alpha > 0.0 {
                    background.fill = color.with_alpha(alpha).into();
                }
                panel.background(background).id(wid).sense(Sense::CLICK)
            }
        };
        let chip = panel.show(ui, |ui| match glyph {
            BadgeGlyph::Char(glyph) => {
                Text::new(glyph)
                    .style(&TextStyle {
                        color,
                        font_size_px: BADGE_FONT,
                        weight: FontWeight::Bold,
                        ..ui.theme.text.clone()
                    })
                    .show(ui);
            }
            BadgeGlyph::Drawn(draw) => draw(ui, color),
        });
        // Take the owned snapshot + click result so the chip's `ui` borrow
        // ends before the tooltip records into `ui`.
        let snapshot = chip.response.snapshot();
        let clicked = chip.response.left.clicked();
        tooltip_after(ui, &snapshot, tip);
        clicked
    }
}
