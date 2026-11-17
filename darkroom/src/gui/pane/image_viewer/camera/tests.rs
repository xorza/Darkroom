use super::*;

#[test]
fn fit_viewport_centers_and_scales_like_contain() {
    // 400×200 texture in an 800×800 pane: width binds at zoom 2 —
    // Contain upscales. pan = ((800,800) - (800,400)) / 2 = (0, 200).
    let v = fit_viewport(Vec2::new(400.0, 200.0), Vec2::new(800.0, 800.0));
    assert_eq!(v.zoom, 2.0);
    assert_eq!(v.pan, Vec2::new(0.0, 200.0));

    // 4000×2000 in 1000×1000: zoom 0.25, pan = (0, (1000-500)/2) = (0, 250).
    let v = fit_viewport(Vec2::new(4000.0, 2000.0), Vec2::new(1000.0, 1000.0));
    assert_eq!(v.zoom, 0.25);
    assert_eq!(v.pan, Vec2::new(0.0, 250.0));

    // Height-bound case: 200×400 in 800×400 → zoom 1, pan = (300, 0).
    let v = fit_viewport(Vec2::new(200.0, 400.0), Vec2::new(800.0, 400.0));
    assert_eq!(v.zoom, 1.0);
    assert_eq!(v.pan, Vec2::new(300.0, 0.0));

    // The drawn rect covers exactly pan..pan+img*zoom.
    let r = draw_rect(Vec2::new(200.0, 400.0), v);
    assert_eq!(r.min, Vec2::new(300.0, 0.0));
    assert_eq!(r.size, Size::new(200.0, 400.0));

    // The display scale never reaches this function: the viewer hands it an
    // image's logical footprint (`logical_size` divides by the scale), so a
    // 400×200-texel image on a 2x display arrives as 200×100. Fitting that in
    // the 800×800 pane zooms 4x and draws 800×400 logical.
    let image = Vec2::new(200.0, 100.0);
    let v = fit_viewport(image, Vec2::new(800.0, 800.0));
    assert_eq!(v.zoom, 4.0);
    assert_eq!(v.pan, Vec2::new(0.0, 200.0));
    assert_eq!(draw_rect(image, v).size, Size::new(800.0, 400.0));
}

#[test]
fn zoom_about_pane_center_keeps_center_texel() {
    // Start from the fit of 400×200 in an 800×800 pane: zoom 2,
    // pan (0, 200). The texel under the pane center (400, 400) is
    // ((400 - 0)/2, (400 - 200)/2) = (200, 100) — the image center.
    let fit = fit_viewport(Vec2::new(400.0, 200.0), Vec2::new(800.0, 800.0));
    assert_eq!(fit.zoom, 2.0);

    // Zoom to 100%: pan' = center - texel·1 = (400-200, 400-100).
    let pane = Vec2::new(800.0, 800.0);
    let v = zoom_about_pane_center(fit, 1.0, pane);
    assert_eq!(v.zoom, 1.0);
    assert_eq!(v.pan, Vec2::new(200.0, 300.0));

    // The invariant holds for an arbitrary target too: zoom 4 →
    // pan' = center - texel·4 = (400-800, 400-400) = (-400, 0).
    let v = zoom_about_pane_center(fit, 4.0, pane);
    assert_eq!(v.zoom, 4.0);
    assert_eq!(v.pan, Vec2::new(-400.0, 0.0));

    // A 200×100 logical footprint (a 400×200-texel image on a 2x display)
    // at zoom 1 draws 200×100 logical — one texel per physical pixel — about
    // the pane center: pan = 400 − 100, 400 − 50.
    let img_2x = Vec2::new(200.0, 100.0);
    let fit_2x = fit_viewport(img_2x, Vec2::new(800.0, 800.0));
    let v = zoom_about_pane_center(fit_2x, 1.0, pane);
    assert_eq!(v.pan, Vec2::new(300.0, 350.0));
    assert_eq!(draw_rect(img_2x, v).size, Size::new(200.0, 100.0));
}

/// A 400 × 200 source painted from a 100 × 50 texture: at zoom 2 from (10, 20) the image spans
/// (10, 20) to (210, 120), 2 pane points a source pixel; at zoom 0.5 from the origin, 0 to
/// (50, 25), 8 source pixels a point. Each corner names its corner pixel, and a point past an
/// edge none.
#[test]
fn a_pane_point_names_the_source_pixel_under_it_at_any_zoom() {
    let (image, native) = (Vec2::new(100.0, 50.0), UVec2::new(400, 200));
    let pixel =
        |at: Vec2, v: Viewport| native_pixel(at, image, v, native).map(|pos| pos.to_array());
    let near = Viewport {
        pan: Vec2::new(10.0, 20.0),
        zoom: 2.0,
    };
    assert_eq!(pixel(Vec2::new(10.0, 20.0), near), Some([0, 0]));
    // (209.5 − 10) / 200 · 400 = 399, (119.5 − 20) / 100 · 200 = 199.
    assert_eq!(pixel(Vec2::new(209.5, 119.5), near), Some([399, 199]));
    assert_eq!(pixel(Vec2::new(209.5, 20.0), near), Some([399, 0]));
    // (11.5 − 10) / 200 · 400 = 3, (23.5 − 20) / 100 · 200 = 7.
    assert_eq!(pixel(Vec2::new(11.5, 23.5), near), Some([3, 7]));
    for past in [
        Vec2::new(210.0, 20.0),
        Vec2::new(10.0, 120.0),
        Vec2::new(9.5, 50.0),
        Vec2::new(50.0, 19.5),
    ] {
        assert_eq!(pixel(past, near), None, "{past}");
    }
    let far = Viewport {
        pan: Vec2::ZERO,
        zoom: 0.5,
    };
    // 25 / 50 · 400 = 200, 12.5 / 25 · 200 = 100.
    assert_eq!(pixel(Vec2::new(25.0, 12.5), far), Some([200, 100]));
    // 49.875 / 50 · 400 = 399, 24.875 / 25 · 200 = 199.
    assert_eq!(pixel(Vec2::new(49.875, 24.875), far), Some([399, 199]));
    assert_eq!(pixel(Vec2::new(50.0, 0.0), far), None);
}
