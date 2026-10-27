//! The band a tie paints over its strands: a halo of the canvas colour, the band and its head,
//! and on a selected tie the selection ring and the knob that turns it.

use std::f32::consts::{FRAC_PI_2, PI};

use glam::Vec2;
use palantir::widget::{CircularArc, LineCap, Shape};
use palantir::{Rect, RgbaF32, Size, Stroke, Ui};

use crate::gui::pane::graph_ui::draw::tie::tie_layout::{Band, SLOT_SPACING};
use crate::gui::theme::tie_theme::TieTheme;

/// The band's length past its outer two strands, half at each end.
const BAND_OVERHANG: f32 = 12.0;
const BAND_WIDTH: f32 = 7.0;
/// The canvas-coloured gap between the band and the wires under it, each side.
const HALO: f32 = 1.5;
/// The head, at the band's `+axis` end: its length along the band and its width across it.
const HEAD_LENGTH: f32 = 11.0;
const HEAD_WIDTH: f32 = 12.0;
/// How far the head reaches into the band.
const HEAD_OVERLAP: f32 = 2.0;
/// The selection ring's half-height, its reach past the band's far end and past the head.
const RING_RADIUS: f32 = 9.0;
const RING_PAST_END: f32 = 5.0;
const RING_PAST_HEAD: f32 = 5.0;
const RING_WIDTH: f32 = 1.5;
/// The knob's distance from the head's outer edge, its radius and its rim.
const KNOB_REACH: f32 = 15.0;
const KNOB_RADIUS: f32 = 5.0;
const KNOB_RIM: f32 = 2.0;

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
}

/// One band's axes on the canvas: local `x` runs along the band axis, local `y` along the
/// strands, both from the tie's centre.
#[derive(Clone, Copy, Debug)]
pub(crate) struct BandAxes {
    centre: Vec2,
    axis: Vec2,
    dir: Vec2,
    /// Half the band's length.
    half: f32,
}

impl BandAxes {
    pub(crate) fn of(band: &Band) -> Self {
        let span = (band.strand_count() - 1) as f32 * SLOT_SPACING;
        Self {
            centre: band.centre,
            axis: band.axis,
            dir: band.dir,
            half: f32::midpoint(span, BAND_OVERHANG),
        }
    }

    fn at(self, x: f32, y: f32) -> Vec2 {
        self.centre + self.axis * x + self.dir * y
    }

    /// A square around everything the band paints, at any angle: the farthest reach is the
    /// knob's rim, on the axis.
    pub(crate) fn bounds(self) -> Rect {
        let reach = self.knob_x() + KNOB_RADIUS + KNOB_RIM;
        Rect {
            min: self.centre - Vec2::splat(reach),
            size: Size::new(reach * 2.0, reach * 2.0),
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
        Rect {
            min,
            size: Size::new(max.x - min.x, max.y - min.y),
        }
    }

    /// The knob's centre, on the axis past the head.
    pub(crate) fn knob_centre(self) -> Vec2 {
        self.at(self.knob_x(), 0.0)
    }

    fn head_x(self) -> f32 {
        self.half - HEAD_OVERLAP + HEAD_LENGTH * 0.5
    }

    fn knob_x(self) -> f32 {
        self.half - HEAD_OVERLAP + HEAD_LENGTH + KNOB_REACH
    }

    pub(crate) fn paint(self, ui: &mut Ui, theme: &TieTheme, look: BandLook) {
        let ink = match look.ink {
            BandInk::Rest => theme.band,
            BandInk::Lit => theme.band_lit,
            BandInk::Crossed => theme.ring,
        };
        // The band as a round-capped line whose caps reach its ends.
        let cap = BAND_WIDTH * 0.5;
        let band = [
            self.at(-self.half + cap, 0.0),
            self.at(self.half - cap, 0.0),
        ];
        self.line(ui, band, theme.halo, BAND_WIDTH + HALO * 2.0);
        self.line(ui, band, ink, BAND_WIDTH);
        // The head as a round-capped line across the band whose caps reach its width.
        let across = (HEAD_WIDTH - HEAD_LENGTH) * 0.5;
        let head = [
            self.at(self.head_x(), -across),
            self.at(self.head_x(), across),
        ];
        self.line(ui, head, theme.halo, HEAD_LENGTH + HALO * 2.0);
        self.line(ui, head, ink, HEAD_LENGTH);
        if look.selected {
            self.ring(ui, theme.ring);
            self.knob(
                ui,
                theme.ring,
                theme.halo,
                look.pinned.then_some(theme.pinned),
            );
        }
    }

    fn line(self, ui: &mut Ui, [a, b]: [Vec2; 2], color: RgbaF32, width: f32) {
        ui.add_shape(Shape::line(a, b, Stroke::new(color, width)).cap(LineCap::Round));
    }

    /// A capsule outline around the band and its head.
    fn ring(self, ui: &mut Ui, color: RgbaF32) {
        let stroke = Stroke::new(color, RING_WIDTH);
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
    /// itself, filled with `pinned` for one a drag pinned.
    fn knob(self, ui: &mut Ui, ring: RgbaF32, halo: RgbaF32, pinned: Option<RgbaF32>) {
        let ring_end = self.half - HEAD_OVERLAP + HEAD_LENGTH + RING_PAST_HEAD;
        let centre = self.knob_centre();
        ui.add_shape(Shape::line(
            self.at(ring_end, 0.0),
            centre,
            Stroke::new(ring, RING_WIDTH),
        ));
        let (fill, rim) = match pinned {
            Some(fill) => (fill, halo),
            None => (halo, ring),
        };
        // A disk as a circle stroked as wide as its radius.
        let disk = KNOB_RADIUS * 0.5;
        ui.add_shape(Shape::circle(centre, disk, Stroke::new(fill, KNOB_RADIUS)));
        ui.add_shape(Shape::circle(
            centre,
            KNOB_RADIUS,
            Stroke::new(rim, KNOB_RIM),
        ));
    }
}
