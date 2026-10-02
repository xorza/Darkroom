use imaginarium::Buffer2;

use crate::bit_buffer2::BitBuffer2;
use crate::io::image::cfa::CfaType;
use crate::io::image::cfa::same_color::{
    SameColorMedian, XTRANS_NEIGHBORS, XTRANS_RADIUS, XTransOffsets,
};
use crate::math::size2us::Size2us;
use crate::math::statistics::median_mut;
use crate::math::vec2us::Vec2us;
use crate::testing::cfa::XTRANS_PATTERN;

#[test]
fn bayer_same_color_neighbors() {
    // 6x6 image, all 100.0, hot pixel at center (2,2)
    let mut pixels = vec![100.0; 36];
    // Set some same-color neighbors to distinct values to verify median
    pixels[0] = 50.0; // (0,0)
    pixels[4] = 60.0; // (4,0)
    pixels[2] = 70.0; // (2,0)
    pixels[4 * 6 + 2] = 80.0; // (2,4)

    let pixels = Buffer2::new(6, 6, pixels);
    let result = SameColorMedian::Bayer.at(&pixels, Vec2us::new(2, 2), None);

    // Neighbors: 50, 60, 70, 80, 100 (0,2=100), 100 (4,2=100), 100 (0,4=100), 100 (4,4=100)
    // Sorted: 50, 60, 70, 80, 100, 100, 100, 100 → median of 8 = (80+100)/2 = 90
    assert!(
        (result - 90.0).abs() < f32::EPSILON,
        "Expected 90.0, got {result}"
    );
}

#[test]
fn bayer_same_color_neighbors_corner() {
    // Hot pixel at corner (0,0) in 4x4 Bayer RGGB
    // Same-color (R) neighbors at stride 2: (2,0), (0,2), (2,2)
    let pixels = vec![
        999.0, 10.0, 50.0, 10.0, 10.0, 10.0, 10.0, 10.0, 60.0, 10.0, 70.0, 10.0, 10.0, 10.0, 10.0,
        10.0,
    ];
    let pixels = Buffer2::new(4, 4, pixels);
    let result = SameColorMedian::Bayer.at(&pixels, Vec2us::ZERO, None);

    // Same-color neighbors: (2,0)=50, (0,2)=60, (2,2)=70
    // Median of [50, 60, 70] = 60
    assert!(
        (result - 60.0).abs() < f32::EPSILON,
        "Expected 60.0, got {result}"
    );
}

/// Reference X-Trans same-color median: collect every in-bounds, unmasked same-color neighbour
/// in the radius-6 window, take the closest `XTRANS_NEIGHBORS` by Manhattan distance (ties in
/// scan order), median them. The precomputed [`XTransOffsets`] must reproduce this exactly.
fn brute_force_xtrans_median(pixels: &Buffer2<f32>, pos: Vec2us, pattern: &CfaType) -> f32 {
    let (w, h) = (pixels.width() as i32, pixels.height() as i32);
    let my_color = pattern.color_at(pos);
    let mut cands: Vec<(i32, f32)> = Vec::new();
    for dy in -XTRANS_RADIUS..=XTRANS_RADIUS {
        for dx in -XTRANS_RADIUS..=XTRANS_RADIUS {
            if dx == 0 && dy == 0 {
                continue;
            }
            let (nx, ny) = (pos.x as i32 + dx, pos.y as i32 + dy);
            if nx < 0 || ny < 0 || nx >= w || ny >= h {
                continue;
            }
            if pattern.color_at(Vec2us::new(nx as usize, ny as usize)) == my_color {
                cands.push((dx.abs() + dy.abs(), *pixels.get(nx as usize, ny as usize)));
            }
        }
    }
    cands.sort_by_key(|&(dist, _)| dist);
    let n = cands.len().min(XTRANS_NEIGHBORS);
    let mut vals: Vec<f32> = cands[..n].iter().map(|&(_, v)| v).collect();
    median_mut(&mut vals)
}

