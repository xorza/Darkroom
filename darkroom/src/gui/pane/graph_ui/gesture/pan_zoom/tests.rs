use super::*;
use crate::core::document::internals::DocumentFixture;
use crate::gui::graph_ctx::internals::GraphCtxFixture;

#[test]
fn node_bounds_uses_cached_sizes_and_falls_back_to_points() {
    // Regression for "Show all leaves nodes offscreen": node extents
    // must come from the cross-frame size cache, because a culled
    // (off-screen) node records no response the frame the button is
    // pressed. Three nodes:
    //   a: (0,0) 150×80      — on-screen, size cached
    //   b: (1000,500) 200×100 — culled, but its size is still cached
    //   c: (-50,300) never measured — contributes a point
    let mut fixture = DocumentFixture::default();
    let a = fixture.stub_at(Vec2::new(0.0, 0.0));
    let b = fixture.stub_at(Vec2::new(1000.0, 500.0));
    fixture.stub_at(Vec2::new(-50.0, 300.0));
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
