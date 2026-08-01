use glam::Vec2;
use palantir::{LineCap, LineJoin, PointerButton, PolylineColors, Rect, Shape, Ui};
use scenarium::NodeId;
use scenarium::{InputPort, Subscription};

use crate::core::edit::intent::types::GraphIntent;
use crate::gui::pane::graph::ctx::CanvasCtx;
use crate::gui::pane::graph::gesture::slot::GestureSlot;
use crate::gui::pane::graph::paint::wire::Wire;
use crate::gui::pane::graph::{CanvasGesture, outer_canvas_widget_id, to_world};
use crate::gui::requests::Requests;
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
    /// The live scribble, or `None` when no gesture is in flight on this
    /// pane. The `Option` is the liveness: [`BreakerUI::probe`] hands out the
    /// buffers only while its slot is latched, so a stale scribble left over
    /// from the last gesture is unreachable rather than merely ignored.
    live: Option<&'a mut Scribble>,
}

impl BreakerProbe<'_> {
    /// True if a breaker gesture is live this frame. Wire-fade emphasis and
    /// similar ambient state read this instead of reaching into the scribble
    /// directly.
    pub(crate) fn is_active(&self) -> bool {
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
    pub(super) fn mark_broken_input(&mut self, addr: InputPort) {
        self.live_scribble().broken.push(addr);
    }

    /// Record `id`'s node body as targeted by the breaker this frame.
    pub(crate) fn mark_broken_node(&mut self, id: NodeId) {
        self.live_scribble().broken_nodes.push(id);
    }

    /// Record `s`'s event wire as targeted by the breaker this frame.
    pub(super) fn mark_broken_subscription(&mut self, s: Subscription) {
        self.live_scribble().broken_subscriptions.push(s);
    }

    fn live_scribble(&mut self) -> &mut Scribble {
        self.live
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

/// The breaker scribble: its samples, how far they run, and the three target
/// sets a frame's probing marks against them. Lives in inner-canvas world
/// (pre-transform) coords so render inside the inner canvas can use the points
/// verbatim and intersection tests share the same frame as the cubic bezier
/// endpoints.
///
/// **Buffers, not gesture state.** This sits beside [`BreakerUI`]'s slot
/// rather than inside it, so a gesture that ends returns four allocations to
/// the *next* one instead of to the allocator — the same bargain
/// `SelectionUI::swept` and `NodeUI::row_tracks` already make. What makes that
/// safe is the slot: [`BreakerUI::probe`] only hands these out while it is
/// latched, so between gestures the leftovers are unreachable rather than
/// merely stale. [`Self::restart`] is where a fresh gesture claims them.
///
/// The three `broken_*` collections are each filled by one render pass's
/// hit-test (via `BreakerProbe::mark_broken_*`) and drained by
/// `BreakerUI::apply` on release into the matching severing `GraphIntent`. All
/// three are cleared together at the start of every frame's probing
/// (`begin_frame`, called from `BreakerUI::probe`) rather than each
/// renderer clearing its own — every render pass visits its own targets at
/// most once per frame, so within-frame duplicates aren't possible either
/// way.
#[derive(Default, Debug)]
pub(super) struct Scribble {
    points: Vec<Vec2>,
    length: f32,
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

impl Scribble {
    /// Begin a fresh scribble at `p`, keeping every buffer's capacity. The
    /// one place a gesture's leftovers are dropped, so nothing downstream has
    /// to ask whether what it is reading belongs to this gesture or the last.
    pub(super) fn restart(&mut self, p: Vec2) {
        self.points.clear();
        self.points.push(p);
        self.length = 0.0;
        self.begin_frame();
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
/// the outer canvas). Hands out a `BreakerProbe` to the canvas record so node
/// and connection draws can flag intersections inline.
///
/// Split in two by what each half *is*, not by when it lives: the slot holds
/// the gesture — present or absent, cancelled by a single `clear` — while the
/// buffers it fills stay put and are reclaimed by the next scribble. Nothing
/// in the slot allocates, which is what lets it stay the same plain
/// [`GestureSlot`] every other controller uses.
#[derive(Default, Debug)]
pub(crate) struct BreakerUI {
    /// The button that latched the live gesture — the whole of its identity,
    /// and what the release check polls: a Ctrl+LMB-launched breaker must
    /// keep reading Left, not Right. Empty when no scribble is in flight,
    /// which is also what makes [`Self::scribble`]'s contents meaningless.
    latched: GestureSlot<PointerButton>,
    scribble: Scribble,
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
    pub(crate) fn apply(&mut self, ui: &mut Ui, cx: CanvasCtx<'_>, out: &mut Requests) {
        let graph_ctx = cx.graph_ctx();
        let resp = ui.response_for(outer_canvas_widget_id());
        // The classifier resolves RMB-drag vs Ctrl+LMB-drag and hands back
        // the latching button, which the gesture polls for continuation.
        if let Some(CanvasGesture::Breaker(button)) = cx.gesture()
            && self.latched.is_idle()
            && let Some(p) = resp.pointer_local
        {
            self.latched.latch(button);
            self.scribble.restart(to_world(p, &graph_ctx.viewport()));
        }
        if cx.cancelled() {
            self.latched.clear();
            return;
        }
        // Copied out, so the slot's borrow ends before the scribble below is
        // touched. No gesture latched is the whole of the "nothing to do"
        // case, which is why it returns here rather than falling through a
        // catch-all arm.
        let Some(button) = self.latched.get().copied() else {
            return;
        };
        // Past that guard the scribble *is* this gesture's, so it needs no
        // liveness check of its own.
        let scribble = &mut self.scribble;
        if resp.button(button).drag.delta().is_some() {
            if let Some(p) = resp.pointer_local {
                scribble.add_point(to_world(p, &graph_ctx.viewport()));
            }
            return;
        }
        // Released: drain each target set into its severing intent. Drained
        // rather than moved out — `broken_nodes` is read while the other two
        // drain, and taking it would hand its allocation to the intent
        // instead of to the next gesture.
        let Scribble {
            broken,
            broken_nodes,
            broken_subscriptions,
            ..
        } = scribble;
        out.extend_graph(
            broken_nodes
                .iter()
                .map(|&node_id| GraphIntent::RemoveNode { node_id }),
        );
        for addr in broken.drain(..) {
            if broken_nodes.contains(&addr.node_id) {
                continue;
            }
            out.push_graph(GraphIntent::SetInput {
                input: addr,
                to: None,
            });
        }
        // A removed node already drops its subscriptions
        // (RemoveNode's undo step captures every edge touching
        // it), so skip any whose emitter or subscriber is doomed
        // to avoid redundant history.
        for s in broken_subscriptions.drain(..) {
            if broken_nodes.contains(&s.emitter) || broken_nodes.contains(&s.subscriber) {
                continue;
            }
            out.push_graph(GraphIntent::SetSubscription {
                emitter: s.emitter,
                event_idx: s.event_idx,
                subscriber: s.subscriber,
                subscribe: false,
            });
        }
        broken_nodes.clear();
        self.latched.clear();
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
        if self.latched.is_idle() {
            return BreakerProbe { live: None };
        }
        self.scribble.begin_frame();
        BreakerProbe {
            live: Some(&mut self.scribble),
        }
    }

    /// Paint the polyline. No-op when no gesture is active or the
    /// polyline has < 2 samples (a `restart` with no `add_point`).
    pub(crate) fn draw(&self, ui: &mut Ui, theme: &Theme) {
        if self.latched.is_idle() || self.scribble.points.len() < 2 {
            return;
        }
        ui.add_shape(
            Shape::polyline(
                &self.scribble.points,
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
