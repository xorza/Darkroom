//! The small sensing glyph a wire terminates on: a filled circle or a
//! rounded triangle in a generously grown hit box, with a hover tooltip.

use common::LibmVec2;
use glam::Vec2;
use palantir::widget::Shape;
use palantir::{
    Border, Configure, Panel, Rect, Response, RgbaF32, Sense, Spacing, TextInput, Ui, Visibility,
    WidgetId,
};

use crate::gui::theme::Theme;
use crate::gui::widgets::support;

/// Hover / grab box scaled past the painted glyph so ports, event triangles,
/// and subscription pins are easier to hit and snap to, while the visible
/// shape stays its own size. The enlarged box is also what keeps the wire
/// hover-highlight repaint-correct: the glyph's own (hover-target) box carries
/// the emphasis zone, so entering/leaving it is a hover-target change and
/// repaints without any pointer subscription.
const HIT_SCALE: f32 = 1.8;

/// What a glyph paints inside its box.
#[derive(Clone, Copy, Debug)]
enum GlyphShape {
    /// A disc, optionally ringed by an annulus strictly outside it; a hollow one is a stroke
    /// ring in the ink with nothing inside.
    Circle {
        outline: Option<RgbaF32>,
        hollow: bool,
    },
    /// An isosceles triangle whose apex points right before `turn` rotates it
    /// about the box center.
    Arrow { turn: f32 },
}

/// The painted glyph: its shape, the side of the square it paints in, and its
/// ink.
#[derive(Clone, Copy, Debug)]
struct Painted {
    shape: GlyphShape,
    /// Side of the *painted* shape; the sensing box is [`HIT_SCALE`] of this.
    size: f32,
    fill: RgbaF32,
}

/// Where the grown box sits.
#[derive(Clone, Copy, Debug)]
enum Placement {
    /// In flow, with the caller's margin — the growth folded back out of it so
    /// the extra hit area doesn't displace the painted glyph.
    Margin(Spacing),
    /// Out of flow, the *grown* box centered on a point in the parent's space.
    CenteredOn(Vec2),
}

/// One port-terminal glyph: the circle a data port paints as, the triangle an
/// emitter event paints as, or the rotated triangle a subscription pin paints
/// as. Builder chain ending in [`show`](Self::show), like a palantir widget.
///
/// Domain-agnostic like the rest of `widgets` — the caller supplies the id,
/// size, colors and tooltip, and maps the returned clicks onto its own
/// intent. What the glyph owns is the part every caller was repeating: the
/// [`HIT_SCALE`]-grown sensing box, `CLICK | DRAG` so it intercepts the press
/// before the node body under it and can latch a wire drag, and the tooltip
/// recorded after it.
///
/// The explicit `wid` is not optional: these ids are reconstructed from domain
/// coordinates by passes that never record the glyph (the geometry rebuild,
/// the snap scans), so they must not drift with the parent structure.
#[derive(Debug)]
pub(crate) struct PortGlyph<'a> {
    wid: WidgetId,
    glyph: Painted,
    placement: Placement,
    visibility: Visibility,
    tooltip: Option<TextInput<'a>>,
}

impl<'a> PortGlyph<'a> {
    /// A data port's circle, `diameter` across, inked `fill`.
    pub(crate) const fn circle(wid: WidgetId, diameter: f32, fill: RgbaF32) -> Self {
        Self::new(
            wid,
            Painted {
                shape: GlyphShape::Circle {
                    outline: None,
                    hollow: false,
                },
                size: diameter,
                fill,
            },
        )
    }

    /// An emitter event's triangle, apex pointing right (the emit direction),
    /// inked `fill`.
    pub(crate) const fn arrow(wid: WidgetId, size: f32, fill: RgbaF32) -> Self {
        Self::new(
            wid,
            Painted {
                shape: GlyphShape::Arrow { turn: 0.0 },
                size,
                fill,
            },
        )
    }

    const fn new(wid: WidgetId, glyph: Painted) -> Self {
        Self {
            wid,
            glyph,
            placement: Placement::Margin(Spacing::ZERO),
            visibility: Visibility::Visible,
            tooltip: None,
        }
    }

