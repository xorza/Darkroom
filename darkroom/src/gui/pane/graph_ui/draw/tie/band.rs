//! The band a tie paints over its strands: a halo of the canvas colour, the band and its head,
//! and on a selected tie the selection ring and the knob that turns it.

use std::f32::consts::{FRAC_PI_2, PI};

use glam::Vec2;
use palantir::widget::{CircularArc, LineCap, Shape};
use palantir::{Rect, RgbaF32, Size, Stroke, Ui};

use crate::gui::pane::graph_ui::frame::tie_layout::{Band, SLOT_SPACING};
use crate::gui::theme::Theme;

/// The band's shape, as an icon's artwork is its own; its strokes are the theme's `stroke.fine`.
///
/// The band's length past its outer two strands, half at each end.
const BAND_OVERHANG: f32 = 12.0;
const BAND_WIDTH: f32 = 7.0;
/// The head, at the band's `+axis` end: its length along the band and its width across it.
const HEAD_LENGTH: f32 = 11.0;
const HEAD_WIDTH: f32 = 12.0;
/// How far the head reaches into the band.
const HEAD_OVERLAP: f32 = 2.0;
/// The selection ring's half-height, its reach past the band's far end and past the head.
const RING_RADIUS: f32 = 9.0;
const RING_PAST_END: f32 = 5.0;
const RING_PAST_HEAD: f32 = 5.0;
/// The knob's distance from the head's outer edge, and its radius.
const KNOB_REACH: f32 = 15.0;
const KNOB_RADIUS: f32 = 5.0;

/// The extra reach of a band's hover and press target past the band and its head.
const HIT_MARGIN: f32 = 4.0;
/// The knob's press target radius, wider than the knob it paints.
pub(crate) const KNOB_HIT_RADIUS: f32 = 11.0;

/// How one band paints this pass.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct BandLook {
    pub(crate) ink: BandInk,
    /// Selected: the selection ring and the knob paint too.
    pub(crate) selected: bool,
    /// The angle is pinned, so the knob paints filled.
    pub(crate) pinned: bool,
}

/// The ink of a band and its head.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum BandInk {
    /// The muted ink, so the wire colours stay clear.
    Rest,
    /// Hovered or selected.
    Lit,
    /// A tie line in flight crosses it.
    Crossed,
    /// Where a tie line in flight will put a new tie: the band at low strength, over the wires it
    /// previews, with no halo to cover them.
    Preview,
}

/// One band's axes on the canvas: local `x` runs along the band axis, local `y` along the
/// strands, both from the tie's center.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct BandAxes {
    center: Vec2,
    axis: Vec2,
    dir: Vec2,
    /// Half the band's length.
    half: f32,
}

impl BandAxes {
    /// The axes of `band`, which places at least one strand: an empty band has no length.
    pub(crate) fn of(band: &Band) -> Self {
        debug_assert!(band.strand_count() > 0, "a band with axes places a strand");
        let span = (band.strand_count() - 1) as f32 * SLOT_SPACING;
        Self {
            center: band.center,
            axis: band.axis,
            dir: band.dir,
            half: f32::midpoint(span, BAND_OVERHANG),
        }
    }

    fn at(self, x: f32, y: f32) -> Vec2 {
        self.center + self.axis * x + self.dir * y
    }

    /// A square around everything the band paints, at any angle: the farthest reach is the
    /// knob's rim, `rim` wide, on the axis.
    pub(crate) fn bounds(self, rim: f32) -> Rect {
        let reach = self.knob_x() + KNOB_RADIUS + rim * 0.5;
        Rect {
            min: self.center - Vec2::splat(reach),
            size: Size::new(reach * 2.0, reach * 2.0),
        }
    }

    /// The axes of a band not placed yet, centered on `center` and square to strands that run
    /// along `dir`, a unit vector, `strand_count` of them.
    pub(crate) fn preview(center: Vec2, dir: Vec2, strand_count: u32) -> Self {
        debug_assert!(strand_count > 0, "a band previews a strand");
        let span = (strand_count - 1) as f32 * SLOT_SPACING;
        Self {
            center,
            axis: dir.perp(),
            dir,
            half: f32::midpoint(span, BAND_OVERHANG),
        }
    }

    /// The band along its axis, from its far end to the head's outer edge: what a tie line
    /// crosses to cross the tie.
    pub(crate) fn reach(self) -> [Vec2; 2] {
        [
            self.at(-self.half, 0.0),
            self.at(self.half - HEAD_OVERLAP + HEAD_LENGTH, 0.0),
        ]
    }

