pub(crate) mod breaker_probe;
pub(crate) mod scribble;

use palantir::widget::{LineCap, LineJoin, Shape};
use palantir::{Stroke, Ui};

use crate::core::edit::graph_intent::GraphIntent;
use crate::gui::frame::requests::Requests;
use crate::gui::pane::graph_ui::canvas_ctx::CanvasCtx;
use crate::gui::pane::graph_ui::gesture::breaker_ui::breaker_probe::BreakerProbe;
use crate::gui::pane::graph_ui::gesture::breaker_ui::scribble::Scribble;
use crate::gui::pane::graph_ui::gesture::button_stroke::{ButtonStroke, StrokeStep};
use crate::gui::pane::graph_ui::prepass_ctx::CanvasGesture;
use crate::gui::theme::Theme;

/// Owns the active wire-breaker gesture (RMB / Ctrl+LMB drag on
/// the outer canvas). Hands out a `BreakerProbe` to the canvas record so node
/// and wire draws can flag intersections inline.
///
/// Split in two by what each half *is*, not by when it lives: the stroke holds
/// the gesture — present or absent, cancelled by a single reset — while the
/// buffers it fills stay put and are reclaimed by the next scribble, as the tie line's do.
#[derive(Default, Debug)]
pub(crate) struct BreakerUi {
    /// The stroke in flight, the whole of the gesture's identity. Idle when no scribble is in
    /// flight, which is also what makes [`Self::scribble`]'s contents meaningless.
    stroke: ButtonStroke,
    scribble: Scribble,
}

impl BreakerUi {
    /// Drop the in-flight scribble. Only the latch is cleared: the point and
    /// broken-target buffers are meaningless without it (see
    /// [`Self::scribble`]) and keep their capacity for the next gesture.
    pub(crate) const fn reset(&mut self) {
        self.stroke.reset();
    }

    /// Whether a scribble is in flight.
    pub(crate) const fn in_flight(&self) -> bool {
        self.stroke.in_flight()
    }

    /// Drive the gesture from the outer canvas response: start, extend,
    /// release. On release, drains all three `broken_*` collections into
    /// their matching severing `GraphIntent` (`RemoveNode`,
    /// `SetInput { to: None }`, `SetSubscription { to: false }`).
    /// `RemoveNode` supersedes any per-edge severing on the same target — the
    /// undo step already detaches every incoming edge and pin, so emitting
    /// both would log a redundant history entry. The context's Esc — resolved
    /// once by the canvas — drops the scribble without emitting.
    pub(crate) fn apply(&mut self, ui: &Ui, ctx: CanvasCtx<'_>, out: &mut Requests) {
        // The classifier resolves RMB-drag vs Ctrl+LMB-drag and hands back
        // the latching button, which the stroke polls for continuation.
        let latch = match ctx.gesture() {
            Some(CanvasGesture::Breaker(button)) => Some(button),
            _ => None,
        };
        match self.stroke.step(ui, ctx, latch) {
            StrokeStep::Started(at) => self.scribble.restart(at),
            StrokeStep::Moved(at) => self.scribble.add_point(at, ctx.graph_ctx().viewport().zoom),
            StrokeStep::Released => release(&mut self.scribble, out),
            StrokeStep::Idle => {}
        }
    }

    /// Hand the active state to the inline intersection consumers (the node
    /// body and both wire hit-tests), or an inert probe when no scribble is in
    /// flight.
    ///
    /// Called once per record pass, so `begin_frame` clears last pass's marks
    /// exactly once before the consumers record this pass's.
    pub(crate) fn probe(&mut self) -> BreakerProbe<'_> {
        if !self.stroke.in_flight() {
            return BreakerProbe::new(None);
        }
        self.scribble.begin_frame();
        BreakerProbe::new(Some(&mut self.scribble))
    }

    /// Paint the polyline. No-op when no gesture is active or the
    /// polyline has < 2 samples (a `restart` with no `add_point`).
    pub(crate) fn draw(&self, ui: &mut Ui, theme: &Theme, zoom: f32) {
        if !self.stroke.in_flight() || self.scribble.points.len() < 2 {
            return;
        }
        ui.add_shape(
            Shape::polyline(
                &self.scribble.points,
                Stroke::new(theme.canvas.breaker_stroke, theme.stroke.wire_at(zoom)),
            )
            .cap(LineCap::Round)
            .join(LineJoin::Round),
        );
    }
}

/// A released scribble: drain each target set into its severing intent. Drained
/// rather than moved out — `broken_node_ids` is read while the other two
/// drain, and taking it would hand its allocation to the intent
/// instead of to the next gesture.
fn release(scribble: &mut Scribble, out: &mut Requests) {
    let Scribble {
        broken,
        broken_node_ids,
        broken_subscriptions,
        ..
    } = scribble;
    out.extend_graph(
        broken_node_ids
            .iter()
            .map(|&node_id| GraphIntent::RemoveNode { node_id }),
    );
    for addr in broken.drain(..) {
        if broken_node_ids.contains(&addr.node_id) {
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
        if broken_node_ids.contains(&s.event.node_id) || broken_node_ids.contains(&s.subscriber) {
            continue;
        }
        out.push_graph(GraphIntent::SetSubscription {
            subscription: s,
            to: false,
        });
    }
    broken_node_ids.clear();
}

#[cfg(test)]
mod tests;
