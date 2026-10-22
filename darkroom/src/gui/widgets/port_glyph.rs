//! The small sensing glyph a wire terminates on: a filled circle or a
//! rounded triangle in a generously grown hit box, with a hover tooltip.

use palantir::TextInput;
use palantir::prelude::*;
use palantir::widget::Shape;

use crate::gui::widgets::support;

/// Hover / grab box scaled past the painted glyph so ports, event triangles,
/// and subscription pins are easier to hit and snap to, while the visible
/// shape stays its own size. The enlarged box is also what keeps the wire
/// hover-highlight repaint-correct: the glyph's own (hover-target) box carries
/// the emphasis zone, so entering/leaving it is a hover-target change and
/// repaints without any pointer subscription.
const HIT_SCALE: f32 = 1.8;

/// Corner rounding of the triangle glyphs, matching the soft corners of the
/// rest of the interface.
const TRIANGLE_RADIUS: f32 = 2.0;

/// Stroke width of the muted ring drawn around a circle with an
/// [`outline`](PortGlyph::outline). Also the amount
/// [`enlarged_diameter`] grows a circle by (on each side), so a
/// plain enlarged circle's footprint matches a ringed one — "important port"
/// reads as one consistent size regardless of which visual carries it.
const OUTLINE_WIDTH: f32 = 2.5;

/// What a glyph paints inside its box.
#[derive(Clone, Copy, Debug)]
enum GlyphShape {
    /// A filled disc, optionally ringed by an annulus strictly outside it.
    Circle { outline: Option<RgbaF32> },
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
/// recorded after its own `ui` borrow ends.
///
/// The explicit `wid` is not optional: these ids are reconstructed from domain
/// coordinates by passes that never record the glyph (the geometry rebuild,
/// the snap scans), so they must not drift with the parent structure.
#[derive(Debug)]
pub(crate) struct PortGlyph<'a> {
    wid: WidgetId,
    glyph: Painted,
    placement: Placement,
    tooltip: Option<TextInput<'a>>,
}

/// What a [`PortGlyph`] saw this frame.
///
/// Plain bools rather than the palantir `Response` they come from: that
/// borrows `Ui`, so it could not escape the panel closure a glyph is usually
/// recorded inside — and these are the two edges every caller reads.
#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct PortGlyphResponse {
    /// Left-double-clicked — the binding toggle on a port circle.
    pub(crate) double_clicked: bool,
    /// Right-clicked. The glyph senses its own `CLICK` and consumes hits over
    /// its rect, so a cell wrapping one must read this too: a right-click
    /// landed on the glyph never reaches the cell (no bubbling).
    pub(crate) secondary_clicked: bool,
}

