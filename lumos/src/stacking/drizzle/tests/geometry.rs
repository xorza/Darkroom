use super::*;

/// Test sgarea with a horizontal segment from (0,0.5) to (1,0.5).
///
/// This is a left-to-right segment at y=0.5 across the full unit square.
/// Case A (both y in [0,1]): trapezoid = 0.5 * (1-0) * (0.5+0.5) = 0.5
#[test]
fn sgarea_horizontal_midpoint() {
    let area = sgarea(DVec2::new(0.0, 0.5), DVec2::new(1.0, 0.5));
    assert!((area - 0.5).abs() < 1e-12, "Expected 0.5, got {area}");
}

/// Test sgarea with reversed direction: (1,0.5) to (0,0.5).
///
/// Same segment but right-to-left → negative sign.
/// `sgn_dx` = -1, trapezoid = -0.5 * (1-0) * (0.5+0.5) = -0.5
#[test]
fn sgarea_horizontal_reversed() {
    let area = sgarea(DVec2::new(1.0, 0.5), DVec2::new(0.0, 0.5));
    assert!((area - (-0.5)).abs() < 1e-12, "Expected -0.5, got {area}");
}

/// Test sgarea with a vertical segment (dx=0) → area = 0.
#[test]
fn sgarea_vertical() {
    let area = sgarea(DVec2::new(0.5, 0.0), DVec2::new(0.5, 1.0));
    assert!(
        area.abs() < 1e-12,
        "Vertical segment should have area 0, got {area}"
    );
}

/// Test sgarea with near-vertical segment (dx ≈ 1e-16) → area ≈ 0.
/// Floating-point arithmetic can produce tiny nonzero dx for segments that
/// should be vertical. Without the tolerance check, this would divide by
/// near-zero dx and produce a huge slope, yielding a wrong area.
#[test]
fn sgarea_near_vertical() {
    // Simulate floating-point jitter: x2 = x1 + tiny epsilon
    let area = sgarea(DVec2::new(0.5, 0.0), DVec2::new(0.5 + 1e-16, 1.0));
    assert!(
        area.abs() < 1e-12,
        "Near-vertical segment should have area ~0, got {area}"
    );

    // Negative near-zero dx
    let area = sgarea(DVec2::new(0.5, 0.0), DVec2::new(0.5 - 1e-16, 1.0));
    assert!(
        area.abs() < 1e-12,
        "Near-vertical segment (negative dx) should have area ~0, got {area}"
    );
}

/// Test sgarea with segment entirely outside (x > 1).
#[test]
fn sgarea_outside_right() {
    let area = sgarea(DVec2::new(1.5, 0.0), DVec2::new(2.5, 1.0));
    assert!(
        area.abs() < 1e-12,
        "Outside segment should have area 0, got {area}"
    );
}

/// Test sgarea with segment entirely below y=0.
#[test]
fn sgarea_below_axis() {
    let area = sgarea(DVec2::new(0.0, -1.0), DVec2::new(1.0, -0.5));
    assert!(
        area.abs() < 1e-12,
        "Below-axis segment should have area 0, got {area}"
    );
}

/// Test sgarea with segment entirely above y=1.
///
/// Both y >= 1 → full rectangle: `sgn_dx` * (xhi - xlo) = 1.0 * (1-0) = 1.0
#[test]
fn sgarea_above_top() {
    let area = sgarea(DVec2::new(0.0, 1.5), DVec2::new(1.0, 2.0));
    assert!(
        (area - 1.0).abs() < 1e-12,
        "Above-top segment should give 1.0, got {area}"
    );
}

/// Test sgarea Case A: diagonal from (0,0) to (1,1).
///
/// Segment entirely within [0,1]×[0,1]. Case A trapezoid:
/// 0.5 * (1-0) * (1+0) = 0.5
#[test]
fn sgarea_case_a_diagonal() {
    let area = sgarea(DVec2::new(0.0, 0.0), DVec2::new(1.0, 1.0));
    assert!((area - 0.5).abs() < 1e-12, "Expected 0.5, got {area}");
}

/// Test sgarea Case B: segment enters inside, exits above y=1.
///
/// Segment from (0, 0.5) to (1, 1.5). Slope = 1.
/// Clipped x: [0, 1]. ylo = 0.5, yhi = 1.5.
/// ylo <= 1.0, yhi > 1.0 → Case B.
/// det = 0*1.5 - 0.5*1 = -0.5
/// xtop = (dx + det) / dy = (1 + (-0.5)) / 1 = 0.5
/// area = `sgn_dx` * (0.5*(xtop-xlo)*(1+ylo) + xhi-xtop)
///       = 1 * (0.5*(0.5-0)*(1+0.5) + 1-0.5)
///       = 0.5*0.5*1.5 + 0.5
///       = 0.375 + 0.5 = 0.875
#[test]
fn sgarea_case_b() {
    let area = sgarea(DVec2::new(0.0, 0.5), DVec2::new(1.0, 1.5));
    assert!((area - 0.875).abs() < 1e-12, "Expected 0.875, got {area}");
}

