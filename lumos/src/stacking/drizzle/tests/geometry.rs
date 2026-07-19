use crate::stacking::drizzle::tests::*;

#[test]
fn test_drizzle_accumulator_rejects_invalid_frame_inputs() {
    let config = DrizzleConfig::x2();
    let mut acc = accumulator(ImageDimensions::new((4, 4), 1), config);

    let mut frame = DrizzleFrame::new(
        AstroImage::from_pixels(ImageDimensions::new((4, 4), 1), vec![1.0; 16]),
        Transform::identity(),
    );
    frame.pixel_weight_map = Some(Buffer2::new_filled(3, 3, 1.0));
    let error = acc.add_frame(frame).unwrap_err();
    assert!(matches!(
        error,
        DrizzleError::PixelWeightDimensionMismatch {
            index: 0,
            expected_width: 4,
            expected_height: 4,
            actual_width: 3,
            actual_height: 3,
        }
    ));

    let mut frame = DrizzleFrame::new(
        AstroImage::from_pixels(ImageDimensions::new((4, 4), 1), vec![1.0; 16]),
        Transform::identity(),
    );
    frame.weight = f32::NAN;
    let error = acc.add_frame(frame).unwrap_err();
    assert!(matches!(
        error,
        DrizzleError::InvalidFrameWeight { index: 0, value } if value.is_nan()
    ));

    let mut pixel_weights = vec![1.0; 16];
    pixel_weights[5] = -0.25;
    let mut frame = DrizzleFrame::new(
        AstroImage::from_pixels(ImageDimensions::new((4, 4), 1), vec![1.0; 16]),
        Transform::identity(),
    );
    frame.pixel_weight_map = Some(Buffer2::new(4, 4, pixel_weights));
    let error = acc.add_frame(frame).unwrap_err();
    assert!(matches!(
        error,
        DrizzleError::InvalidPixelWeight {
            frame_index: 0,
            pixel_index: 5,
            value: -0.25,
        }
    ));
}

/// Test sgarea with a horizontal segment from (0,0.5) to (1,0.5).
///
/// This is a left-to-right segment at y=0.5 across the full unit square.
/// Case A (both y in [0,1]): trapezoid = 0.5 * (1-0) * (0.5+0.5) = 0.5
#[test]
fn test_sgarea_horizontal_midpoint() {
    let area = sgarea(0.0, 0.5, 1.0, 0.5);
    assert!((area - 0.5).abs() < 1e-12, "Expected 0.5, got {}", area);
}

/// Test sgarea with reversed direction: (1,0.5) to (0,0.5).
///
/// Same segment but right-to-left → negative sign.
/// sgn_dx = -1, trapezoid = -0.5 * (1-0) * (0.5+0.5) = -0.5
#[test]
fn test_sgarea_horizontal_reversed() {
    let area = sgarea(1.0, 0.5, 0.0, 0.5);
    assert!((area - (-0.5)).abs() < 1e-12, "Expected -0.5, got {}", area);
}

/// Test sgarea with a vertical segment (dx=0) → area = 0.
#[test]
fn test_sgarea_vertical() {
    let area = sgarea(0.5, 0.0, 0.5, 1.0);
    assert!(
        area.abs() < 1e-12,
        "Vertical segment should have area 0, got {}",
        area
    );
}

/// Test sgarea with near-vertical segment (dx ≈ 1e-16) → area ≈ 0.
/// Floating-point arithmetic can produce tiny nonzero dx for segments that
/// should be vertical. Without the tolerance check, this would divide by
/// near-zero dx and produce a huge slope, yielding a wrong area.
#[test]
fn test_sgarea_near_vertical() {
    // Simulate floating-point jitter: x2 = x1 + tiny epsilon
    let area = sgarea(0.5, 0.0, 0.5 + 1e-16, 1.0);
    assert!(
        area.abs() < 1e-12,
        "Near-vertical segment should have area ~0, got {}",
        area
    );

    // Negative near-zero dx
    let area = sgarea(0.5, 0.0, 0.5 - 1e-16, 1.0);
    assert!(
        area.abs() < 1e-12,
        "Near-vertical segment (negative dx) should have area ~0, got {}",
        area
    );
}