impl<'a> PortGlyph<'a> {
    /// A data port's circle, `diameter` across, inked `fill`.
    pub(crate) const fn circle(wid: WidgetId, diameter: f32, fill: RgbaF32) -> Self {
        Self::new(
            wid,
            Painted {
                shape: GlyphShape::Circle { outline: None },
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
            tooltip: None,
        }
    }

    /// Ring a circle with an annulus strictly outside its fill. Ignored by
    /// [`arrow`](Self::arrow). Default: no ring.
    pub(crate) const fn outline(mut self, color: RgbaF32) -> Self {
        if let GlyphShape::Circle { outline } = &mut self.glyph.shape {
            *outline = Some(color);
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

    /// Hover tooltip. `None` (the default) records none — which is what a
    /// port off the hovered node passes, since only that node builds tooltips.
    /// A `&'static str` stays borrowed until the bubble records; an
    /// [`InternedStr`](palantir::InternedStr) from [`fmt!`](palantir::fmt) is
    /// already in the record pass's text arena.
    pub(crate) fn tooltip(mut self, tooltip: Option<impl Into<TextInput<'a>>>) -> Self {
        self.tooltip = tooltip.map(Into::into);
        self
    }

    pub(crate) fn show(self, ui: &mut Ui) -> PortGlyphResponse {
        let Self {
            wid,
            glyph,
            placement,
            tooltip,
        } = self;
        let (hit, inset) = (glyph.hit(), glyph.inset());
        let panel = Panel::zstack()
            .id(wid)
            .size((Sizing::fixed(hit), Sizing::fixed(hit)))
            .sense(Sense::CLICK | Sense::DRAG);
        let panel = match placement {
            Placement::Margin(margin) => {
                let [l, t, r, b] = margin.as_array();
                panel.margin(Spacing::new(l - inset, t - inset, r - inset, b - inset))
            }
            Placement::CenteredOn(point) => panel.pos(point - Vec2::splat(hit * 0.5)),
        };
        let shown = panel.show(ui, |ui| glyph.draw(ui));
        // Take the owned snapshot + the click edges so the glyph's `ui` borrow
        // ends before the tooltip records into `ui`.
        let snapshot = shown.response.snapshot();
        let response = PortGlyphResponse {
            double_clicked: shown.response.left.double_clicked(),
            secondary_clicked: shown.response.right.clicked(),
        };
        support::tooltip_after(ui, &snapshot, tooltip);
        response
    }
}

/// A circle's diameter grown by [`OUTLINE_WIDTH`] on each side, so a plain
/// circle's footprint matches an [`outline`](PortGlyph::outline)d one.
pub(crate) const fn enlarged_diameter(base: f32) -> f32 {
    base + 2.0 * OUTLINE_WIDTH
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

    /// Paint into the `size`-sided square at [`Self::inset`] within the
    /// [`Self::hit`]-sided box.
    fn draw(self, ui: &mut Ui) {
        let Self { shape, size, fill } = self;
        let (hit, inset) = (self.hit(), self.inset());
        match shape {
            GlyphShape::Circle { outline } => {
                let rect = Rect::new(inset, inset, size, size);
                let radius = size * 0.5;
                // The ring paints *before* the fill — an annulus strictly
                // outside the fill's radius, so they don't overlap either way.
                // A stroke paints its own rect's *inner*-edge annulus, so
                // drawing it on `rect` would eat into the fill; inflating first
                // lands the ring's inner edge exactly on the fill's outer edge.
                if let Some(color) = outline {
                    ui.add_shape(
                        Shape::rect(rect.inflated(OUTLINE_WIDTH))
                            .corners(radius + OUTLINE_WIDTH)
                            .border(Stroke::new(color, OUTLINE_WIDTH)),
                    );
                }
                ui.add_shape(Shape::rect(rect).corners(radius).fill(fill));
            }
            GlyphShape::Arrow { turn } => {
                // Vertices are inset by the corner radius — the SDF rounds by
                // *dilating* (`sdf - radius`), so the rounded result grows back
                // out to the glyph box instead of past it. `turn` rotates the
                // rounded points about the box center; the layout box is
                // unchanged (the glyph isn't clipped to the owner rect, so a
                // rotated apex may exceed it).
                let r = TRIANGLE_RADIUS;
                let center = Vec2::splat(hit * 0.5);
                let rot = Vec2::from_angle(turn);
                let tf = |v: Vec2| center + rot.rotate(v - center);
                ui.add_shape(
                    Shape::triangle(
                        tf(Vec2::new(inset + r, inset + r)),
                        tf(Vec2::new(inset + r, inset + size - r)),
                        tf(Vec2::new(inset + size - r, inset + size * 0.5)),
                    )
                    .radius(r)
                    .fill(fill),
                );
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
        // 10 + 2 * 2.5: the ring's width on both sides.
        assert_eq!(
            enlarged_diameter(10.0),
            15.0,
            "enlarged port matches an outlined circle's footprint"
        );
    }

    /// The subscription pin is the emitter arrow turned by `PI + FRAC_PI_4`.
    /// Half of that (the `PI`) mirrors the apex from right to left; the
    /// remaining quarter-turn aims it up-left at the incoming wire. Checked
    /// here because the rotation is the one thing about the pin that isn't
    /// visible at its call site.
    #[test]
    fn a_half_turn_mirrors_the_arrow_apex_across_the_box_center() {
        let (size, hit) = (10.0f32, 10.0f32 * HIT_SCALE);
        let inset = (hit - size) * 0.5;
        let center = Vec2::splat(hit * 0.5);
        let apex = Vec2::new(inset + size - size * 0.0, inset + size * 0.5);
        let turned = center + Vec2::from_angle(PI).rotate(apex - center);
        // Apex sits `size/2` right of center; a half turn puts it `size/2` left.
        assert!(
            (apex.x - center.x - size * 0.5).abs() < 1e-4,
            "base apex points right"
        );
        assert!(
            (turned.x - center.x + size * 0.5).abs() < 1e-4,
            "turned apex points left, mirrored about the center"
        );
        assert!(
            (turned.y - center.y).abs() < 1e-4,
            "a half turn keeps the apex on the center line"
        );
    }
}
