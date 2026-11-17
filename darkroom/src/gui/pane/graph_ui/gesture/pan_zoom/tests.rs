use palantir::SystemScale;
use scenarium::GraphId;

use super::*;
use crate::core::document::internals::DocumentFixture;
use crate::gui::graph_ctx::internals::GraphCtxFixture;
use crate::gui::pane::graph_ui::internals::CanvasHarness;

#[test]
fn node_bounds_uses_cached_sizes_and_falls_back_to_points() {
    // Regression for "Show all leaves nodes offscreen": node extents must come from the
    // cross-frame size cache, as a culled node records no response the frame the button is
    // pressed. Three nodes:
    //   a: (0,0) 150×80      — on-screen, size cached
    //   b: (1000,500) 200×100 — culled, but its size is still cached
    //   c: (-50,300) never measured — contributes a point
    let mut fixture = DocumentFixture::default();
    let a = fixture.ghost_node_at(Vec2::new(0.0, 0.0));
    let b = fixture.ghost_node_at(Vec2::new(1000.0, 500.0));
    fixture.ghost_node_at(Vec2::new(-50.0, 300.0));
    let scene = GraphCtxFixture::over(fixture);
    let mut geometry = CanvasGeometry::default();
    geometry.seed_node_size(a, Size::new(150.0, 80.0));
    geometry.seed_node_size(b, Size::new(200.0, 100.0));

    // Union: min = c's x / a's y = (-50, 0); max = b's far corner
    // (1000+200, 500+100) = (1200, 600) → size (1250, 600). Without
    // the cache, b would count as a point and max.x would be 1000 —
    // its whole 200×100 body left outside the fit.
    let all = node_bounds(&geometry, scene.graph_ctx(), Bounded::All).unwrap();
    assert_eq!(all.min, Vec2::new(-50.0, 0.0));
    assert_eq!(all.size, Size::new(1250.0, 600.0));

    // `Bounded::Selected` filters to exactly the selected node's rect.
    let scene = scene.with_selection([b]);
    let sel = node_bounds(&geometry, scene.graph_ctx(), Bounded::Selected).unwrap();
    assert_eq!(sel.min, Vec2::new(1000.0, 500.0));
    assert_eq!(sel.size, Size::new(200.0, 100.0));

    // Empty graph → nothing to frame.
    let empty = GraphCtxFixture::over(DocumentFixture::default());
    assert!(node_bounds(&geometry, empty.graph_ctx(), Bounded::All).is_none());
}

/// Fitting puts the bounds' center on the pane's center, at the tighter
/// axis's scale, never past 1:1 and never under the minimum zoom. Every value
/// here is exact in f32, so the asserts are exact too.
#[test]
fn fit_target_centers_at_the_tighter_scale() {
    let pane = Vec2::new(800.0, 600.0);
    let cases = [
        // Margin 40 leaves 720×520: 720/1000 = 0.72 binds before 520/500.
        // pan = (400, 300) - (500, 250)·0.72 = (40, 120).
        (
            Rect::new(0.0, 0.0, 1000.0, 500.0),
            0.72,
            Vec2::new(40.0, 120.0),
        ),
        // It would fit at 5.2×, but stops at 1:1.
        // pan = (400, 300) - (50, 50) = (350, 250).
        (
            Rect::new(0.0, 0.0, 100.0, 100.0),
            1.0,
            Vec2::new(350.0, 250.0),
        ),
        // A zero-size box constrains no axis, so it is 1:1 and centered.
        // pan = (400, 300) - (200, 200) = (200, 100).
        (
            Rect::new(200.0, 200.0, 0.0, 0.0),
            1.0,
            Vec2::new(200.0, 100.0),
        ),
        // The exact fit is under the minimum zoom, so the minimum holds.
        // pan = (400, 300) - (50000, 50000)·0.1 = (-4600, -4700).
        (
            Rect::new(0.0, 0.0, 100_000.0, 100_000.0),
            CANVAS_ZOOM.min,
            Vec2::new(-4600.0, -4700.0),
        ),
    ];
    for (bounds, zoom, pan) in cases {
        let t = fit_target(bounds, pane);
        assert_eq!((t.zoom, t.pan), (zoom, pan), "{bounds:?}");
        assert_eq!(
            t.pan + bounds.center() * t.zoom,
            pane * 0.5,
            "{bounds:?} centered"
        );
    }
}

