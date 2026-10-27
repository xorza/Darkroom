//! The canvas camera's held input.

use common::FloatExt;
use glam::Vec2;
use palantir::Ui;

use crate::core::document::viewport::Viewport;
use crate::core::edit::graph_intent::GraphIntent;
use crate::gui::frame::requests::Requests;
use crate::gui::pane::graph_ui::canvas;
use crate::gui::pane::graph_ui::gesture::pan_zoom;
use crate::gui::pane::graph_ui::prepass_ctx::CanvasGesture;
use crate::gui::pane::graph_ui::prepass_ctx::PrepassCtx;
use crate::gui::widgets::pan_drag;

/// The camera's held input: a middle-button pan, from its press to its release. Wheel, touchpad
/// and pinch input has no press to hold. The camera is outside the undo history, so a pan is no
/// undo gesture: `darkroom/AGENTS.md`, "Undo", "The camera is outside the undo history".
#[derive(Debug, Default)]
pub(crate) struct CameraGesture {
    /// The camera's pan when a held pan latched.
    pan: Option<Vec2>,
}

impl CameraGesture {
    /// Drop the input in flight.
    pub(crate) const fn reset(&mut self) {
        self.pan = None;
    }

    /// Whether a pan drag is latched. Scroll input has no press to cancel, so
    /// it never counts.
    pub(crate) const fn in_flight(&self) -> bool {
        self.pan.is_some()
    }

    /// Read the outer canvas's current-frame response, compute the target
    /// viewport, and emit a `GraphIntent::SetViewport` when it changed: the
    /// intent, not a direct write, is the only thing that moves the document's
    /// camera. Three independent sources:
    ///
    /// - **Middle-button drag** (`Sense::DRAG` + `Ui::drag_delta_by`): canvas
    ///   pan, `anchor + delta` from the latch until release. Left-drag stays with
    ///   rubber-band selection.
    /// - **Scroll** (`Sense::SCROLL`): a two-finger touchpad swipe pans, and a
    ///   mouse wheel zooms about the cursor (graph-editor convention: Figma /
    ///   Blender node editor / `ComfyUI`). Palantir ingests the scroll delta
    ///   already negated, so `+y` means "scroll content down" → zoom out, and
    ///   `-y` (wheel up) → zoom in.
    /// - **Pinch** (`Sense::PINCH`): zoom about the cursor, with the
    ///   `Response::pointer_local` pivot.
    pub(crate) fn prepass(&mut self, ui: &Ui, ctx: PrepassCtx<'_>, out: &mut Requests) {
        let viewport = ctx.graph_ctx().viewport();
        if ctx.cancelled()
            && let Some(back) = self.cancel_pan(viewport)
        {
            out.push_graph(back);
            return;
        }
        let resp = ui.response_for(canvas::outer_wid());
        let mut v = viewport;
        // Pan latch comes from the central classification; continuation and
        // wheel/pinch zoom below read the response directly (not arbitration).
        if ctx.input().gesture == Some(CanvasGesture::Pan) {
            self.pan = Some(viewport.pan);
        }
        pan_drag::fold(&mut self.pan, resp.middle.drag.delta(), &mut v.pan);
        pan_zoom::CANVAS_ZOOM.fold_scroll(&mut v, ui, &resp);
        // Only emit when the gesture actually moved the viewport (approx
        // compare — exact float `!=` would emit on the last-bit noise the
        // zoom-about-cursor algebra leaves behind, which `approximately_eq`
        // reads at the pan's own magnitude rather than against a fixed
        // epsilon a four-digit pan would swamp).
        let unchanged =
            v.pan.approximately_eq(viewport.pan) && v.zoom.approximately_eq(viewport.zoom);
        if !unchanged {
            out.push_graph(GraphIntent::SetViewport { to: v });
        }
    }

    /// Drop the pan drag in flight and put the camera back where it latched, at `viewport`'s
    /// zoom, as Esc does; `None` with no pan in flight.
    fn cancel_pan(&mut self, viewport: Viewport) -> Option<GraphIntent> {
        let start = self.pan.take()?;
        Some(GraphIntent::SetViewport {
            to: Viewport {
                pan: start,
                ..viewport
            },
        })
    }
}

#[cfg(test)]
mod tests;