    /// The box around the band and its head, grown by a margin: the tie's hover and press
    /// target.
    pub(crate) fn hit_rect(self) -> Rect {
        let [a, b] = self.reach();
        let grow = HEAD_WIDTH * 0.5 + HIT_MARGIN;
        let (min, max) = (a.min(b) - Vec2::splat(grow), a.max(b) + Vec2::splat(grow));
        Rect::from_min_max(min, max)
    }

    /// The knob's center, on the axis past the head.
    pub(crate) fn knob_center(self) -> Vec2 {
        self.at(self.knob_x(), 0.0)
    }

    fn head_x(self) -> f32 {
        self.half - HEAD_OVERLAP + HEAD_LENGTH * 0.5
    }

    fn knob_x(self) -> f32 {
        self.half - HEAD_OVERLAP + HEAD_LENGTH + KNOB_REACH
    }

    /// Paint the band in the theme's tie inks, the halo, ring, stem and knob rim at its
    /// `stroke.fine`, so a selected tie draws its marks at one weight.
    pub(crate) fn paint(self, ui: &mut Ui, theme: &Theme, look: BandLook) {
        let line = theme.stroke.fine;
        let theme = &theme.tie;
        let ink = match look.ink {
            BandInk::Rest => theme.band,
            BandInk::Lit => theme.band_lit,
            BandInk::Crossed => theme.ring,
            BandInk::Preview => theme.band_preview,
        };
        let halo = (look.ink != BandInk::Preview).then_some(theme.halo);
        // The band as a round-capped line whose caps reach its ends.
        let cap = BAND_WIDTH * 0.5;
        let band = [
            self.at(-self.half + cap, 0.0),
            self.at(self.half - cap, 0.0),
        ];
        if let Some(halo) = halo {
            self.line(ui, band, halo, BAND_WIDTH + line * 2.0);
        }
        self.line(ui, band, ink, BAND_WIDTH);
        // The head as a round-capped line across the band whose caps reach its width.
        let across = (HEAD_WIDTH - HEAD_LENGTH) * 0.5;
        let head = [
            self.at(self.head_x(), -across),
            self.at(self.head_x(), across),
        ];
        if let Some(halo) = halo {
            self.line(ui, head, halo, HEAD_LENGTH + line * 2.0);
        }
        self.line(ui, head, ink, HEAD_LENGTH);
        if look.selected {
            let ring = Stroke::new(theme.ring, line);
            self.ring(ui, ring);
            self.knob(ui, ring, theme.halo, look.pinned.then_some(theme.pinned));
        }
    }

    fn line(self, ui: &mut Ui, [a, b]: [Vec2; 2], color: RgbaF32, width: f32) {
        ui.add_shape(Shape::line(a, b, Stroke::new(color, width)).cap(LineCap::Round));
    }

    /// A capsule outline around the band and its head.
    fn ring(self, ui: &mut Ui, stroke: Stroke) {
        let left = -self.half - RING_PAST_END + RING_RADIUS;
        let right = self.half - HEAD_OVERLAP + HEAD_LENGTH + RING_PAST_HEAD - RING_RADIUS;
        for y in [-RING_RADIUS, RING_RADIUS] {
            ui.add_shape(Shape::line(self.at(left, y), self.at(right, y), stroke));
        }
        let axis_angle = self.axis.y.atan2(self.axis.x);
        for (x, start_angle) in [
            (right, axis_angle - FRAC_PI_2),
            (left, axis_angle + FRAC_PI_2),
        ] {
            ui.add_shape(Shape::arc(
                CircularArc {
                    center: self.at(x, 0.0),
                    radius: RING_RADIUS,
                    start_angle,
                    sweep: PI,
                },
                stroke,
            ));
        }
    }

    /// The stem from the ring to the knob, and the knob: hollow for an angle the band takes
    /// itself, filled with `pinned` for one a drag pinned. The stem and the rim stroke as `ring`.
    fn knob(self, ui: &mut Ui, ring: Stroke, halo: RgbaF32, pinned: Option<RgbaF32>) {
        let ring_end = self.half - HEAD_OVERLAP + HEAD_LENGTH + RING_PAST_HEAD;
        let center = self.knob_center();
        ui.add_shape(Shape::line(self.at(ring_end, 0.0), center, ring));
        let (fill, rim) = match pinned {
            Some(fill) => (fill, halo),
            None => (halo, ring.color),
        };
        // A disk as a circle stroked as wide as its radius.
        let disk = KNOB_RADIUS * 0.5;
        ui.add_shape(Shape::circle(center, disk, Stroke::new(fill, KNOB_RADIUS)));
        ui.add_shape(Shape::circle(
            center,
            KNOB_RADIUS,
            Stroke::new(rim, ring.width),
        ));
    }
}
