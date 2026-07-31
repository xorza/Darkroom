use glam::Vec2;
use palantir::{LineCap, LineJoin, PointerButton, PolylineColors, Rect, Shape, Ui};
use scenarium::NodeId;
use scenarium::{InputPort, Subscription};

use crate::core::edit::intent::sink::Intents;
use crate::core::edit::intent::types::GraphIntent;
use crate::gui::pane::graph::ctx::CanvasCtx;
use crate::gui::pane::graph::gesture::slot::GestureSlot;
use crate::gui::pane::graph::paint::wire::Wire;
use crate::gui::pane::graph::{CanvasGesture, outer_canvas_widget_id, to_world};
use crate::gui::theme::Theme;

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
    state: Option<&'a mut BreakerState>,
}

impl BreakerProbe<'_> {
    /// True if a breaker gesture is live this frame. Wire-fade emphasis and
    /// similar ambient state read this instead of reaching into `state`
    /// directly.
    pub(crate) fn is_active(&self) -> bool {
        self.state.is_some()
    }

    /// True if the active breaker polyline crosses `wire`. A no-op (false)
    /// when no breaker gesture is live, so wire renderers can call it
    /// unconditionally before deciding whether to record a cut.
    pub(crate) fn crosses_wire(&self, wire: &Wire) -> bool {
        self.state
            .as_deref()
            .is_some_and(|b| b.intersects_cubic(wire.p0, wire.p1, wire.p2, wire.p3))
    }

    /// True if the active breaker polyline crosses `rect`. A no-op (false)
    /// when no breaker gesture is live.
    pub(crate) fn crosses_rect(&self, rect: Rect) -> bool {
        self.state
            .as_deref()
            .is_some_and(|b| b.intersects_rect(rect))
    }

    /// Record `addr`'s input binding as targeted by the breaker this frame.
    /// Call only after a `crosses_*` check returned true for it — asserts a
    /// gesture is live, so the three `mark_broken_*` siblings are the one
    /// place that invariant is spelled out, instead of a copy-pasted
    /// `unwrap` at each of the three call sites.
    pub(super) fn mark_broken_input(&mut self, addr: InputPort) {
        self.live_state().broken.push(addr);
    }

    /// Record `id`'s node body as targeted by the breaker this frame.
    pub(crate) fn mark_broken_node(&mut self, id: NodeId) {
        self.live_state().broken_nodes.push(id);
    }

    /// Record `s`'s event wire as targeted by the breaker this frame.
    pub(super) fn mark_broken_subscription(&mut self, s: Subscription) {
        self.live_state().broken_subscriptions.push(s);
    }

    fn live_state(&mut self) -> &mut BreakerState {
        self.state
            .as_deref_mut()
            .expect("mark_broken_* called with no live breaker gesture")
    }
}

/// Polyline samples closer than this (in inner-canvas world units)
/// are dropped — keeps the breaker from accumulating sub-pixel
/// duplicates on a slow drag.
const MIN_POINT_DISTANCE: f32 = 4.0;
/// Hard cap on the total polyline length. Once hit, further points
/// stop appending; the last segment is clamped to land exactly on
/// the limit. Matches the deprecated breaker.
const MAX_BREAKER_LENGTH: f32 = 2000.0;
/// Bezier sampling resolution for hit-testing. 16 segments matches
/// the deprecated implementation's `ensure_sampled` density and is
/// cheap enough to redo every frame for every visible connection.
const BEZIER_SAMPLES: usize = 16;

/// Active connection-breaker gesture. Lives in inner-canvas world
/// (pre-transform) coords so render inside the inner canvas can use
/// the points verbatim and intersection tests share the same frame
/// as the cubic bezier endpoints.
///
/// The three `broken_*` collections below are each filled by one render
/// pass's hit-test (via `BreakerProbe::mark_broken_*`) and drained by
/// `BreakerUI::apply` on release into the matching severing `GraphIntent`. All
/// three are cleared together at the start of every frame's probing
/// (`begin_frame`, called from `BreakerUI::probe`) rather than each
/// renderer clearing its own — every render pass visits its own targets at
/// most once per frame, so within-frame duplicates aren't possible either
/// way.
#[derive(Debug)]
pub(super) struct BreakerState {
    points: Vec<Vec2>,
    length: f32,
    /// Mouse button that latched this gesture. The release-detection
    /// check polls `drag_delta_by(button)`, so a Ctrl+LMB-launched
    /// breaker must keep reading the Left button, not Right.
    button: PointerButton,
    /// Target input ports whose data binding the breaker intersects this
    /// frame, drained on release into an unbound `GraphIntent::SetInput`.
    broken: Vec<InputPort>,
    /// Nodes whose body rect the breaker crosses this frame, drained on
    /// release into `GraphIntent::RemoveNode`.
    broken_nodes: Vec<NodeId>,
    /// Event subscriptions whose wire the breaker intersects this frame,
    /// drained on release into `SetSubscription { subscribe: false }`.
    broken_subscriptions: Vec<Subscription>,
}

