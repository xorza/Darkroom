pub(crate) mod breaker_probe;
pub(crate) mod scribble;

use palantir::widget::{LineCap, LineJoin, Shape};
use palantir::{PointerButton, Stroke, Ui};

use crate::core::edit::graph_intent::GraphIntent;
use crate::gui::pane::graph::canvas::outer_canvas_widget_id;
use crate::gui::pane::graph::ctx::CanvasCtx;
use crate::gui::pane::graph::gesture::breaker::breaker_probe::BreakerProbe;
use crate::gui::pane::graph::gesture::breaker::scribble::Scribble;
use crate::gui::pane::graph::gesture::canvas_gesture::CanvasGesture;
use crate::gui::pane::graph::gesture::slot::GestureSlot;
use crate::gui::requests::Requests;
use crate::gui::theme::Theme;

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
    /// Drop the in-flight scribble. Only the latch is cleared: the point and
    /// broken-target buffers are meaningless without it (see
    /// [`Self::scribble`]) and keep their capacity for the next gesture.
    pub(crate) fn reset(&mut self) {
        self.latched.clear();
    }

    /// Whether a scribble is in flight.
    pub(crate) const fn in_flight(&self) -> bool {
        !self.latched.is_idle()
    }

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
            self.scribble.restart(graph_ctx.viewport().to_world(p));
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
                scribble.add_point(graph_ctx.viewport().to_world(p));
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
        // A removed node already drops its subscriptions (its
        // step captures every edge touching it), so skip any whose
        // emitter or subscriber is doomed to avoid redundant history.
        for s in broken_subscriptions.drain(..) {
            if broken_nodes.contains(&s.emitter) || broken_nodes.contains(&s.subscriber) {
                continue;
            }
            out.push_graph(GraphIntent::SetSubscription {
                subscription: s,
                subscribe: false,
            });
        }
        broken_nodes.clear();
        self.latched.clear();
    }

    /// Hand the active state to the inline intersection consumers (the node
    /// body and both wire hit-tests), or an inert probe when no scribble is in
    /// flight.
    ///
    /// Called once per record pass, so `begin_frame` clears last pass's marks
    /// exactly once before the consumers record this pass's.
    pub(crate) fn probe(&mut self) -> BreakerProbe<'_> {
        if self.latched.is_idle() {
            return BreakerProbe::new(None);
        }
        self.scribble.begin_frame();
        BreakerProbe::new(Some(&mut self.scribble))
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
                Stroke::new(theme.colors.breaker_stroke, theme.stroke_width),
            )
            .cap(LineCap::Round)
            .join(LineJoin::Round),
        );
    }
}

#[cfg(test)]
mod tests;
