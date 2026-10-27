use glam::Vec2;

use crate::core::document::viewport::Viewport;
use crate::core::edit::graph_intent::GraphIntent;
use crate::gui::pane::graph_ui::gesture::pan_zoom::camera_gesture::CameraGesture;
use crate::gui::widgets::pan_drag;

/// Esc on a held pan puts the camera back where the pan latched, at the zoom it has now, and
/// ends the pan; with no pan in flight it does nothing.
#[test]
fn esc_puts_a_held_pan_back_where_it_latched() {
    let mut camera = CameraGesture::default();
    let now = Viewport {
        pan: Vec2::new(130.0, 28.0),
        zoom: 2.0,
    };
    assert!(camera.cancel_pan(now).is_none(), "nothing to cancel");

    let start = Vec2::new(100.0, 40.0);
    camera.pan = Some(start);
    match camera.cancel_pan(now) {
        Some(GraphIntent::SetViewport { to }) => {
            assert_eq!(
                to,
                Viewport {
                    pan: start,
                    zoom: 2.0
                }
            );
        }
        other => panic!("{other:?}"),
    }
    assert!(camera.pan.is_none(), "the pan is over");
}

/// A pan's edges: an unlatched camera ignores a delta, a latched one measures from the latch
/// rather than integrating, and a `None` delta is the release that ends it.
#[test]
fn a_pan_drag_measures_from_its_latch_and_releases_once() {
    let mut camera = CameraGesture::default();
    let mut unlatched = Vec2::ZERO;
    pan_drag::fold(&mut camera.pan, Some(Vec2::new(99.0, 99.0)), &mut unlatched);
    assert_eq!(unlatched, Vec2::ZERO, "an idle camera cannot pan");

    let start = Vec2::new(100.0, 40.0);
    camera.pan = Some(start);
    let mut pan = start;
    pan_drag::fold(&mut camera.pan, Some(Vec2::new(10.0, -5.0)), &mut pan);
    assert_eq!(pan, Vec2::new(110.0, 35.0), "start + delta");
    // Measured from the latch: start + the new total (130, 28), not the last frame's result
    // plus the new delta (140, 23) that integrating would give.
    pan_drag::fold(&mut camera.pan, Some(Vec2::new(30.0, -12.0)), &mut pan);
    assert_eq!(pan, Vec2::new(130.0, 28.0), "start + total, not integrated");

    pan_drag::fold(&mut camera.pan, None, &mut pan);
    assert!(camera.pan.is_none(), "a None delta ends the pan");
    let mut after = pan;
    pan_drag::fold(&mut camera.pan, Some(Vec2::new(5.0, 5.0)), &mut after);
    assert_eq!(after, pan, "a released anchor drives nothing");
}