/// The canvas draws through the camera landed on the display, and keeps the stored one exact. At
/// 2× a stored pan of (10.3, 20.3) draws at (10.5, 20.5): the probe at (40, 40) lands its body at
/// the canvas's corner + (50.5, 60.5), on whole physical px, (101, 121) past it. The context the
/// canvas reads hands that landed camera, and the document still holds (10.3, 20.3).
#[test]
fn the_canvas_draws_a_fractional_pan_on_whole_physical_px() {
    let stored = Viewport {
        pan: Vec2::new(10.3, 20.3),
        zoom: 1.0,
    };
    let mut h = CanvasHarness::scaled(DocumentFixture::probes(1), SystemScale::new(2.0).unwrap());
    let node = h.node(0);
    assert_eq!(
        h.document().main_view.node_placements[&node].pos,
        Vec2::new(40.0, 40.0)
    );
    h.document_mut().main_view.viewport = stored;
    h.prime(2);

    let corner =
        h.ui.rect(canvas::outer_wid(GraphId::Main))
            .expect("the canvas shows")
            .min;
    let body = h.node_rect(node);
    assert_eq!(body.min - corner, Vec2::new(50.5, 60.5));
    assert_eq!((body.min - corner) * 2.0, Vec2::new(101.0, 121.0));
    assert_eq!(
        corner * 2.0,
        (corner * 2.0).round(),
        "the canvas starts on a pixel"
    );

    let display = h.ui.frame_value(|ui| ui.display());
    assert_eq!(
        h.ctx.graph_ctx_on(display).camera(),
        Viewport {
            pan: Vec2::new(10.5, 20.5),
            zoom: 1.0,
        }
    );
    assert_eq!(
        h.document().main_view.viewport,
        stored,
        "the stored camera stays exact"
    );
}

/// A step lands on the next rung of `2^(n/2)`, from a rung and from between two, stops at the
/// range's ends, and holds the content at the pane's center.
#[test]
fn a_zoom_step_lands_on_the_next_rung() {
    use std::f32::consts::{FRAC_1_SQRT_2, SQRT_2};

    let pane = Vec2::new(800.0, 600.0);
    let at = |zoom: f32| Viewport {
        pan: Vec2::new(30.0, -20.0),
        zoom,
    };
    let cases = [
        (1.0, ZoomStep::In, Some(SQRT_2)),
        // Between two rungs: up to the next, not by a factor.
        (1.2, ZoomStep::In, Some(SQRT_2)),
        (1.2, ZoomStep::Out, Some(1.0)),
        (SQRT_2, ZoomStep::Out, Some(1.0)),
        (1.0, ZoomStep::Out, Some(FRAC_1_SQRT_2)),
        // A hair under a rung counts as on it.
        (0.9995, ZoomStep::In, Some(SQRT_2)),
        // The ends of the range close the ladder.
        (4.0, ZoomStep::In, Some(5.0)),
        (5.0, ZoomStep::In, None),
        (0.125, ZoomStep::Out, Some(0.1)),
        (0.1, ZoomStep::Out, None),
    ];
    for (zoom, step, expected) in cases {
        let from = at(zoom);
        let to = step_target(from, step, pane);
        assert_eq!(to.map(|to| to.zoom), expected, "{zoom} {step:?}");
        if let Some(to) = to {
            let center = pane * 0.5;
            let moved = to.to_world(center) - from.to_world(center);
            // f32 pan math over a pane of hundreds of pixels: under a thousandth of a pixel.
            assert!(moved.length() < 1e-3, "{zoom} {step:?}: {moved}");
        }
    }
    // Two opposite steps return.
    let there = step_target(at(1.0), ZoomStep::In, pane).unwrap();
    let back = step_target(there, ZoomStep::Out, pane).unwrap();
    assert_eq!(back.zoom, 1.0);
    assert!((back.pan - at(1.0).pan).length() < 1e-3);
}

/// A drag of the zoom control doubles the zoom per 200 px along `x − y`, about the pane's center:
/// from zoom 1 and pan 0 in an 800 × 600 pane, 200 px right gives zoom 2 and pan
/// (400, 300) − (400, 300) · 2 = (−400, −300); 100 px right and 100 up, the same `x − y` of 200,
/// gives the same; 200 px down halves it, pan (400, 300) − (400, 300) · ½ = (200, 150). Travel
/// past the range stops at its end, 5 for 2^25 and 0.1 for 2^−25.
#[test]
fn a_zoom_scrub_doubles_per_two_hundred_pixels_about_the_center() {
    let pane = Vec2::new(800.0, 600.0);
    let anchor = Viewport {
        pan: Vec2::ZERO,
        zoom: 1.0,
    };
    let scrub = |x: f32, y: f32| scrub_target(anchor, Vec2::new(x, y), pane);
    let doubled = Viewport {
        pan: Vec2::new(-400.0, -300.0),
        zoom: 2.0,
    };
    assert_eq!(scrub(200.0, 0.0), doubled);
    assert_eq!(scrub(100.0, -100.0), doubled);
    assert_eq!(
        scrub(0.0, 200.0),
        Viewport {
            pan: Vec2::new(200.0, 150.0),
            zoom: 0.5,
        }
    );
    assert_eq!(scrub(0.0, 0.0), anchor);
    assert_eq!(scrub(5000.0, 0.0).zoom, 5.0);
    assert_eq!(scrub(-5000.0, 0.0).zoom, 0.1);
}