/// Test sgarea with segment entirely outside (x > 1).
#[test]
fn test_sgarea_outside_right() {
    let area = sgarea(1.5, 0.0, 2.5, 1.0);
    assert!(
        area.abs() < 1e-12,
        "Outside segment should have area 0, got {}",
        area
    );
}

/// Test sgarea with segment entirely below y=0.
#[test]
fn test_sgarea_below_axis() {
    let area = sgarea(0.0, -1.0, 1.0, -0.5);
    assert!(
        area.abs() < 1e-12,
        "Below-axis segment should have area 0, got {}",
        area
    );
}

/// Test sgarea with segment entirely above y=1.
///
/// Both y >= 1 → full rectangle: sgn_dx * (xhi - xlo) = 1.0 * (1-0) = 1.0
#[test]
fn test_sgarea_above_top() {
    let area = sgarea(0.0, 1.5, 1.0, 2.0);
    assert!(
        (area - 1.0).abs() < 1e-12,
        "Above-top segment should give 1.0, got {}",
        area
    );
}

/// Test sgarea Case A: diagonal from (0,0) to (1,1).
///
/// Segment entirely within [0,1]×[0,1]. Case A trapezoid:
/// 0.5 * (1-0) * (1+0) = 0.5
#[test]
fn test_sgarea_case_a_diagonal() {
    let area = sgarea(0.0, 0.0, 1.0, 1.0);
    assert!((area - 0.5).abs() < 1e-12, "Expected 0.5, got {}", area);
}

/// Test sgarea Case B: segment enters inside, exits above y=1.
///
/// Segment from (0, 0.5) to (1, 1.5). Slope = 1.
/// Clipped x: [0, 1]. ylo = 0.5, yhi = 1.5.
/// ylo <= 1.0, yhi > 1.0 → Case B.
/// det = 0*1.5 - 0.5*1 = -0.5
/// xtop = (dx + det) / dy = (1 + (-0.5)) / 1 = 0.5
/// area = sgn_dx * (0.5*(xtop-xlo)*(1+ylo) + xhi-xtop)
///       = 1 * (0.5*(0.5-0)*(1+0.5) + 1-0.5)
///       = 0.5*0.5*1.5 + 0.5
///       = 0.375 + 0.5 = 0.875
#[test]
fn test_sgarea_case_b() {
    let area = sgarea(0.0, 0.5, 1.0, 1.5);
    assert!((area - 0.875).abs() < 1e-12, "Expected 0.875, got {}", area);
}

/// Test sgarea Case C: segment enters above y=1, exits inside.
///
/// Segment from (0, 1.5) to (1, 0.5). Slope = -1.
/// Clipped x: [0, 1]. ylo = 1.5, yhi = 0.5.
/// ylo > 1.0 → Case C.
/// det = 0*0.5 - 1.5*1 = -1.5
/// xtop = (dx + det) / dy = (1 + (-1.5)) / (-1) = (-0.5)/(-1) = 0.5
/// area = sgn_dx * (0.5*(xhi-xtop)*(1+yhi) + xtop-xlo)
///       = 1 * (0.5*(1-0.5)*(1+0.5) + 0.5-0)
///       = 0.5*0.5*1.5 + 0.5
///       = 0.375 + 0.5 = 0.875
#[test]
fn test_sgarea_case_c() {
    let area = sgarea(0.0, 1.5, 1.0, 0.5);
    assert!((area - 0.875).abs() < 1e-12, "Expected 0.875, got {}", area);
}

/// Test sgarea with segment crossing y=0 (clip to y >= 0).
///
/// Segment from (0, -0.5) to (1, 0.5). Slope = 1.
/// Clipped x: [0, 1]. ylo = -0.5, yhi = 0.5.
/// ylo < 0 → clip: det = 0*0.5 - (-0.5)*1 = 0.5, xlo_new = det/dy = 0.5/1 = 0.5, ylo=0.
/// Now xlo=0.5, ylo=0, xhi=1, yhi=0.5. Case A:
/// 0.5*(1-0.5)*(0.5+0) = 0.5*0.5*0.5 = 0.125
#[test]
fn test_sgarea_crosses_y_zero() {
    let area = sgarea(0.0, -0.5, 1.0, 0.5);
    assert!((area - 0.125).abs() < 1e-12, "Expected 0.125, got {}", area);
}

