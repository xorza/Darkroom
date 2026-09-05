//! The small sensing glyph a wire terminates on: a filled circle or a
//! rounded triangle in a generously grown hit box, with a hover tooltip.

use glam::Vec2;
use palantir::{
    Configure, Panel, Rect, RgbaF32, Sense, Shape, Sizing, Spacing, TextInput, Ui, WidgetId,
};

use crate::gui::widgets::support::{filled_rect, stroked_rect, tooltip_after};

/// Hover / grab box scaled past the painted glyph so ports, event triangles,
/// and subscription pins are easier to hit and snap to, while the visible
/// shape stays its own size. The enlarged box is also what keeps the wire
/// hover-highlight repaint-correct: the glyph's own (hover-target) box carries
/// the emphasis zone, so entering/leaving it is a hover-target change and
/// repaints without any pointer subscription.
const HIT_SCALE: f32 = 1.8;

/// Corner rounding of the triangle glyphs, matching the soft corners of the
/// rest of the chrome.
const TRIANGLE_RADIUS: f32 = 2.0;

/// Stroke width of the muted ring drawn around a circle with an
/// [`outline`](PortGlyph::outline). Also the amount
/// [`PortGlyph::enlarged_diameter`] grows a circle by (on each side), so a
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
/// size, colours and tooltip, and maps the returned clicks onto its own
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
    /// Side of the *painted* shape; the sensing box is [`HIT_SCALE`] of this.
    size: f32,
    shape: GlyphShape,
    fill: RgbaF32,
    placement: Placement,
    tip: Option<TextInput<'a>>,
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
    /// A data port's circle, `diameter` across.
    pub(crate) fn circle(wid: WidgetId, diameter: f32) -> Self {
        Self::new(wid, diameter, GlyphShape::Circle { outline: None })
    }

    /// An emitter event's triangle, apex pointing right (the emit direction).
    pub(crate) fn arrow(wid: WidgetId, size: f32) -> Self {
        Self::new(wid, size, GlyphShape::Arrow { turn: 0.0 })
    }

    fn new(wid: WidgetId, size: f32, shape: GlyphShape) -> Self {
        Self {
            wid,
            size,
            shape,
            fill: RgbaF32::WHITE,
            placement: Placement::Margin(Spacing::ZERO),
            tip: None,
        }
    }

    /// A circle's diameter grown by [`OUTLINE_WIDTH`] on each side, so a plain
    /// circle's footprint matches an [`outline`](Self::outline)d one. `false`
    /// returns `base` unchanged.
    pub(crate) fn enlarged_diameter(base: f32, enlarged: bool) -> f32 {
        if enlarged {
            base + 2.0 * OUTLINE_WIDTH
        } else {
            base
        }
    }

    /// Ink of the painted shape. Defaults to white — every caller sets it.
    pub(crate) fn fill(mut self, color: RgbaF32) -> Self {
        self.fill = color;
        self
    }

    /// Ring a circle with an annulus strictly outside its fill. Ignored by
    /// [`arrow`](Self::arrow). Default: no ring.
    pub(crate) fn outline(mut self, color: RgbaF32) -> Self {
        if let GlyphShape::Circle { outline } = &mut self.shape {
            *outline = Some(color);
        }
        self
    }

    /// Rotate an arrow's apex by `radians` about the box center. Ignored by
    /// [`circle`](Self::circle). Default: none (apex points right).
    pub(crate) fn turn(mut self, radians: f32) -> Self {
        if let GlyphShape::Arrow { turn } = &mut self.shape {
            *turn = radians;
        }
        self
    }

    /// Lay the glyph out in flow with `margin`. The hit-box growth is folded
    /// back out of it, so node layout and the glyph's own position are
    /// unchanged and only the hover/grab area grows. Default: no margin.
    pub(crate) fn margin(mut self, margin: Spacing) -> Self {
        self.placement = Placement::Margin(margin);
        self
    }

    /// Place the glyph out of flow, its *grown* box centered on `point` in the
    /// parent's coordinate space — for one that hangs off a corner rather than
    /// sitting in a row.
    pub(crate) fn centered_on(mut self, point: Vec2) -> Self {
        self.placement = Placement::CenteredOn(point);
        self
    }

    /// Hover tooltip. `None` (the default) records none — which is what a
    /// port off the hovered node passes, since only that node builds tips.
    /// A `&'static str` stays borrowed until the bubble records; an
    /// [`InternedStr`](palantir::InternedStr) from [`fmt!`](palantir::fmt) is
    /// already in the record pass's text arena.
    pub(crate) fn tip(mut self, tip: Option<impl Into<TextInput<'a>>>) -> Self {
        self.tip = tip.map(Into::into);
        self
    }

    pub(crate) fn show(self, ui: &mut Ui) -> PortGlyphResponse {
        let Self {
            wid,
            size,
            shape,
            fill,
            placement,
            tip,
        } = self;
        let hit = size * HIT_SCALE;
        // Half the growth — the painted glyph's offset within the grown box.
        let inset = (hit - size) * 0.5;
        let panel = Panel::zstack()
            .id(wid)
            .size((Sizing::fixed(hit), Sizing::fixed(hit)))
            .sense(Sense::CLICK | Sense::DRAG);
        let panel = match placement {
            Placement::Margin(margin) => {
                let [l, t, r, b] = margin.as_array();
                panel.margin(Spacing::new(l - inset, t - inset, r - inset, b - inset))
            }
            Placement::CenteredOn(point) => panel.position(point - Vec2::splat(hit * 0.5)),
        };
        let glyph = panel.show(ui, |ui| shape.draw(ui, size, inset, hit, fill));
        // Take the owned snapshot + the click edges so the glyph's `ui` borrow
        // ends before the tooltip records into `ui`.
        let snapshot = glyph.response.snapshot();
        let response = PortGlyphResponse {
            double_clicked: glyph.response.left.double_clicked(),
            secondary_clicked: glyph.response.right.clicked(),
        };
        tooltip_after(ui, &snapshot, tip);
        response
    }
}

impl GlyphShape {
    /// Paint into the `size`-sided square at `inset` within the `hit`-sided box.
    fn draw(self, ui: &mut Ui, size: f32, inset: f32, hit: f32, fill: RgbaF32) {
        match self {
            GlyphShape::Circle { outline } => {
                let rect = Rect::new(inset, inset, size, size);
                let radius = size * 0.5;
                // The ring paints *before* the fill — an annulus strictly
                // outside the fill's radius, so they don't overlap either way.
                // A stroke paints its own rect's *inner*-edge annulus, so
                // drawing it on `rect` would eat into the fill; inflating first
                // lands the ring's inner edge exactly on the fill's outer edge.
                if let Some(color) = outline {
                    stroked_rect(
                        ui,
                        rect.inflated(OUTLINE_WIDTH),
                        radius + OUTLINE_WIDTH,
                        color,
                        OUTLINE_WIDTH,
                    );
                }
                filled_rect(ui, rect, radius, fill);
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

    #[test]
    fn enlarged_diameter_grows_by_the_outline_width_on_each_side() {
        let base = 10.0;
        assert_eq!(
            PortGlyph::enlarged_diameter(base, false),
            base,
            "plain port is unchanged"
        );
        assert_eq!(
            PortGlyph::enlarged_diameter(base, true),
            base + 2.0 * OUTLINE_WIDTH,
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
        let turned = center + Vec2::from_angle(std::f32::consts::PI).rotate(apex - center);
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
