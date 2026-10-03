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
    let img = Vec2::new(200.0, 100.0);
    let v = fit_viewport(img, Vec2::new(800.0, 800.0));
    assert_eq!(v.zoom, 4.0);
    assert_eq!(v.pan, Vec2::new(0.0, 200.0));
    assert_eq!(draw_rect(img, v).size, Size::new(800.0, 400.0));
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