/// The precomputed X-Trans offsets must reproduce the brute-force closest-N same-color median at
/// every pixel — including borders (fewer neighbours) and interior (the N-cutoff is exercised).
#[test]
fn xtrans_offsets_match_brute_force() {
    let pattern = CfaType::XTrans(XTRANS_PATTERN);
    // Not a multiple of 6, so all 36 phases hit the borders.
    let size = Size2us::new(29, 23);
    // Deterministic, well-spread values so medians are sensitive to which neighbours are chosen.
    let px: Vec<f32> = (0..size.pixel_count())
        .map(|i| ((i.wrapping_mul(2_654_435_761) >> 8) % 1000) as f32 / 1000.0)
        .collect();
    let pixels = Buffer2::new(size.width, size.height, px);
    let offsets = XTransOffsets::new(&XTRANS_PATTERN);

    for y in 0..size.height {
        for x in 0..size.width {
            let pos = Vec2us::new(x, y);
            let got = offsets.median(&pixels, pos, None);
            let want = brute_force_xtrans_median(&pixels, pos, &pattern);
            assert_eq!(
                got, want,
                "X-Trans median mismatch at ({x},{y}): precomputed {got} vs brute-force {want}"
            );
        }
    }
}

/// X-Trans same-color selection: with each color held at a distinct constant, an interior
/// pixel's same-color median is exactly its own color's value (a wrong-color pick would mix them).
#[test]
fn xtrans_median_selects_same_color() {
    let pattern = CfaType::XTrans(XTRANS_PATTERN);
    let size = Size2us::new(24usize, 24usize);
    let color_val = |c: u8| 0.1 * f32::from(c + 1); // R→0.1, G→0.2, B→0.3
    let px: Vec<f32> = (0..size.pixel_count())
        .map(|i| color_val(pattern.color_at(Vec2us::new(i % size.width, i / size.width))))
        .collect();
    let pixels = Buffer2::new(size.width, size.height, px);
    let neighbors = SameColorMedian::new(&pattern);

    // Interior pixels (≥6 from every border) of each color — all 24 nearest same-color in-bounds.
    for &(x, y) in &[(13usize, 12usize), (12, 12), (14, 13)] {
        let c = pattern.color_at(Vec2us::new(x, y));
        let got = neighbors.at(&pixels, Vec2us::new(x, y), None);
        assert!(
            (got - color_val(c)).abs() < f32::EPSILON,
            "({x},{y}) color {c}: expected {} got {got}",
            color_val(c)
        );
    }
}

/// A defect is never repaired from another defect: neighbours flagged in the mask are skipped.
///
/// Exercised through the Mono arm for its simple 8-connected geometry, but the walk is shared, so
/// this pins the rule for the Bayer and X-Trans strategies too.
#[test]
fn same_color_median_skips_masked_neighbours() {
    // 3x3 with the centre defective. Four neighbours read 10 and four read 1000, so including
    // the high four moves the median from 10 to 505 — a gap no rounding could blur.
    let size = Size2us::new(3, 3);
    let pixels = Buffer2::new(
        3,
        3,
        vec![
            10.0, 10.0, 10.0, 10.0, 999.0, 1000.0, 1000.0, 1000.0, 1000.0,
        ],
    );
    let centre = Vec2us::new(1, 1);

    // Unmasked: median of [10, 10, 10, 10, 1000, 1000, 1000, 1000] = (10 + 1000) / 2.
    let unmasked = SameColorMedian::Mono.at(&pixels, centre, None);
    assert_eq!(unmasked, 505.0);

    // Flag the four high neighbours; only the four 10s survive.
    let mut mask = BitBuffer2::new_default(size);
    for idx in [5, 6, 7, 8] {
        mask.set(idx, true);
    }
    let masked = SameColorMedian::Mono.at(&pixels, centre, Some(&mask));
    assert_eq!(masked, 10.0);

    // Every neighbour flagged leaves nothing to repair from, so the centre pixel stands.
    let mut all = BitBuffer2::new_default(size);
    for idx in [0, 1, 2, 3, 5, 6, 7, 8] {
        all.set(idx, true);
    }
    assert_eq!(SameColorMedian::Mono.at(&pixels, centre, Some(&all)), 999.0);
}
