use std::time::Duration;

use glam::Vec2;

use crate::gui::pane::graph::gesture::pan_zoom::camera_gesture::{CameraGesture, SCROLL_RUN_GAP};
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
