//! The canvas camera's held input, and the undo gesture each run of it is.

use std::time::Duration;

use glam::Vec2;

use crate::core::edit::gesture_id::GestureId;
use crate::gui::pane::graph::gesture::pan_zoom;
use crate::gui::pane::graph::gesture::slot::GestureSlot;
use crate::gui::requests::Requests;

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

    /// Latch a pan drag at the camera's current `pan`, as a new gesture.
    pub(crate) fn latch_pan(&mut self, pan: Vec2, out: &mut Requests) {
        self.pan.latch(PanAnchor {
            start: pan,
            gesture: out.open_gesture(),
        });
    }

    /// Fold a live pan drag into `pan` — see [`pan_zoom::fold_pan_drag`] —
    /// and return its gesture while it is held.
    pub(crate) fn fold_pan(&mut self, delta: Option<Vec2>, pan: &mut Vec2) -> Option<GestureId> {
        pan_zoom::fold_pan_drag(&mut self.pan, |anchor| anchor.start, delta, pan)
            .map(|anchor| anchor.gesture)
    }

    /// The gesture a frame of wheel, touchpad or pinch input at `now` belongs
    /// to: the run in flight if it moved the camera at most [`SCROLL_RUN_GAP`]
    /// ago, else a new run.
    pub(crate) fn scroll_frame(&mut self, now: Duration, out: &mut Requests) -> GestureId {
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