/// Test sgarea Case C: segment enters above y=1, exits inside.
///
/// Segment from (0, 1.5) to (1, 0.5). Slope = -1.
/// Clipped x: [0, 1]. ylo = 1.5, yhi = 0.5.
/// ylo > 1.0 → Case C.
/// det = 0*0.5 - 1.5*1 = -1.5
/// xtop = (dx + det) / dy = (1 + (-1.5)) / (-1) = (-0.5)/(-1) = 0.5
/// area = `sgn_dx` * (0.5*(xhi-xtop)*(1+yhi) + xtop-xlo)
///       = 1 * (0.5*(1-0.5)*(1+0.5) + 0.5-0)
///       = 0.5*0.5*1.5 + 0.5
///       = 0.375 + 0.5 = 0.875
#[test]
fn sgarea_case_c() {
    let area = sgarea(DVec2::new(0.0, 1.5), DVec2::new(1.0, 0.5));
    assert!((area - 0.875).abs() < 1e-12, "Expected 0.875, got {area}");
}

/// Test sgarea with segment crossing y=0 (clip to y >= 0).
///
/// Segment from (0, -0.5) to (1, 0.5). Slope = 1.
/// Clipped x: [0, 1]. ylo = -0.5, yhi = 0.5.
/// ylo < 0 → clip: det = 0*0.5 - (-0.5)*1 = 0.5, `xlo_new` = det/dy = 0.5/1 = 0.5, ylo=0.
/// Now xlo=0.5, ylo=0, xhi=1, yhi=0.5. Case A:
/// 0.5*(1-0.5)*(0.5+0) = 0.5*0.5*0.5 = 0.125
#[test]
fn sgarea_crosses_y_zero() {
    let area = sgarea(DVec2::new(0.0, -0.5), DVec2::new(1.0, 0.5));
    assert!((area - 0.125).abs() < 1e-12, "Expected 0.125, got {area}");
}

/// Test boxer: quadrilateral exactly overlapping output pixel → area = 1.0.
///
/// Quad corners at (0,0), (1,0), (1,1), (0,1). Output pixel (0,0) = [0,1]×[0,1].
/// Perfect overlap → area = 1.0.
#[test]
fn boxer_exact_overlap() {
    let quad = [
        DVec2::new(0.0, 0.0),
        DVec2::new(1.0, 0.0),
        DVec2::new(1.0, 1.0),
        DVec2::new(0.0, 1.0),
    ];
    let area = boxer(DVec2::new(0.0, 0.0), &quad);
    assert!(
        (area - 1.0).abs() < 1e-12,
        "Exact overlap should give area 1.0, got {area}"
    );
}

/// Test boxer: quad shifted right by 0.5 → overlap = 0.5.
///
/// Quad at (0.5,0)→(1.5,0)→(1.5,1)→(0.5,1). Output pixel (0,0) = [0,1]×[0,1].
/// x overlap: [0.5, 1.0] = 0.5, y overlap: [0, 1] = 1.0. Total = 0.5.
#[test]
fn boxer_half_overlap_x() {
    let quad = [
        DVec2::new(0.5, 0.0),
        DVec2::new(1.5, 0.0),
        DVec2::new(1.5, 1.0),
        DVec2::new(0.5, 1.0),
    ];
    let area = boxer(DVec2::new(0.0, 0.0), &quad);
    assert!(
        (area - 0.5).abs() < 1e-12,
        "Half x-overlap should give area 0.5, got {area}"
    );
}

/// Test boxer: quad shifted right 0.5 AND up 0.5 → overlap = 0.25.
///
/// Quad at (0.5,0.5)→(1.5,0.5)→(1.5,1.5)→(0.5,1.5). Output pixel (0,0).
/// x overlap: 0.5, y overlap: 0.5. Total = 0.25.
#[test]
fn boxer_quarter_overlap() {
    let quad = [
        DVec2::new(0.5, 0.5),
        DVec2::new(1.5, 0.5),
        DVec2::new(1.5, 1.5),
        DVec2::new(0.5, 1.5),
    ];
    let area = boxer(DVec2::new(0.0, 0.0), &quad);
    assert!(
        (area - 0.25).abs() < 1e-12,
        "Quarter overlap should give area 0.25, got {area}"
    );
}

/// Test boxer with a different output pixel index.
///
/// Quad at (3,5)→(4,5)→(4,6)→(3,6). Output pixel (3,5) = [3,4]×[5,6].
/// After shifting: [0,1]×[0,1] exactly. Area = 1.0.
#[test]
fn boxer_nonzero_pixel() {
    let quad = [
        DVec2::new(3.0, 5.0),
        DVec2::new(4.0, 5.0),
        DVec2::new(4.0, 6.0),
        DVec2::new(3.0, 6.0),
    ];
    let area = boxer(DVec2::new(3.0, 5.0), &quad);
    assert!(
        (area - 1.0).abs() < 1e-12,
        "Exact overlap at (3,5) should give area 1.0, got {area}"
    );
}

