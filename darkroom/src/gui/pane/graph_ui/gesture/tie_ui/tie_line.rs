//! [`TieLine`]: the straight line a tie gesture draws, and what it crosses.

use glam::Vec2;

use crate::core::document::tie::TieId;
use crate::core::document::wire_key::WireKey;
use crate::gui::pane::graph_ui::draw::wire::Wire;
use crate::gui::pane::graph_ui::gesture::breaker_ui::scribble::BEZIER_SAMPLES;

/// A line shorter than this, in world units, crosses nothing: a press that barely moved.
const MIN_LENGTH: f32 = 3.0;

/// The tie line in flight, from where it latched to where the pointer holds it, in world units,
/// and what the last record pass found it crossing. The crossing buffers keep their capacity for
/// the next line.
#[derive(Debug, Default)]
pub(crate) struct TieLine {
    pub(crate) start: Vec2,
    pub(crate) end: Vec2,
    /// Each wire it crosses, at its first crossing along the line.
    pub(crate) wires: Vec<WireCrossing>,
    /// Each tie whose band it crosses.
    pub(crate) ties: Vec<TieCrossing>,
}

/// Where the line first crosses a wire: the piece it crosses, which is the route index a tie
/// put there takes, and the crossing's place along the line and on the canvas.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct WireCrossing {
    pub(crate) wire: WireKey,
    pub(crate) piece: usize,
    pub(crate) along: f32,
    pub(crate) point: Vec2,
}

/// A tie whose band the line crosses, and how far along the line.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct TieCrossing {
    pub(crate) tie: TieId,
    pub(crate) along: f32,
}

impl TieLine {
    /// Start a line at `at`, crossing nothing.
    pub(super) fn restart(&mut self, at: Vec2) {
        self.start = at;
        self.end = at;
        self.begin_pass();
    }

    /// Forget last pass's crossings, before this pass's draw finds them again.
    pub(super) fn begin_pass(&mut self) {
        self.wires.clear();
        self.ties.clear();
    }

    /// The first crossing of the line with `pieces`, a wire from its output end, or `None`.
    pub(crate) fn first_crossing(&self, wire: WireKey, pieces: &[Wire]) -> Option<WireCrossing> {
        if self.start.distance(self.end) < MIN_LENGTH {
            return None;
        }
        let (lo, hi) = (self.start.min(self.end), self.start.max(self.end));
        let mut first: Option<WireCrossing> = None;
        for (piece, curve) in pieces.iter().enumerate() {
            // A cubic stays inside its control points' box, so a piece whose box
            // misses the line's needs no samples.
            let hull = curve.hull();
            if hull.max().cmplt(lo).any() || hull.min.cmpgt(hi).any() {
                continue;
            }
            let mut prev = curve.p0;
            for sample in 1..=BEZIER_SAMPLES {
                let next = curve.at(sample as f32 / BEZIER_SAMPLES as f32);
                if let Some(along) = self.crosses(prev, next)
                    && first.is_none_or(|first| along < first.along)
                {
                    first = Some(WireCrossing {
                        wire,
                        piece,
                        along,
                        point: self.start.lerp(self.end, along),
                    });
                }
                prev = next;
            }
        }
        first
    }

    /// How far along the line, from 0 at its start to 1 at its end, it crosses the segment from
    /// `a` to `b`, or `None` when it misses it or runs parallel to it.
    pub(crate) fn crosses(&self, a: Vec2, b: Vec2) -> Option<f32> {
        if self.start.distance(self.end) < MIN_LENGTH {
            return None;
        }
        let (line, segment) = (self.end - self.start, b - a);
        let denominator = line.perp_dot(segment);
        if denominator == 0.0 {
            return None;
        }
        let to_segment = a - self.start;
        let along = to_segment.perp_dot(segment) / denominator;
        let on_segment = to_segment.perp_dot(line) / denominator;
        ((0.0..=1.0).contains(&along) && (0.0..=1.0).contains(&on_segment)).then_some(along)
    }
}
