//! The canvas camera's held input, and the undo gesture each run of it is.

use std::time::Duration;

use common::FloatExt;
use glam::Vec2;
use palantir::Ui;

use crate::core::edit::gesture_id::GestureId;
use crate::core::edit::graph_intent::GraphIntent;
use crate::gui::frame::requests::Requests;
use crate::gui::pane::graph_ui::canvas;
use crate::gui::pane::graph_ui::gesture::canvas_gesture::CanvasGesture;
use crate::gui::pane::graph_ui::gesture::pan_zoom;
use crate::gui::pane::graph_ui::prepass_ctx::PrepassCtx;
use crate::gui::widgets::gesture_slot::GestureSlot;

/// The longest pause inside one run of wheel, touchpad or pinch input. The
/// notches of one wheel spin arrive tens of milliseconds apart, and a
/// touchpad swipe's momentum events closer still, so a pause of half a second
/// reads as a new intent.
const SCROLL_RUN_GAP: Duration = Duration::from_millis(500);

/// The camera's input in flight, and the undo gesture it folds into.
///
/// A middle-button pan is one gesture from its press to its release. Wheel,
/// touchpad and pinch input has no press, so a run of it is one gesture until
/// it pauses for longer than [`SCROLL_RUN_GAP`].
#[derive(Debug, Default)]
pub(crate) struct CameraGesture {
    pan: GestureSlot<PanAnchor>,
    scroll: Option<ScrollRun>,
}

/// A latched pan drag: the camera's pan when the press latched.
#[derive(Debug, Clone, Copy)]
struct PanAnchor {
    start: Vec2,
    gesture: GestureId,
}

/// A run of wheel, touchpad or pinch input, and when it last moved the camera.
#[derive(Debug, Clone, Copy)]
struct ScrollRun {
    gesture: GestureId,
    last: Duration,
}

impl CameraGesture {
    /// Drop the input in flight.
    pub(crate) fn reset(&mut self) {
        self.pan.clear();
        self.scroll = None;
    }

    /// Whether a pan drag is latched. A scroll run has no press to cancel,
    /// so it never counts.
    pub(crate) const fn in_flight(&self) -> bool {
        !self.pan.is_idle()
    }

    /// Read the outer canvas's current-frame response, compute the target
    /// viewport, and emit a `GraphIntent::SetViewport` when it changed. The
    /// intent (not a direct write) is the only thing that mutates the document's
    /// viewport — so pan/zoom rides the same undo path as every other edit, and
    /// each of this camera's gestures folds into one undo entry. Three independent
    /// sources:
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
    pub(crate) fn emit(&mut self, ui: &Ui, cx: PrepassCtx<'_>, out: &mut Requests) {
        let viewport = cx.graph_ctx().viewport();
        let resp = ui.response_for(canvas::outer_wid());
        let mut v = viewport;
        // Pan latch comes from the central classification; continuation and
        // wheel/pinch zoom below read the response directly (not arbitration).
        if cx.input().gesture == Some(CanvasGesture::Pan) {
            self.latch_pan(viewport.pan, out);
        }
        let held = self.fold_pan(resp.middle.drag.delta(), &mut v.pan);
        pan_zoom::CANVAS_ZOOM.fold_scroll(&mut v, ui, &resp);
        // Only emit when the gesture actually moved the viewport (approx
        // compare — exact float `!=` would emit on the last-bit noise the
        // zoom-about-cursor algebra leaves behind, which `approximately_eq`
        // reads at the pan's own magnitude rather than against a fixed
        // epsilon a four-digit pan would swamp).
        let unchanged =
            v.pan.approximately_eq(viewport.pan) && v.zoom.approximately_eq(viewport.zoom);
        if unchanged {
            return;
        }
        // A frame that both pans and scrolls belongs to the held pan.
        let gesture = held.unwrap_or_else(|| self.scroll_frame(ui.now(), out));
        out.push_graph(GraphIntent::SetViewport {
            to: v,
            gesture: Some(gesture),
        });
    }

    /// Latch a pan drag at the camera's current `pan`, as a new gesture.
    fn latch_pan(&mut self, pan: Vec2, out: &mut Requests) {
        self.pan.latch(PanAnchor {
            start: pan,
            gesture: out.open_gesture(),
        });
    }

    /// Fold a live pan drag into `pan` — see [`GestureSlot::fold_pan`] —
    /// and return its gesture while it is held.
    fn fold_pan(&mut self, delta: Option<Vec2>, pan: &mut Vec2) -> Option<GestureId> {
        self.pan
            .fold_pan(|anchor| anchor.start, delta, pan)
            .map(|anchor| anchor.gesture)
    }

    /// The gesture a frame of wheel, touchpad or pinch input at `now` belongs
    /// to: the run in flight if it moved the camera at most [`SCROLL_RUN_GAP`]
    /// ago, else a new run.
    fn scroll_frame(&mut self, now: Duration, out: &mut Requests) -> GestureId {
        match &mut self.scroll {
            Some(run) if now.saturating_sub(run.last) <= SCROLL_RUN_GAP => {
                run.last = now;
                run.gesture
            }
            _ => {
                let gesture = out.open_gesture();
                self.scroll = Some(ScrollRun { gesture, last: now });
                gesture
            }
        }
    }
}

#[cfg(test)]
mod tests;