impl BreakerState {
    pub(crate) fn start(p: Vec2, button: PointerButton) -> Self {
        Self {
            points: vec![p],
            length: 0.0,
            button,
            broken: Vec::new(),
            broken_nodes: Vec::new(),
            broken_subscriptions: Vec::new(),
        }
    }

    /// Clear every `broken_*` collection at the start of a frame's probing.
    /// The single point where this happens — called once from
    /// [`BreakerUI::probe`] — rather than each renderer clearing its own
    /// field, which is easy to forget (and had been forgotten for two of
    /// the three).
    fn begin_frame(&mut self) {
        self.broken.clear();
        self.broken_nodes.clear();
        self.broken_subscriptions.clear();
    }

    pub(super) fn add_point(&mut self, p: Vec2) {
        let last = *self.points.last().unwrap();
        let seg = last.distance(p);
        if seg <= MIN_POINT_DISTANCE {
            return;
        }
        let remaining = MAX_BREAKER_LENGTH - self.length;
        if remaining <= 0.0 {
            return;
        }
        let (clamped, added) = if seg <= remaining {
            (p, seg)
        } else {
            let t = remaining / seg;
            (last + (p - last) * t, remaining)
        };
        self.points.push(clamped);
        self.length += added;
    }

    fn segments(&self) -> impl Iterator<Item = (Vec2, Vec2)> + '_ {
        self.points.windows(2).map(|w| (w[0], w[1]))
    }

    /// True if the breaker polyline crosses `rect`: either any sample
    /// falls inside, or any breaker segment crosses one of the four
    /// edges. `rect` is in the same frame as the polyline (inner-
    /// canvas pre-transform world coords).
    fn intersects_rect(&self, rect: Rect) -> bool {
        if self.points.is_empty() {
            return false;
        }
        let min = rect.min;
        let max = rect.max();
        let inside = |p: Vec2| p.x >= min.x && p.x <= max.x && p.y >= min.y && p.y <= max.y;
        if self.points.iter().any(|&p| inside(p)) {
            return true;
        }
        let edges = [
            (Vec2::new(min.x, min.y), Vec2::new(max.x, min.y)),
            (Vec2::new(max.x, min.y), Vec2::new(max.x, max.y)),
            (Vec2::new(max.x, max.y), Vec2::new(min.x, max.y)),
            (Vec2::new(min.x, max.y), Vec2::new(min.x, min.y)),
        ];
        for (a, b) in self.segments() {
            for &(e0, e1) in &edges {
                if segments_intersect(a, b, e0, e1) {
                    return true;
                }
            }
        }
        false
    }

    /// True if any cubic-bezier sample-segment crosses any breaker
    /// segment. Samples the bezier into `BEZIER_SAMPLES` chords; this
    /// runs once per connection per frame while the gesture is
    /// active, so we don't cache.
    fn intersects_cubic(&self, p0: Vec2, p1: Vec2, p2: Vec2, p3: Vec2) -> bool {
        if self.points.len() < 2 {
            return false;
        }
        let mut prev = p0;
        for i in 1..=BEZIER_SAMPLES {
            let t = i as f32 / BEZIER_SAMPLES as f32;
            let next = cubic_point(p0, p1, p2, p3, t);
            for (b0, b1) in self.segments() {
                if segments_intersect(prev, next, b0, b1) {
                    return true;
                }
            }
            prev = next;
        }
        false
    }
}

fn cubic_point(p0: Vec2, p1: Vec2, p2: Vec2, p3: Vec2, t: f32) -> Vec2 {
    let u = 1.0 - t;
    let uu = u * u;
    let tt = t * t;
    p0 * (uu * u) + p1 * (3.0 * uu * t) + p2 * (3.0 * u * tt) + p3 * (tt * t)
}

/// Standard 2D segment–segment intersection: proper-crossing only
/// (no collinear-overlap), which is enough for "did the breaker
/// scribble cross this wire?".
fn segments_intersect(a1: Vec2, a2: Vec2, b1: Vec2, b2: Vec2) -> bool {
    let o1 = orient(a1, a2, b1);
    let o2 = orient(a1, a2, b2);
    let o3 = orient(b1, b2, a1);
    let o4 = orient(b1, b2, a2);
    (o1 * o2 < 0.0) && (o3 * o4 < 0.0)
}

fn orient(p: Vec2, q: Vec2, r: Vec2) -> f32 {
    (q.x - p.x) * (r.y - p.y) - (q.y - p.y) * (r.x - p.x)
}

