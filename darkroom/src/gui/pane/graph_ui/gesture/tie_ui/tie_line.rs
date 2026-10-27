//! [`TieLine`]: the straight line a tie gesture draws, and what it crosses.

use glam::Vec2;

use crate::core::document::tie::TieId;
use crate::core::document::wire_key::WireKey;
use crate::gui::pane::graph_ui::frame::wire::Wire;
use crate::gui::pane::graph_ui::gesture::segments;

/// A line shorter than this on screen, in logical px, crosses nothing: a press that barely
/// moved, which is as far at every zoom.
const MIN_LENGTH: f32 = 3.0;

/// The tie line in flight, from where it latched to where the pointer holds it, in world units,
/// and what the last record pass found it crossing. The crossing buffers keep their capacity for
/// the next line.
#[derive(Debug, Default)]
pub(crate) struct TieLine {
    pub(crate) start: Vec2,
    pub(crate) end: Vec2,
    /// Screen px per world unit when the end last moved, which [`MIN_LENGTH`] is measured at.
    pub(crate) zoom: f32,
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
    /// Start a line at `at`, drawn at `zoom`, crossing nothing.
    pub(super) fn restart(&mut self, at: Vec2, zoom: f32) {
        self.start = at;
        self.end = at;
        self.zoom = zoom;
        self.begin_pass();
    }

    /// Move the line's end to `at`, drawn at `zoom`.
    pub(super) const fn reach(&mut self, at: Vec2, zoom: f32) {
        self.end = at;
        self.zoom = zoom;
    }

    /// Whether the line is long enough on screen to cross anything.
    fn is_long(&self) -> bool {
        self.start.distance(self.end) * self.zoom >= MIN_LENGTH
    }

    /// Forget last pass's crossings, before this pass's draw finds them again.
    pub(super) fn begin_pass(&mut self) {
        self.wires.clear();
        self.ties.clear();
    }

    /// The first crossing of the line with `pieces`, a wire from its output end, or `None`.
    pub(crate) fn first_crossing(&self, wire: WireKey, pieces: &[Wire]) -> Option<WireCrossing> {
        if !self.is_long() {
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
            for [a, b] in curve.chords() {
                if let Some(along) = self.crossing(a, b)
                    && first.is_none_or(|first| along < first.along)
                {
                    first = Some(WireCrossing {
                        wire,
                        piece,
                        along,
                        // Start plus the fraction of the run, not `lerp`'s weighted sum of
                        // the ends, which rounds a straight line's own coordinate off it.
                        point: self.start + (self.end - self.start) * along,
                    });
                }
            }
        }
        first
    }

    /// How far along the line, from 0 at its start to 1 at its end, it crosses the segment from
    /// `a` to `b`, or `None` when it misses it, runs parallel to it, or is too short to cross.
    pub(crate) fn crosses(&self, a: Vec2, b: Vec2) -> Option<f32> {
        self.is_long().then(|| self.crossing(a, b)).flatten()
    }

    /// [`Self::crosses`] for a line already known to be long enough, by the strokes' one rule.
    fn crossing(&self, a: Vec2, b: Vec2) -> Option<f32> {
        segments::crossing([self.start, self.end], [a, b])
    }
}

#[cfg(test)]
mod tests {
    use glam::Vec2;
    use scenarium::{InputPort, NodeId, OutputPort};

    use crate::core::document::wire_key::{DataWire, WireKey};
    use crate::gui::pane::graph_ui::frame::wire::Wire;
    use crate::gui::pane::graph_ui::gesture::tie_ui::tie_line::{TieLine, WireCrossing};

    fn line(start: Vec2, end: Vec2, zoom: f32) -> TieLine {
        let mut line = TieLine::default();
        line.restart(start, zoom);
        line.reach(end, zoom);
        line
    }

    /// A line from (0, 0) to (10, 0) crosses the segment x = 5 halfway, misses one past its
    /// end, and crosses none it runs parallel to. It crosses nothing until it is 3 px long on
    /// screen: 3 world units at zoom 1, 30 at zoom 0.1.
    #[test]
    fn a_line_crosses_a_segment_at_its_fraction_once_it_is_long_on_screen() {
        let across = line(Vec2::ZERO, Vec2::new(10.0, 0.0), 1.0);
        let at = |x: f32| (Vec2::new(x, -1.0), Vec2::new(x, 1.0));
        let (a, b) = at(5.0);
        assert_eq!(across.crosses(a, b), Some(0.5));
        let (a, b) = at(12.0);
        assert_eq!(across.crosses(a, b), None, "past the end");
        assert_eq!(
            across.crosses(Vec2::new(0.0, 1.0), Vec2::new(10.0, 1.0)),
            None,
            "parallel"
        );

        let segment = (Vec2::new(1.0, -1.0), Vec2::new(1.0, 1.0));
        for (length, zoom, crosses) in [
            (3.0, 1.0, true),
            (2.9, 1.0, false),
            (30.0, 0.1, true),
            (29.0, 0.1, false),
        ] {
            let short = line(Vec2::ZERO, Vec2::new(length, 0.0), zoom);
            assert_eq!(
                short.crosses(segment.0, segment.1).is_some(),
                crosses,
                "{length} at {zoom}"
            );
        }
    }

    /// Over a wire of two pieces, the line's first crossing is its earliest along the line,
    /// whichever piece it is on, at the point the fraction names.
    #[test]
    fn the_first_crossing_is_the_earliest_along_the_line() {
        let wire = WireKey::Data(DataWire {
            output: OutputPort::new(NodeId::unique(), 0),
            input: InputPort::new(NodeId::unique(), 0),
        });
        // Two straight vertical pieces, the first at x = 6 and the second at x = 2.
        let vertical = |x: f32| Wire {
            p0: Vec2::new(x, -4.0),
            p1: Vec2::new(x, -2.0),
            p2: Vec2::new(x, 2.0),
            p3: Vec2::new(x, 4.0),
        };
        let across = line(Vec2::ZERO, Vec2::new(10.0, 0.0), 1.0);
        assert_eq!(
            across.first_crossing(wire, &[vertical(6.0), vertical(2.0)]),
            Some(WireCrossing {
                wire,
                piece: 1,
                along: 0.2,
                point: Vec2::new(2.0, 0.0),
            })
        );
        assert_eq!(across.first_crossing(wire, &[vertical(20.0)]), None);

        // A level wire from (0, 0) to (200, 0) has its middle sample at x = 100 exactly, the end
        // of chord 7 and the start of chord 8: a line straight through it crosses the wire,
        // halfway along the line.
        let level = Wire::data(Vec2::ZERO, Vec2::new(200.0, 0.0));
        let through = line(Vec2::new(100.0, -20.0), Vec2::new(100.0, 20.0), 1.0);
        let crossing = through.first_crossing(wire, &[level]).expect("it crosses");
        assert_eq!(
            (crossing.along, crossing.point),
            (0.5, Vec2::new(100.0, 0.0))
        );
    }
}
