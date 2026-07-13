//! Low-level glyph-drawing primitives for a port row: the circle a data
//! port paints as, the triangle an emitter event paints as, and the shared
//! hit-box-growth math both ride on. None of these take any domain context
//! (`RecordCtx`/`Scene`) — they're pure "draw this shape in this box"
//! helpers, unlike [`super`], which is grid orchestration and per-cell
//! rendering.

use aperture::{Color, Configure, Panel, Rect, Sense, Shape, Sizing, Spacing, Ui, WidgetId};
use glam::Vec2;

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
pub(crate) fn port_diameter(base: f32, enlarged: bool) -> f32 {
    if enlarged {
        base + 2.0 * PORT_OUTLINE_WIDTH
    } else {
        base
    }
}

/// A port circle's extra decoration — currently just an input's muted ring
/// (an output's pinned satellite is a canvas-level decoration instead — see
/// `crate::gui::canvas::pin_ui`). A flag rather than a bare `Option<Color>`
/// so a future second decoration doesn't need restructuring.
#[derive(Debug)]
pub(crate) enum PortDecoration {
    None,
    Outline(Color),
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn circle_frame(
    ui: &mut Ui,
    wid: WidgetId,
    diameter: f32,
    fill: Color,
    decoration: PortDecoration,
    margin: Spacing,
    tip: &str,
) {
    let port = diameter;
    let GrownHitBox {
        hit,
        inset,
        margin: hit_margin,
    } = grown_hit_box(port, margin);
    let radius = port * 0.5;

    // Explicit `id(wid)` so the cross-frame id stays stable: prepass
    // computes the same `port_circle_wid` and reads its response,
    // record paints with the same id — no drift even if the parent
    // structure shifts. CLICK | DRAG so the port (a) intercepts the
    // press before it falls through to the node body's `Sense::DRAG`,
    // and (b) can latch a connection drag.
    let circle = Panel::zstack()
        .id(wid)
        .size((Sizing::Fixed(hit), Sizing::Fixed(hit)))
        .margin(hit_margin)
        .sense(Sense::CLICK | Sense::DRAG)
        .show(ui, |ui| {
            let rect = Rect::new(inset, inset, port, port);
            // Decoration paints *before* the fill: the ring (an annulus
            // strictly outside the fill's radius) doesn't overlap it either
            // way.
            match decoration {
                PortDecoration::None => {}
                PortDecoration::Outline(color) => {
                    // A stroke paints its own rect's *inner*-edge annulus, so
                    // drawing it on `rect` itself would eat into the fill.
                    // Inflate first: the ring's inner edge then lands exactly
                    // on the fill's outer edge instead of inside it.
                    stroked_rect(
                        ui,
                        rect.inflated(PORT_OUTLINE_WIDTH),
                        radius + PORT_OUTLINE_WIDTH,
                        color,
                        PORT_OUTLINE_WIDTH,
                    );
                }
            }
            filled_rect(ui, rect, radius, fill);
        });
    let snapshot = circle.response.snapshot();
    tooltip_after(ui, &snapshot, tip.to_owned());
}

/// Paints an event port glyph: a right-pointing triangle (a port dot rotated
/// 90°), the same `port_size` box and edge overhang as a data port's circle,
/// so it lines up with the outputs above it. `fill` carries the hover state;
/// `tip` shows as a hover tooltip. Senses `CLICK | DRAG` so a subscription
/// wire can be dragged out of it. Like `circle_frame`, the sensing box is
/// `PORT_HIT_SCALE`-grown with the extra pulled back out of layout via
/// negative margin, so the triangle stays put while hover/grab (and the
/// wire hover-highlight zone) get generous.
pub(crate) fn event_glyph(
    ui: &mut Ui,
    theme: &Theme,
    wid: WidgetId,
    fill: Color,
    margin: Spacing,
    tip: &str,
) {
    let port = theme.port_size;
    let GrownHitBox {
        hit,
        inset,
        margin: hit_margin,
    } = grown_hit_box(port, margin);
    let glyph = Panel::zstack()
        .id(wid)
        .size((Sizing::Fixed(hit), Sizing::Fixed(hit)))
        .margin(hit_margin)
        .sense(Sense::CLICK | Sense::DRAG)
        .show(ui, |ui| {
            // Right-pointing isosceles triangle filling the port box (offset
            // by `inset` to center in the grown hit box): the apex points
            // outward (away from the node body), matching the emit
            // direction. SDF-antialiased via the triangle primitive. Vertices
            // are inset by the corner radius: the SDF rounds by *dilating*
            // (`sdf - radius`), so the rounded result grows back out to the
            // port box instead of past it.
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
    let snapshot = glyph.response.snapshot();
    tooltip_after(ui, &snapshot, tip.to_owned());
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
