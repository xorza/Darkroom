//! The canvas camera's held input, and the undo gesture each run of it is.

use std::time::Duration;

use glam::Vec2;

use crate::core::edit::gesture_id::GestureId;
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
    pub(crate) fn in_flight(&self) -> bool {
        !self.pan.is_idle()
    }

    /// Latch a pan drag at the camera's current `pan`, as a new gesture.
    pub(crate) fn latch_pan(&mut self, pan: Vec2, out: &mut Requests) {
        self.pan.latch(PanAnchor {
            start: pan,
            gesture: out.open_gesture(),
        });
    }

    /// Fold a live pan drag into `pan`, and return its gesture while it is
    /// held: `anchor + delta`, or, for a missing delta after a latch, the
    /// release that drops the anchor. A call before anything latched does
    /// nothing at all.
    ///
    /// Measured from the latch rather than integrated per frame, so a pan
    /// lands exactly where the pointer says however many frames it took (no
    /// per-frame rounding drift).
    pub(crate) fn fold_pan(&mut self, delta: Option<Vec2>, pan: &mut Vec2) -> Option<GestureId> {
        let &anchor = self.pan.get()?;
        let Some(delta) = delta else {
            self.pan.clear();
            return None;
        };
        *pan = anchor.start + delta;
        Some(anchor.gesture)
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
mod tests {
    use std::time::Duration;

    use glam::Vec2;

    use crate::gui::pane::graph::gesture::pan_zoom::camera_gesture::{
        CameraGesture, SCROLL_RUN_GAP,
    };
    use crate::gui::requests::Requests;

    /// The pan gesture's three edges: an unlatched camera ignores everything,
    /// a latched one measures from the latch rather than integrating and
    /// names one gesture throughout, and a `None` delta is the release that
    /// ends it.
    #[test]
    fn a_pan_drag_measures_from_its_latch_and_releases_once() {
        let mut out = Requests::default();
        let mut camera = CameraGesture::default();

        // Before any latch, a delta drives nothing — `emit_pan_zoom` calls in
        // every frame, most of them with no gesture in flight.
        let mut unlatched = Vec2::ZERO;
        assert_eq!(
            camera.fold_pan(Some(Vec2::new(99.0, 99.0)), &mut unlatched),
            None
        );
        assert_eq!(unlatched, Vec2::ZERO, "an idle camera cannot pan");

        let start = Vec2::new(100.0, 40.0);
        camera.latch_pan(start, &mut out);
        let mut pan = start;

        // Frame 1: start + delta.
        let first = camera.fold_pan(Some(Vec2::new(10.0, -5.0)), &mut pan);
        assert_eq!(pan, Vec2::new(110.0, 35.0), "start + delta");

        // Frame 2, larger travel: measured from the *latch*, so this is
        // start + the new total (130, 28), not frame 1's result + the new
        // delta (140, 23) that integrating would give.
        let second = camera.fold_pan(Some(Vec2::new(30.0, -12.0)), &mut pan);
        assert_eq!(pan, Vec2::new(130.0, 28.0), "start + total, not integrated");
        assert!(first.is_some(), "a held pan names its gesture");
        assert_eq!(first, second, "every frame of one pan is one gesture");

        // Release, then a stray delta: the anchor is gone, so nothing moves.
        assert_eq!(camera.fold_pan(None, &mut pan), None);
        assert!(camera.pan.is_idle(), "a None delta ends the gesture");
        let mut after = pan;
        assert_eq!(camera.fold_pan(Some(Vec2::new(5.0, 5.0)), &mut after), None);
        assert_eq!(after, pan, "a released anchor drives nothing");

        // The next press is another gesture.
        camera.latch_pan(pan, &mut out);
        let next = camera.fold_pan(Some(Vec2::ONE), &mut pan);
        assert!(next.is_some());
        assert_ne!(next, first, "two pans are two gestures");
    }

    /// A scroll run lasts while its frames are at most `SCROLL_RUN_GAP`
    /// apart, measured from the last frame rather than the first, and a
    /// longer pause starts a new one.
    #[test]
    fn a_scroll_run_ends_at_a_pause_longer_than_the_gap() {
        let mut out = Requests::default();
        let mut camera = CameraGesture::default();
        let ms = Duration::from_millis;

        let run = camera.scroll_frame(ms(1_000), &mut out);
        // Frames 400 ms apart chain past the gap measured from the first.
        assert_eq!(camera.scroll_frame(ms(1_400), &mut out), run);
        assert_eq!(camera.scroll_frame(ms(1_800), &mut out), run);
        // Exactly the gap is still the same run.
        assert_eq!(
            camera.scroll_frame(ms(1_800) + SCROLL_RUN_GAP, &mut out),
            run
        );
        // One millisecond more than the gap is a new one.
        let next = camera.scroll_frame(ms(2_301) + SCROLL_RUN_GAP, &mut out);
        assert_ne!(next, run);
        assert_eq!(
            camera.scroll_frame(ms(2_301) + SCROLL_RUN_GAP, &mut out),
            next
        );

        // A reset ends the run whatever the clock says.
        camera.reset();
        assert_ne!(
            camera.scroll_frame(ms(2_301) + SCROLL_RUN_GAP, &mut out),
            next
        );
    }
}