/// Test boxer with no overlap → area = 0.
#[test]
fn boxer_no_overlap() {
    let quad = [
        DVec2::new(5.0, 5.0),
        DVec2::new(6.0, 5.0),
        DVec2::new(6.0, 6.0),
        DVec2::new(5.0, 6.0),
    ];
    let area = boxer(DVec2::new(0.0, 0.0), &quad);
    assert!(
        area.abs() < 1e-12,
        "No overlap should give area 0, got {area}"
    );
}

/// Test boxer with a 45° rotated square.
///
/// Diamond centered at (0.5, 0.5) with vertices at distance 0.5*sqrt(2)/sqrt(2) = 0.5:
/// (0.5, 0), (1, 0.5), (0.5, 1), (0, 0.5).
/// This diamond is inscribed in the unit square, with area = 0.5.
#[test]
fn boxer_rotated_diamond() {
    let quad = [
        DVec2::new(0.5, 0.0),
        DVec2::new(1.0, 0.5),
        DVec2::new(0.5, 1.0),
        DVec2::new(0.0, 0.5),
    ];
    let area = boxer(DVec2::new(0.0, 0.0), &quad);
    assert!(
        (area - 0.5).abs() < 1e-12,
        "Diamond inscribed in unit square should have area 0.5, got {area}"
    );
}

/// Test boxer with a rotated rectangle that partially clips output pixel.
///
/// A square rotated 30° centered at (0.5, 0.5) with half-side 0.5.
/// Corners at center ± rotated (±0.5, ±0.5):
///   cos30 = √3/2 ≈ 0.8660, sin30 = 0.5
///   BL: (0.5 + (-0.5*cos30 - (-0.5)*sin30), 0.5 + (-0.5*sin30 + (-0.5)*cos30))
///     = (0.5 + (-0.4330 + 0.25), 0.5 + (-0.25 - 0.4330))
///     = (0.3170, -0.1830)
///   BR: (0.5 + (0.5*cos30 - (-0.5)*sin30), 0.5 + (0.5*sin30 + (-0.5)*cos30))
///     = (0.5 + (0.4330 + 0.25), 0.5 + (0.25 - 0.4330))
///     = (1.1830, 0.3170)
///   TR: (0.5 + (0.5*cos30 - 0.5*sin30), 0.5 + (0.5*sin30 + 0.5*cos30))
///     = (0.5 + (0.4330 - 0.25), 0.5 + (0.25 + 0.4330))
///     = (0.6830, 1.1830)
///   TL: (0.5 + (-0.5*cos30 - 0.5*sin30), 0.5 + (-0.5*sin30 + 0.5*cos30))
///     = (0.5 + (-0.4330 - 0.25), 0.5 + (-0.25 + 0.4330))
///     = (-0.1830, 0.6830)
///
/// The quad area = 1.0 (unit square rotated). The overlap with the unit square
/// [0,1]×[0,1] must be strictly between 0 and 1 since corners extend beyond.
/// By symmetry (30° rotation around center), the overlap should be ~0.933.
/// (Exact: 1 - 2 triangles clipped, each triangle has base 0.183 and height ~0.183*tan60)
///
/// Rather than computing the exact analytical value, we verify:
/// 1) 0 < overlap < 1 (it's a partial clip)
/// 2) overlap is close to the quad area minus the clipped triangles
#[test]
fn boxer_rotated_partial_clip() {
    let cos30 = (PI / 6.0).cos();
    let sin30 = (PI / 6.0).sin();
    let cx = 0.5;
    let cy = 0.5;

    // Rotate (±0.5, ±0.5) by 30° around (cx, cy)
    let corners = [
        (-0.5, -0.5), // BL
        (0.5, -0.5),  // BR
        (0.5, 0.5),   // TR
        (-0.5, 0.5),  // TL
    ];

    let quad: [DVec2; 4] = corners
        .map(|(dx, dy)| DVec2::new(cx + dx * cos30 - dy * sin30, cy + dx * sin30 + dy * cos30));

    let area = boxer(DVec2::new(0.0, 0.0), &quad);

    // The rotated square extends beyond [0,1]×[0,1], so overlap < 1.0
    assert!(
        area < 1.0,
        "30° rotated square should partially clip, got area {area}"
    );
    // But the center is at (0.5,0.5) so most of the area is inside
    assert!(
        area > 0.8,
        "Most of the rotated square should be inside, got area {area}"
    );

    // Verified by Python reference implementation of sgarea/boxer:
    //   Edge 0→1: sgarea(DVec2::new(0.3170, -0.1830), DVec2::new(1.1830, 0.3170)) = 0.038675
    //   Edge 1→2: sgarea(DVec2::new(1.1830, 0.3170), DVec2::new(0.6830, 1.1830)) = -0.278312
    //   Edge 2→3: sgarea(DVec2::new(0.6830, 1.1830), DVec2::new(-0.1830, 0.6830)) = -0.644338
    //   Edge 3→0: sgarea(DVec2::new(-0.1830, 0.6830), DVec2::new(0.3170, -0.1830)) = 0.038675
    //   Sum = -0.845299, abs = 0.845299
    let expected = 0.845_299;
    assert!(
        (area - expected).abs() < 1e-4,
        "Expected overlap ~{expected:.6}, got {area:.6}"
    );
}