    /// Ring a circle with an annulus strictly outside its fill. Ignored by
    /// [`arrow`](Self::arrow). Default: no ring.
    pub(crate) const fn outline(mut self, color: RgbaF32) -> Self {
        if let GlyphShape::Circle { outline, .. } = &mut self.glyph.shape {
            *outline = Some(color);
        }
        self
    }

    /// Draw a circle as a stroke ring with nothing inside: an input that takes any type, told
    /// apart by shape where a typed port is told apart by hue. Ignored by
    /// [`arrow`](Self::arrow). Default: filled.
    pub(crate) const fn hollow(mut self) -> Self {
        if let GlyphShape::Circle { hollow, .. } = &mut self.glyph.shape {
            *hollow = true;
        }
        self
    }

    /// Rotate an arrow's apex by `radians` about the box center. Ignored by
    /// [`circle`](Self::circle). Default: none (apex points right).
    pub(crate) const fn turn(mut self, radians: f32) -> Self {
        if let GlyphShape::Arrow { turn } = &mut self.glyph.shape {
            *turn = radians;
        }
        self
    }

    /// Lay the glyph out in flow with `margin`. The hit-box growth is folded
    /// back out of it, so node layout and the glyph's own position are
    /// unchanged and only the hover/grab area grows. Default: no margin.
    pub(crate) const fn margin(mut self, margin: Spacing) -> Self {
        self.placement = Placement::Margin(margin);
        self
    }

    /// Place the glyph out of flow, its *grown* box centered on `point` in the
    /// parent's coordinate space — for one that hangs off a corner rather than
    /// sitting in a row.
    pub(crate) const fn centered_on(mut self, point: Vec2) -> Self {
        self.placement = Placement::CenteredOn(point);
        self
    }

    /// Whether the glyph shows; hidden, it is still laid out, so its rect stays known.
    pub(crate) const fn visibility(mut self, visibility: Visibility) -> Self {
        self.visibility = visibility;
        self
    }

    /// Hover tooltip. `None` (the default) records none — which is what a
    /// port off the hovered node passes, since only that node builds tooltips.
    /// A `&'static str` stays borrowed until the bubble records; an
    /// [`InternedStr`](palantir::InternedStr) from [`fmt!`](palantir::fmt) is
    /// already in the record pass's text arena.
    pub(crate) fn tooltip(mut self, tooltip: Option<impl Into<TextInput<'a>>>) -> Self {
        self.tooltip = tooltip.map(Into::into);
        self
    }

    /// Record the glyph and answer its response: a left double-click is the binding toggle on a
    /// port circle, and a right-click its menu. The glyph senses its own `CLICK` and takes the
    /// hits over its rect, so a cell wrapping one reads its right-click too: one landed on the
    /// glyph never reaches the cell. A ring, an [`outline`](Self::outline) or a
    /// [`hollow`](Self::hollow) circle's, strokes at the theme's `stroke.line`, and a triangle
    /// rounds at `radius.mark`.
    pub(crate) fn show(self, ui: &mut Ui, theme: &Theme) -> Response {
        let Self {
            wid,
            glyph,
            placement,
            visibility,
            tooltip,
        } = self;
        let (hit, inset) = (glyph.hit(), glyph.inset());
        let panel = Panel::zstack()
            .id(wid)
            .visibility(visibility)
            .size(hit)
            .sense(Sense::CLICK | Sense::DRAG);
        let panel = match placement {
            Placement::Margin(margin) => {
                let [l, t, r, b] = margin.to_array();
                panel.margin(Spacing::new(l - inset, t - inset, r - inset, b - inset))
            }
            Placement::CenteredOn(point) => panel.at(point - Vec2::splat(hit * 0.5)),
        };
        let shown = panel.show(ui, |ui| {
            glyph.draw(ui, theme.stroke.line, theme.radius.mark);
        });
        support::tooltip_after(ui, &shown.response, tooltip);
        shown.response
    }
}

/// A circle's diameter grown by a `ring` width on each side, so a plain circle's footprint
/// matches an [`outline`](PortGlyph::outline)d one: "important port" reads as one size whichever
/// visual carries it.
pub(crate) const fn enlarged_diameter(base: f32, ring: f32) -> f32 {
    base + 2.0 * ring
}