/// Test boxer: quadrilateral exactly overlapping output pixel → area = 1.0.
///
/// Quad corners at (0,0), (1,0), (1,1), (0,1). Output pixel (0,0) = [0,1]×[0,1].
/// Perfect overlap → area = 1.0.
#[test]
fn test_boxer_exact_overlap() {
    let x = [0.0, 1.0, 1.0, 0.0];
    let y = [0.0, 0.0, 1.0, 1.0];
    let area = boxer(0.0, 0.0, &x, &y);
    assert!(
        (area - 1.0).abs() < 1e-12,
        "Exact overlap should give area 1.0, got {}",
        area
    );
}

/// Test boxer: quad shifted right by 0.5 → overlap = 0.5.
///
/// Quad at (0.5,0)→(1.5,0)→(1.5,1)→(0.5,1). Output pixel (0,0) = [0,1]×[0,1].
/// x overlap: [0.5, 1.0] = 0.5, y overlap: [0, 1] = 1.0. Total = 0.5.
#[test]
fn test_boxer_half_overlap_x() {
    let x = [0.5, 1.5, 1.5, 0.5];
    let y = [0.0, 0.0, 1.0, 1.0];
    let area = boxer(0.0, 0.0, &x, &y);
    assert!(
        (area - 0.5).abs() < 1e-12,
        "Half x-overlap should give area 0.5, got {}",
        area
    );
}

/// Test boxer: quad shifted right 0.5 AND up 0.5 → overlap = 0.25.
///
/// Quad at (0.5,0.5)→(1.5,0.5)→(1.5,1.5)→(0.5,1.5). Output pixel (0,0).
/// x overlap: 0.5, y overlap: 0.5. Total = 0.25.
#[test]
fn test_boxer_quarter_overlap() {
    let x = [0.5, 1.5, 1.5, 0.5];
    let y = [0.5, 0.5, 1.5, 1.5];
    let area = boxer(0.0, 0.0, &x, &y);
    assert!(
        (area - 0.25).abs() < 1e-12,
        "Quarter overlap should give area 0.25, got {}",
        area
    );
}

/// Test boxer with a different output pixel index.
///
/// Quad at (3,5)→(4,5)→(4,6)→(3,6). Output pixel (3,5) = [3,4]×[5,6].
/// After shifting: [0,1]×[0,1] exactly. Area = 1.0.
#[test]
fn test_boxer_nonzero_pixel() {
    let x = [3.0, 4.0, 4.0, 3.0];
    let y = [5.0, 5.0, 6.0, 6.0];
    let area = boxer(3.0, 5.0, &x, &y);
    assert!(
        (area - 1.0).abs() < 1e-12,
        "Exact overlap at (3,5) should give area 1.0, got {}",
        area
    );
}

/// Test boxer with no overlap → area = 0.
#[test]
fn test_boxer_no_overlap() {
    let x = [5.0, 6.0, 6.0, 5.0];
    let y = [5.0, 5.0, 6.0, 6.0];
    let area = boxer(0.0, 0.0, &x, &y);
    assert!(
        area.abs() < 1e-12,
        "No overlap should give area 0, got {}",
        area
    );
}

/// Test boxer with a 45° rotated square.
///
/// Diamond centered at (0.5, 0.5) with vertices at distance 0.5*sqrt(2)/sqrt(2) = 0.5:
/// (0.5, 0), (1, 0.5), (0.5, 1), (0, 0.5).
/// This diamond is inscribed in the unit square, with area = 0.5.
#[test]
fn test_boxer_rotated_diamond() {
    let x = [0.5, 1.0, 0.5, 0.0];
    let y = [0.0, 0.5, 1.0, 0.5];
    let area = boxer(0.0, 0.0, &x, &y);
    assert!(
        (area - 0.5).abs() < 1e-12,
        "Diamond inscribed in unit square should have area 0.5, got {}",
        area
    );
}
