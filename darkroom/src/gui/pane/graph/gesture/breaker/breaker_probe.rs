//! [`BreakerProbe`]: the breaker gesture as the canvas draw tests against it.

use palantir::Rect;
use scenarium::{InputPort, NodeId, Subscription};

use crate::gui::pane::graph::gesture::breaker::scribble::Scribble;
use crate::gui::pane::graph::paint::wire::Wire;

/// The active gesture, threaded through node and wire rendering so
/// intersection tests run inline with the draw that knows the geometry.
/// Passed as `&mut BreakerProbe<'_>` so Rust auto-reborrows at each
/// nested call.
///
/// Everything it tests against is already in the polyline's own frame
/// (inner-canvas pre-transform world coords), because every caller takes its
/// geometry from `CanvasGeometry` rather than from a raw `Ui::response_for`
/// rect — so the probe converts nothing.
#[derive(Debug)]
pub(crate) struct BreakerProbe<'a> {
    /// The live scribble, or `None` when no gesture is in flight. The `Option` is the liveness: [`BreakerUI::probe`](crate::gui::pane::graph::gesture::breaker::BreakerUI::probe) hands out the
    /// buffers only while its slot is latched, so a stale scribble left over
    /// from the last gesture is unreachable rather than merely ignored.
    live: Option<&'a mut Scribble>,
}

impl<'a> BreakerProbe<'a> {
    /// A probe over `live`, the scribble in flight, or none.
    pub(super) const fn new(live: Option<&'a mut Scribble>) -> Self {
        Self { live }
    }

    /// True if a breaker gesture is live this frame. Wire-fade emphasis and
    /// similar ambient state read this instead of reaching into the scribble
    /// directly.
    pub(crate) const fn is_active(&self) -> bool {
        self.live.is_some()
    }

    /// True if the active breaker polyline crosses `wire`. A no-op (false)
    /// when no breaker gesture is live, so wire renderers can call it
    /// unconditionally before deciding whether to record a cut.
    pub(crate) fn crosses_wire(&self, wire: &Wire) -> bool {
        self.live
            .as_deref()
            .is_some_and(|s| s.intersects_cubic(wire.p0, wire.p1, wire.p2, wire.p3))
    }

    /// True if the active breaker polyline crosses `rect`. A no-op (false)
    /// when no breaker gesture is live.
    pub(crate) fn crosses_rect(&self, rect: Rect) -> bool {
        self.live
            .as_deref()
            .is_some_and(|s| s.intersects_rect(rect))
    }

    /// Record `addr`'s input binding as targeted by the breaker this frame.
    /// Call only after a `crosses_*` check returned true for it — asserts a
    /// gesture is live, so the three `mark_broken_*` siblings are the one
    /// place that invariant is spelled out, instead of a copy-pasted
    /// `unwrap` at each of the three call sites.
    pub(crate) fn mark_broken_input(&mut self, addr: InputPort) {
        self.live_scribble().broken.push(addr);
    }

    /// Record `id`'s node body as targeted by the breaker this frame.
    pub(crate) fn mark_broken_node(&mut self, id: NodeId) {
        self.live_scribble().broken_nodes.push(id);
    }

    /// Record `s`'s event wire as targeted by the breaker this frame.
    pub(crate) fn mark_broken_subscription(&mut self, s: Subscription) {
        self.live_scribble().broken_subscriptions.push(s);
    }

    fn live_scribble(&mut self) -> &mut Scribble {
        self.live
            .as_deref_mut()
            .expect("mark_broken_* called with no live breaker gesture")
    }
}