/// Owns the active connection-breaker gesture (RMB / Ctrl+LMB drag on
/// the outer canvas). The state is `Option<BreakerState>` rather than
/// flat fields so the absence of a gesture is one variant and the
/// gesture can be cancelled by a single assignment. Hands out a
/// `BreakerProbe` to the canvas record so node and connection draws
/// can flag intersections inline.
#[derive(Default, Debug)]
pub(crate) struct BreakerUI {
    state: GestureSlot<BreakerState>,
}

impl BreakerUI {
    /// Drive the gesture from the outer canvas response: start, extend,
    /// release. On release, drains all three `broken_*` collections into
    /// their matching severing `GraphIntent` (`RemoveNode`, `SetInput { to: None
    /// }`, `SetSubscription { subscribe: false }`).
    /// `RemoveNode` supersedes any per-edge severing on
    /// the same target — the undo step already detaches every incoming
    /// edge and pin, so emitting both would log a redundant history entry.
    /// The context's Esc — resolved once by the canvas — drops the
    /// scribble without emitting.
    pub(crate) fn apply(&mut self, ui: &mut Ui, cx: CanvasCtx<'_>, out: &mut Intents) {
        let graph_ctx = cx.graph_ctx();
        let resp = ui.response_for(outer_canvas_widget_id());
        // The classifier resolves RMB-drag vs Ctrl+LMB-drag and hands back
        // the latching button, which the gesture polls for continuation.
        if let Some(CanvasGesture::Breaker(button)) = cx.gesture()
            && self.state.is_idle()
            && let Some(p) = resp.pointer_local
        {
            self.state.latch(BreakerState::start(
                to_world(p, &graph_ctx.viewport()),
                button,
            ));
        }
        if cx.cancelled() {
            self.state.clear();
            return;
        }
        let button = self.state.get().map(|b| b.button);
        match (
            self.state.get_mut(),
            button.and_then(|b| resp.button(b).drag.delta()),
        ) {
            (Some(b), Some(_)) => {
                if let Some(p) = resp.pointer_local {
                    b.add_point(to_world(p, &graph_ctx.viewport()));
                }
            }
            (Some(b), None) => {
                let doomed_nodes = std::mem::take(&mut b.broken_nodes);
                out.push_node_removals(doomed_nodes.iter().copied());
                for addr in b.broken.drain(..) {
                    if doomed_nodes.contains(&addr.node_id) {
                        continue;
                    }
                    out.push(GraphIntent::SetInput {
                        input: addr,
                        to: None,
                    });
                }
                // A removed node already drops its subscriptions
                // (RemoveNode's undo step captures every edge touching
                // it), so skip any whose emitter or subscriber is doomed
                // to avoid redundant history.
                for s in b.broken_subscriptions.drain(..) {
                    if doomed_nodes.contains(&s.emitter) || doomed_nodes.contains(&s.subscriber) {
                        continue;
                    }
                    out.push(GraphIntent::SetSubscription {
                        emitter: s.emitter,
                        event_idx: s.event_idx,
                        subscriber: s.subscriber,
                        subscribe: false,
                    });
                }
                self.state.clear();
            }
            _ => {}
        }
    }

    /// Hand the active state to this pane's inline intersection consumers
    /// (the node body and both wire hit-tests), or an inert probe when the
    /// scribble belongs to another pane.
    ///
    /// The pane check is load-bearing, not cosmetic. The polyline lives in
    /// its own graph's pre-transform world coordinates, and every pane places
    /// its nodes in its own — so an unscoped probe would test one pane's
    /// scribble against another's rects and mark wires and nodes broken in
    /// a graph the pointer never touched, deleting them on release. It
    /// also keeps `begin_frame` to one call per frame: this runs once per
    /// visible pane, and a second call would clear the marks the owning
    /// pane just recorded.
    pub(crate) fn probe(&mut self) -> BreakerProbe<'_> {
        let mut state = self.state.get_mut();
        if let Some(b) = state.as_deref_mut() {
            b.begin_frame();
        }
        BreakerProbe { state }
    }

    /// Paint the polyline. No-op when no gesture is active or the
    /// polyline has < 2 samples (a `start` with no `add_point`).
    pub(crate) fn draw(&self, ui: &mut Ui, theme: &Theme) {
        let Some(b) = self.state.get() else {
            return;
        };
        if b.points.len() < 2 {
            return;
        }
        ui.add_shape(
            Shape::polyline(
                &b.points,
                PolylineColors::Single(theme.colors.breaker_stroke),
                theme.breaker_stroke_width,
            )
            .cap(LineCap::Round)
            .join(LineJoin::Round),
        );
    }
}

#[cfg(test)]
mod tests;
