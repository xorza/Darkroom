use glam::DVec2;

use crate::registration::distortion::tps::distortion_map::DistortionMap;
use crate::registration::distortion::tps::tests::{
    EXACT_PX, assert_near, fit_default, square_source_4,
};
use common::{Size2us, Vec2us};

/// `DistortionMap` from pure translation: grid dimensions, vectors, and statistics.
#[test]
fn distortion_map_translation() {
    let source = square_source_4();
    let shift = DVec2::new(5.0, 3.0);
    let target: Vec<DVec2> = source.iter().map(|&p| p + shift).collect();

    let tps = fit_default(&source, &target);
    let map = DistortionMap::from_tps(&tps, Size2us::new(100, 100), 20);

    // Nodes every 20 px and on the last pixel: 0, 20, 40, 60, 80, 99 — none past the frame,
    // where the old rule put its last node at 100.
    assert_eq!(map.grid, Size2us::new(6, 6));
    assert_eq!(map.cols, [0, 20, 40, 60, 80, 99]);
    assert_eq!(map.rows, map.cols);

    // For pure translation, every grid point should have distortion ~(5, 3)
    for gy in 0..map.grid.height {
        for gx in 0..map.grid.width {
            let d = map.get(Vec2us::new(gx, gy)).unwrap();
            assert!(
                d.abs_diff_eq(shift, EXACT_PX),
                "grid ({gx},{gy}): {d} vs {shift}"
            );
        }
    }

    // Mean magnitude should be sqrt(5^2 + 3^2) = sqrt(34) = 5.83095...
    let expected_mag = 34.0_f64.sqrt();
    assert!(
        (map.mean_magnitude - expected_mag).abs() <= EXACT_PX,
        "Mean magnitude: {} expected {}",
        map.mean_magnitude,
        expected_mag
    );

    // Max magnitude should also be ~sqrt(34) for uniform translation
    assert!(
        (map.max_magnitude - expected_mag).abs() <= EXACT_PX,
        "Max magnitude: {} expected {}",
        map.max_magnitude,
        expected_mag
    );
}

/// `DistortionMap::get` returns None for out-of-bounds indices.
#[test]
fn distortion_map_get_out_of_bounds() {
    let source = square_source_4();
    let target: Vec<DVec2> = source.iter().map(|&p| p + DVec2::new(1.0, 1.0)).collect();
    let tps = fit_default(&source, &target);
    let map = DistortionMap::from_tps(&tps, Size2us::new(100, 100), 50);

    // Valid: within grid
    assert!(map.get(Vec2us::ZERO).is_some());

    // Invalid: beyond grid dimensions
    assert!(map.get(Vec2us::new(map.grid.width, 0)).is_none());
    assert!(map.get(Vec2us::new(0, map.grid.height)).is_none());
    assert!(
        map.get(Vec2us::new(map.grid.width, map.grid.height))
            .is_none()
    );
    assert!(map.get(Vec2us::new(1000, 1000)).is_none());
}

/// `DistortionMap::interpolate` with bilinear on translation: exact at grid and mid-points.
#[test]
fn distortion_map_interpolation() {
    let source = square_source_4();
    let shift = DVec2::new(10.0, 5.0);
    let target: Vec<DVec2> = source.iter().map(|&p| p + shift).collect();

    let tps = fit_default(&source, &target);
    let map = DistortionMap::from_tps(&tps, Size2us::new(100, 100), 25);

    // At grid point (0, 0): should be ~(10, 5)
    let d0 = map.interpolate(DVec2::new(0.0, 0.0));
    assert_near(d0, shift, EXACT_PX);

    // At midpoint between grid nodes (37.5, 62.5):
    // For uniform translation, bilinear interpolation of constant field = constant
    let d_mid = map.interpolate(DVec2::new(37.5, 62.5));
    assert_near(d_mid, shift, EXACT_PX);
}

/// `DistortionMap` with non-uniform distortion: verify gradient.
#[test]
fn distortion_map_non_uniform_gradient() {
    let mut source = Vec::new();
    let mut target = Vec::new();

    for y in (0..=200).step_by(50) {
        for x in (0..=200).step_by(50) {
            let s = DVec2::new(f64::from(x), f64::from(y));
            source.push(s);
            // Distortion: dx = 0.05*x, dy = 0.02*y
            // At x=0: dx=0. At x=200: dx=10.
            // At y=0: dy=0. At y=200: dy=4.
            let dx = s.x * 0.05;
            let dy = s.y * 0.02;
            target.push(s + DVec2::new(dx, dy));
        }
    }

    let tps = fit_default(&source, &target);
    let map = DistortionMap::from_tps(&tps, Size2us::new(200, 200), 25);

    // At (25, 100): dx = 25*0.05 = 1.25, dy = 100*0.02 = 2.0
    let d_left = map.interpolate(DVec2::new(25.0, 100.0));
    // At (175, 100): dx = 175*0.05 = 8.75, dy = 100*0.02 = 2.0
    let d_right = map.interpolate(DVec2::new(175.0, 100.0));

    assert!(
        d_right.x > d_left.x,
        "Right side dx ({}) should exceed left ({}) because distortion grows with x",
        d_right.x,
        d_left.x
    );

    // The field is linear, which the spline holds exactly and the map's bilinear interpolation
    // reproduces.
    assert_near(d_left, DVec2::new(1.25, 2.0), EXACT_PX);
    assert_near(d_right, DVec2::new(8.75, 2.0), EXACT_PX);
}

/// Past the grid the distortion is the nearest edge's, not zero: under a uniform shift of (10, 5)
/// the map reads (10, 5) at points before, after and far outside its 100 × 100 grid.
#[test]
fn distortion_map_holds_its_edge_past_the_grid() {
    let source = square_source_4();
    let shift = DVec2::new(10.0, 5.0);
    let target: Vec<DVec2> = source.iter().map(|&p| p + shift).collect();
    let map = DistortionMap::from_tps(&fit_default(&source, &target), Size2us::new(100, 100), 25);
    for p in [
        DVec2::new(-30.0, 50.0),
        DVec2::new(50.0, -30.0),
        DVec2::new(130.0, 50.0),
        DVec2::new(50.0, 1000.0),
        DVec2::new(-1e6, 1e6),
    ] {
        assert!(
            map.interpolate(p).abs_diff_eq(shift, EXACT_PX),
            "{p}: {} vs {shift}",
            map.interpolate(p)
        );
    }
}
