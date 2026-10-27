//! [`ButtonStroke`]: a stroke across the bare canvas, held by the button that latched it.

use glam::Vec2;
use palantir::{PointerButton, Ui};

use crate::gui::pane::graph_ui::canvas;
use crate::gui::pane::graph_ui::canvas_ctx::CanvasCtx;

/// A stroke across the bare canvas — the breaker's scribble, the tie line — held by the button
/// whose drag latched it, which the release check polls: a stroke a Ctrl+LMB drag started keeps
/// reading Left, not Right.
#[derive(Debug, Default)]
pub(crate) struct ButtonStroke {
    latched: Option<PointerButton>,
}

/// What one frame does to a stroke, its points in canvas-world coords.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum StrokeStep {
    /// Nothing this frame: no stroke, one Esc dropped, or one held with the pointer off the
    /// surface.
    Idle,
    /// A stroke latched, at this point.
    Started(Vec2),
    /// The held stroke reached this point.
    Moved(Vec2),
    /// The button let go: the stroke is over, and its owner acts on what it crossed.
    Released,
}

impl ButtonStroke {
    /// Drop the stroke in flight.
    pub(crate) const fn reset(&mut self) {
        self.latched = None;
    }

    pub(crate) const fn in_flight(&self) -> bool {
        self.latched.is_some()
    }

    /// Step the stroke one frame off the outer canvas: latch it when idle and `latch` names the
    /// button the canvas's gesture classified for it, follow the pointer while that button
    /// drags, and end on its release. The context's Esc drops it.
    pub(crate) fn step(
        &mut self,
        ui: &Ui,
        ctx: CanvasCtx<'_>,
        latch: Option<PointerButton>,
    ) -> StrokeStep {
        let resp = ui.response_for(canvas::outer_wid());
        let viewport = ctx.graph_ctx().viewport();
        let at = resp.pointer_local.map(|local| viewport.to_world(local));
        if ctx.cancelled() {
            self.latched = None;
            return StrokeStep::Idle;
        }
        if self.latched.is_none()
            && let (Some(button), Some(at)) = (latch, at)
        {
            self.latched = Some(button);
            return StrokeStep::Started(at);
        }
        let Some(button) = self.latched else {
            return StrokeStep::Idle;
        };
        if resp.button(button).drag.delta().is_some() {
            return at.map_or(StrokeStep::Idle, StrokeStep::Moved);
        }
        self.latched = None;
        StrokeStep::Released
    }
}
