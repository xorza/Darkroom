//! Low-level glyph-drawing primitives for a port row: the circle a data
//! port paints as, the triangle an emitter event paints as, and the shared
//! hit-box-growth math both ride on. None of these take any domain context
//! (`DrawCtx`/`GraphCtx`) — they're pure "draw this shape in this box"
//! helpers, unlike [`super`], which is grid orchestration and per-cell
//! rendering.

use glam::Vec2;
use palantir::{Color, Configure, Panel, Rect, Sense, Shape, Sizing, Spacing, Ui, WidgetId};

use crate::gui::theme::Theme;
use crate::gui::widgets::support::{filled_rect, stroked_rect, tooltip_after};

/// Hover / grab box scaled past the painted glyph so ports, event
/// triangles, and subscription pins are easier to hit and snap to,
/// while the visible shape stays `port_size`. The enlarged box is also
/// what keeps the wire hover-highlight repaint-correct: the glyph's own
/// (hover-target) box carries the emphasis zone, so entering/leaving it
/// is a hover-target change and repaints without any pointer
/// subscription.
pub(crate) const PORT_HIT_SCALE: f32 = 1.8;

/// Corner rounding of the event triangles (emitter glyph + subscription
/// pin), matching the soft corners of the rest of the chrome.
pub(crate) const EVENT_TRIANGLE_RADIUS: f32 = 2.0;

/// Stroke width of the muted ring drawn around a non-required input's port
/// circle (see `circle_frame`'s `outline` param). Also the amount a
/// required input's plain circle grows by (on each side), so a required
/// input's total footprint matches that ring — "important port" reads as
/// one consistent size regardless of which visual (ring vs. bigger fill)
/// carries it.
const PORT_OUTLINE_WIDTH: f32 = 2.5;

/// A port circle's diameter — `base` for a plain port, or `base` grown by
/// [`PORT_OUTLINE_WIDTH`] on each side to match a non-required input's
/// circle-plus-ring footprint (a required input, via [`circle_frame`]'s
/// `diameter`).
pub(super) fn port_diameter(base: f32, enlarged: bool) -> f32 {
    if enlarged {
        base + 2.0 * PORT_OUTLINE_WIDTH
    } else {
        base
    }
}

/// The frame both port glyphs paint into: a `PORT_HIT_SCALE`-grown sensing
/// box with the growth folded back out of `margin`, `draw` painting the
/// glyph into the `base`-sized square at `inset`, and the tooltip recorded
/// after the panel's borrow ends.
///
/// Explicit `id(wid)` so the cross-frame id stays stable: the prepass
/// computes the same id and reads its response, the record paints with it —
/// no drift even if the parent structure shifts. `CLICK | DRAG` so the glyph
/// (a) intercepts the press before it falls through to the node body's
/// `Sense::DRAG`, and (b) can latch a wire drag.
fn glyph_frame(
    ui: &mut Ui,
    wid: WidgetId,
    base: f32,
    margin: Spacing,
    tip: &str,
    draw: impl FnOnce(&mut Ui, f32),
) {
    let GrownHitBox {
        hit,
        inset,
        margin: hit_margin,
    } = grown_hit_box(base, margin);
    let glyph = Panel::zstack()
        .id(wid)
        .size((Sizing::fixed(hit), Sizing::fixed(hit)))
        .margin(hit_margin)
        .sense(Sense::CLICK | Sense::DRAG)
        .show(ui, |ui| draw(ui, inset));
    let snapshot = glyph.response.snapshot();
    tooltip_after(ui, &snapshot, tip.to_owned());
}

pub(super) fn circle_frame(
    ui: &mut Ui,
    wid: WidgetId,
    diameter: f32,
    fill: Color,
    outline: Option<Color>,
    margin: Spacing,
    tip: &str,
) {
    let radius = diameter * 0.5;
    glyph_frame(ui, wid, diameter, margin, tip, |ui, inset| {
        let rect = Rect::new(inset, inset, diameter, diameter);
        // The ring paints *before* the fill — an annulus strictly outside
        // the fill's radius, so they don't overlap either way. A stroke
        // paints its own rect's *inner*-edge annulus, so drawing it on
        // `rect` would eat into the fill; inflating first lands the ring's
        // inner edge exactly on the fill's outer edge.
        if let Some(color) = outline {
            stroked_rect(
                ui,
                rect.inflated(PORT_OUTLINE_WIDTH),
                radius + PORT_OUTLINE_WIDTH,
                color,
                PORT_OUTLINE_WIDTH,
            );
        }
        filled_rect(ui, rect, radius, fill);
    });
}

/// Paints an event port glyph: a right-pointing triangle (a port dot rotated
/// 90°), the same `port_size` box and edge overhang as a data port's circle,
/// so it lines up with the outputs above it. `fill` carries the hover state;
/// `tip` shows as a hover tooltip. Senses `CLICK | DRAG` so a subscription
/// wire can be dragged out of it. Like `circle_frame`, the sensing box is
/// `PORT_HIT_SCALE`-grown with the extra pulled back out of layout via
/// negative margin, so the triangle stays put while hover/grab (and the
/// wire hover-highlight zone) get generous.
pub(super) fn event_glyph(
    ui: &mut Ui,
    theme: &Theme,
    wid: WidgetId,
    fill: Color,
    margin: Spacing,
    tip: &str,
) {
    let port = theme.port_size;
    glyph_frame(ui, wid, port, margin, tip, |ui, inset| {
        // Right-pointing isosceles triangle filling the port box: the apex
        // points outward (away from the node body), matching the emit
        // direction. Vertices are inset by the corner radius — the SDF
        // rounds by *dilating* (`sdf - radius`), so the rounded result grows
        // back out to the port box instead of past it.
        let r = EVENT_TRIANGLE_RADIUS;
        ui.add_shape(
            Shape::triangle(
                Vec2::new(inset + r, inset + r),
                Vec2::new(inset + r, inset + port - r),
                Vec2::new(inset + port - r, inset + port * 0.5),
            )
            .radius(r)
            .fill(fill),
        );
    });
}

/// A glyph's `PORT_HIT_SCALE`-grown sensing box, from [`grown_hit_box`].
#[derive(Debug)]
struct GrownHitBox {
    /// The grown box side.
    hit: f32,
    /// Half the growth — the glyph's paint offset within the box.
    inset: f32,
    /// The caller's margin with the growth folded back out.
    margin: Spacing,
}

/// Grows `base` into a `PORT_HIT_SCALE`-larger sensing box and folds that
/// growth back out of `margin` (as a negative adjustment) so the extra hit
/// area doesn't displace the painted glyph — node layout and the glyph's
/// own position are unchanged, only the hover/grab area grows. Shared by
/// port circles ([`circle_frame`]) and event triangles ([`event_glyph`]).
fn grown_hit_box(base: f32, margin: Spacing) -> GrownHitBox {
    let hit = base * PORT_HIT_SCALE;
    let inset = (hit - base) * 0.5;
    let [l, t, r, b] = margin.as_array();
    GrownHitBox {
        hit,
        inset,
        margin: Spacing::new(l - inset, t - inset, r - inset, b - inset),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn port_diameter_enlarges_by_the_outline_width_on_each_side() {
        let base = 10.0;
        assert_eq!(port_diameter(base, false), base, "plain port is unchanged");
        assert_eq!(
            port_diameter(base, true),
            base + 2.0 * PORT_OUTLINE_WIDTH,
            "enlarged port matches an optional input's circle-plus-ring footprint"
        );
    }
}