impl Painted {
    /// Side of the sensing box.
    const fn hit(self) -> f32 {
        self.size * HIT_SCALE
    }

    /// Half the growth — the painted glyph's offset within the grown box.
    const fn inset(self) -> f32 {
        (self.hit() - self.size) * 0.5
    }

    /// An arrow's corners in the [`Self::hit`]-sided box, its apex last, pointing right and
    /// turned by `turn` about the box center. Rounding keeps a triangle's edges, so the rounded
    /// arrow stays inside the glyph box. The layout box is unchanged by the turn: the glyph is
    /// not clipped to the owner rect, so a turned apex may exceed it.
    fn arrow_vertices(self, turn: f32) -> [Vec2; 3] {
        let (size, inset) = (self.size, self.inset());
        let center = Vec2::splat(self.hit() * 0.5);
        let rot = Vec2::libm_from_angle(turn);
        [
            Vec2::new(inset, inset),
            Vec2::new(inset, inset + size),
            Vec2::new(inset + size, inset + size * 0.5),
        ]
        .map(|v| center + rot.rotate(v - center))
    }

    /// Paint into the `size`-sided square at [`Self::inset`] within the
    /// [`Self::hit`]-sided box, a ring at `ring` wide and a triangle's corners at `corner`.
    fn draw(self, ui: &mut Ui, ring: f32, corner: f32) {
        let Self { shape, size, fill } = self;
        let inset = self.inset();
        match shape {
            GlyphShape::Circle { outline, hollow } => {
                let rect = Rect::new(inset, inset, size, size);
                let radius = size * 0.5;
                // The ring paints *before* the fill — an annulus strictly
                // outside the fill's radius, so they don't overlap either way.
                // A stroke paints its own rect's *inner*-edge annulus, so
                // drawing it on `rect` would eat into the fill; inflating first
                // lands the ring's inner edge exactly on the fill's outer edge.
                if let Some(color) = outline {
                    ui.add_shape(
                        Shape::rect().at(rect.inflated(ring)).corners(radius + ring),
                        Border::new(color, ring),
                    );
                }
                let disc = Shape::rect().at(rect).corners(radius);
                if hollow {
                    ui.add_shape(disc, Border::new(fill, ring));
                } else {
                    ui.add_shape(disc, fill);
                }
            }
            GlyphShape::Arrow { turn } => {
                let [a, b, apex] = self.arrow_vertices(turn);
                ui.add_shape(Shape::triangle(a, b, apex).radius(corner), fill);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::f32::consts::PI;

    #[test]
    fn enlarged_diameter_grows_by_the_outline_width_on_each_side() {
        // 10 + 2 * 2: the ring's width on both sides.
        assert_eq!(
            enlarged_diameter(10.0, 2.0),
            14.0,
            "enlarged port matches an outlined circle's footprint"
        );
    }

    /// A 10-sided arrow sits in its 18-sided box (`HIT_SCALE` 1.8) at inset 4: (4, 4), (4, 14),
    /// and the apex at (14, 9), right of the center (9, 9). A half turn mirrors the apex to
    /// (4, 9) across the center. Within 1e-5 there:
    /// `sin(PI)` in f32 is -8.7e-8, not 0.
    #[test]
    fn an_arrow_points_right_and_a_half_turn_mirrors_it() {
        let arrow = Painted {
            shape: GlyphShape::Arrow { turn: 0.0 },
            size: 10.0,
            fill: RgbaF32::WHITE,
        };
        assert_eq!(
            arrow.arrow_vertices(0.0),
            [
                Vec2::new(4.0, 4.0),
                Vec2::new(4.0, 14.0),
                Vec2::new(14.0, 9.0)
            ]
        );
        let [a, b, apex] = arrow.arrow_vertices(PI);
        for (got, expected) in [
            (a, Vec2::new(14.0, 14.0)),
            (b, Vec2::new(14.0, 4.0)),
            (apex, Vec2::new(4.0, 9.0)),
        ] {
            assert!(got.distance(expected) < 1e-5, "{got} against {expected}");
        }
    }
}
