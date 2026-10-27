//! [`TieProbe`]: the tie line as the canvas draw tests against it.

use glam::Vec2;

use crate::core::document::tie::TieId;
use crate::core::document::wire_key::WireKey;
use crate::gui::pane::graph_ui::frame::wire::Wire;
use crate::gui::pane::graph_ui::gesture::tie_ui::tie_line::{TieCrossing, TieLine, WireCrossing};

/// The tie line in flight, threaded through the wire and band draws so each records what the
/// line crosses with the geometry it paints, as the breaker probe does.
#[derive(Debug)]
pub(crate) struct TieProbe<'a> {
    /// The live line, or `None` when no line is in flight.
    live: Option<&'a mut TieLine>,
}

impl<'a> TieProbe<'a> {
    pub(super) const fn new(live: Option<&'a mut TieLine>) -> Self {
        Self { live }
    }

    pub(crate) const fn is_active(&self) -> bool {
        self.live.is_some()
    }

    /// Where the line first crosses `wire`, painted as `pieces`, recorded for the release.
    pub(crate) fn cross_wire(&mut self, wire: WireKey, pieces: &[Wire]) -> Option<WireCrossing> {
        let line = self.live.as_deref_mut()?;
        let crossing = line.first_crossing(wire, pieces)?;
        line.wires.push(crossing);
        Some(crossing)
    }

    /// Whether the line crosses the band of `tie`, which runs from `a` to `b`, recorded for the
    /// release.
    pub(crate) fn cross_band(&mut self, tie: TieId, [a, b]: [Vec2; 2]) -> bool {
        let Some(line) = self.live.as_deref_mut() else {
            return false;
        };
        let Some(along) = line.crosses(a, b) else {
            return false;
        };
        line.ties.push(TieCrossing { tie, along });
        true
    }
}
