use crate::stacking::drizzle::tests::*;

/// Test square kernel with identity transform, uniform image, scale=1, pixfrac=1.
///
/// Each input pixel maps to exactly one output pixel with overlap = 1.0.
/// Jacobian = scale² * pixfrac² = 1.0 (area of unit square in output space).
/// Weight = overlap / jaco = 1.0 / 1.0 = 1.0.
/// Output = input value.
#[test]
fn test_square_kernel_identity_uniform() {
    let pixels: Vec<f32> = (0..25).map(|i| i as f32).collect();
    let image = AstroImage::from_pixels(ImageDimensions::new((5, 5), 1), pixels.clone());

    let config = DrizzleConfig {
        scale: 1.0,
        pixfrac: 1.0,
        kernel: DrizzleKernel::Square,
        fill_value: 0.0,
        min_coverage: 0.0,
    };
    let mut acc = accumulator(ImageDimensions::new((5, 5), 1), config);
    acc.add_image(image, &Transform::identity(), 1.0, None);
    let result = acc.finalize();
    let out = result.image.channel(0);

    for (i, (&actual, &expected)) in out.iter().zip(pixels.iter()).enumerate() {
        assert!(
            (actual - expected).abs() < 1e-4,
            "Pixel {} should be {}, got {}",
            i,
            expected,
            actual
        );
    }
}

/// Test square kernel matches turbo kernel with pure translation (no rotation).
///
/// For axis-aligned drops (no rotation/shear), the Square kernel should produce
/// the same output as the Turbo kernel since the quadrilateral is an axis-aligned
/// rectangle in both cases.
#[test]
fn test_square_kernel_matches_turbo_no_rotation() {
    let pixels: Vec<f32> = (0..100).map(|i| (i as f32) * 0.01).collect();
    let image1 = AstroImage::from_pixels(ImageDimensions::new((10, 10), 1), pixels.clone());
    let image2 = AstroImage::from_pixels(ImageDimensions::new((10, 10), 1), pixels);
    let transform = Transform::translation(DVec2::new(0.3, -0.2));

    // Turbo
    let config_turbo = DrizzleConfig {
        scale: 2.0,
        pixfrac: 0.8,
        kernel: DrizzleKernel::Turbo,
        fill_value: 0.0,
        min_coverage: 0.0,
    };
    let mut acc_turbo = accumulator(ImageDimensions::new((10, 10), 1), config_turbo);
    acc_turbo.add_image(image1, &transform, 1.0, None);
    let result_turbo = acc_turbo.finalize();

    // Square
    let config_square = DrizzleConfig {
        scale: 2.0,
        pixfrac: 0.8,
        kernel: DrizzleKernel::Square,
        fill_value: 0.0,
        min_coverage: 0.0,
    };
    let mut acc_square = accumulator(ImageDimensions::new((10, 10), 1), config_square);
    acc_square.add_image(image2, &transform, 1.0, None);
    let result_square = acc_square.finalize();

    let out_turbo = result_turbo.image.channel(0);
    let out_square = result_square.image.channel(0);

    for i in 0..out_turbo.len() {
        assert!(
            (out_turbo[i] - out_square[i]).abs() < 1e-3,
            "Pixel {} turbo={} square={} differ by {}",
            i,
            out_turbo[i],
            out_square[i],
            (out_turbo[i] - out_square[i]).abs()
        );
    }
}

/// Test square kernel with 45° rotation on uniform image.
///
/// Uniform value 3.0. After 45° rotation, the weighted mean at every covered
/// interior pixel should still be 3.0 (weighted average of identical values).
/// Also verify that there is meaningful coverage in the output interior.
#[test]
fn test_square_kernel_rotation() {
    let image = AstroImage::from_pixels(ImageDimensions::new((20, 20), 1), vec![3.0; 20 * 20]);

    // 45° rotation around center (10, 10)
    let angle = FRAC_PI_4;
    let cos_a = angle.cos();
    let sin_a = angle.sin();
    let cx = 10.0;
    let cy = 10.0;
    // Rotation matrix (row-major): translate to origin, rotate, translate back
    // x' = cos*(x-cx) - sin*(y-cy) + cx
    // y' = sin*(x-cx) + cos*(y-cy) + cy
    use crate::math::dmat3::DMat3;
    use crate::stacking::registration::transform::TransformType;
    let matrix = DMat3::from_rows(
        [cos_a, -sin_a, cx * (1.0 - cos_a) + cy * sin_a],
        [sin_a, cos_a, -cx * sin_a + cy * (1.0 - cos_a)],
        [0.0, 0.0, 1.0],
    );
    let transform = Transform::from_matrix(matrix, TransformType::Euclidean);

    let config = DrizzleConfig {
        scale: 1.0,
        pixfrac: 1.0,
        kernel: DrizzleKernel::Square,
        fill_value: 0.0,
        min_coverage: 0.0,
    };
    let mut acc = accumulator(ImageDimensions::new((20, 20), 1), config);
    acc.add_image(image, &transform, 1.0, None);
    let result = acc.finalize();
    let out = result.image.channel(0);

    // Verify coverage exists in the interior (rotated image should still cover center)
    let center_coverage = result.coverage[(10, 10)];
    assert!(
        center_coverage > 0.0,
        "Center should have coverage, got {}",
        center_coverage
    );

    // All covered pixels should have value ~3.0 (uniform weighted mean)
    let covered_pixels: Vec<f32> = out.iter().copied().filter(|&v| v > 0.01).collect();
    assert!(
        !covered_pixels.is_empty(),
        "Should have some covered pixels"
    );
    for (i, &val) in covered_pixels.iter().enumerate() {
        assert!(
            (val - 3.0).abs() < 0.05,
            "Covered pixel {} should be ~3.0, got {}",
            i,
            val
        );
    }
}

/// Test square kernel with pixfrac < 1.0 produces smaller drops.
///
/// pixfrac=0.5 produces drops that are 0.5× the input pixel size, so they
/// cover fewer output pixels than pixfrac=1.0.
#[test]
fn test_square_kernel_pixfrac() {
    // Use translation (0.1, 0.1) to avoid perfectly centered drops
    let transform = Transform::translation(DVec2::new(0.1, 0.1));

    // Single bright pixel at (2,2)
    let mut pixels1 = vec![0.0f32; 6 * 6];
    pixels1[2 * 6 + 2] = 1.0;
    let image1 = AstroImage::from_pixels(ImageDimensions::new((6, 6), 1), pixels1);

    let config1 = DrizzleConfig {
        scale: 2.0,
        pixfrac: 1.0,
        kernel: DrizzleKernel::Square,
        fill_value: 0.0,
        min_coverage: 0.0,
    };
    let mut acc1 = accumulator(ImageDimensions::new((6, 6), 1), config1);
    acc1.add_image(image1, &transform, 1.0, None);
    let r1 = acc1.finalize();
    let covered_1 = r1.image.channel(0).iter().filter(|&&v| v > 0.01).count();

    let mut pixels2 = vec![0.0f32; 6 * 6];
    pixels2[2 * 6 + 2] = 1.0;
    let image2 = AstroImage::from_pixels(ImageDimensions::new((6, 6), 1), pixels2);

    let config2 = DrizzleConfig {
        scale: 2.0,
        pixfrac: 0.5,
        kernel: DrizzleKernel::Square,
        fill_value: 0.0,
        min_coverage: 0.0,
    };
    let mut acc2 = accumulator(ImageDimensions::new((6, 6), 1), config2);
    acc2.add_image(image2, &transform, 1.0, None);
    let r2 = acc2.finalize();
    let covered_2 = r2.image.channel(0).iter().filter(|&&v| v > 0.01).count();

    assert!(
        covered_1 > covered_2,
        "pixfrac=1.0 should cover more pixels ({}) than pixfrac=0.5 ({})",
        covered_1,
        covered_2
    );
}

/// Test square kernel with per-pixel weights.
///
/// Uniform image = 5.0, pixel (1,1) excluded via weight=0.
/// scale=1, pixfrac=1. Output (1,1) should be fill_value, others = 5.0.
#[test]
fn test_square_kernel_with_pixel_weights() {
    let image = AstroImage::from_pixels(ImageDimensions::new((4, 4), 1), vec![5.0; 16]);

    let mut pw = Buffer2::new_filled(4, 4, 1.0f32);
    *pw.get_mut(1, 1) = 0.0;

    let config = DrizzleConfig {
        scale: 1.0,
        pixfrac: 1.0,
        kernel: DrizzleKernel::Square,
        fill_value: -1.0,
        min_coverage: 0.0,
    };
    let mut acc = accumulator(ImageDimensions::new((4, 4), 1), config);
    acc.add_image(image, &Transform::identity(), 1.0, Some(&pw));
    let result = acc.finalize();
    let out = result.image.channel(0);

    // (1,1) excluded → fill_value = -1.0
    assert!(
        (out[5] - (-1.0)).abs() < 1e-5,
        "Excluded pixel (1,1) should be fill_value -1.0, got {}",
        out[5]
    );
    // (0,0) normal → 5.0
    assert!(
        (out[0] - 5.0).abs() < 1e-5,
        "Normal pixel (0,0) should be 5.0, got {}",
        out[0]
    );
    // (2,2) normal → 5.0
    assert!(
        (out[2 * 4 + 2] - 5.0).abs() < 1e-5,
        "Normal pixel (2,2) should be 5.0, got {}",
        out[2 * 4 + 2]
    );
}

/// Test square kernel with scale=2 on a single bright pixel.
///
/// scale=2, pixfrac=0.8: the drop is 0.8×0.8 of the input pixel. Integer-center: input pixel
/// (1,1) center (1,1), corners (0.6,0.6)..(1.4,1.4), identity ×scale 2 → quad [1.2,2.8]²
/// (area 2.56 = (pixfrac·scale)² = the Jacobian). The quad covers output cells 1,2,3 with
/// per-axis overlaps 0.3/1.0/0.3. The center cell (2,2) is fully inside and no neighbour's
/// quad reaches it → undiluted 2.0; edge cells are shared 50/50 with a zero-valued
/// neighbour's quad → 1.0 (same footprint as the turbo kernel).
#[test]
fn test_square_kernel_scale2_single_pixel() {
    let mut pixels = vec![0.0f32; 4 * 4];
    pixels[5] = 2.0; // pixel (1,1)
    let image = AstroImage::from_pixels(ImageDimensions::new((4, 4), 1), pixels);

    let config = DrizzleConfig {
        scale: 2.0,
        pixfrac: 0.8,
        kernel: DrizzleKernel::Square,
        fill_value: 0.0,
        min_coverage: 0.0,
    };
    let mut acc = accumulator(ImageDimensions::new((4, 4), 1), config);
    acc.add_image(image, &Transform::identity(), 1.0, None);
    let result = acc.finalize();
    let out = result.image.channel(0);
    let w = 8usize;

    let at = |x: usize, y: usize| out[y * w + x];
    // Center cell (2,2): quad fully covers it, no neighbour reaches → undiluted 2.0.
    assert!((at(2, 2) - 2.0).abs() < 1e-4, "center (2,2): {}", at(2, 2));
    // Edge cells: shared 50/50 with a zero-valued neighbour's quad → 1.0.
    assert!((at(1, 2) - 1.0).abs() < 1e-4, "edge (1,2): {}", at(1, 2));
    assert!((at(3, 2) - 1.0).abs() < 1e-4, "edge (3,2): {}", at(3, 2));
    assert!((at(2, 1) - 1.0).abs() < 1e-4, "edge (2,1): {}", at(2, 1));
    assert!((at(2, 3) - 1.0).abs() < 1e-4, "edge (2,3): {}", at(2, 3));
    // No pixel exceeds the bright value.
    assert!(out.iter().all(|&v| v <= 2.0 + 1e-4), "no pixel exceeds 2.0");
}

/// Test Jacobian correctness via the square (polygon-overlap) kernel where two input pixels
/// with different values mix in one output cell.
///
/// scale=2, pixfrac=1.0, identity. Integer-center: pixel (0,0)=10 → quad [-1,1]², pixel
/// (1,0)=20 → quad [1,3]×[-1,1]; their shared edge x=1 lands on the center of output cell 1.
/// Both quads have area (pixfrac·scale)² = 4 (the Jacobian), so each contributes weight
/// overlap/4.
///   cell (0,0) = [-0.5,0.5): only pixel (0,0) overlaps (1.0) → 10.0
///   cell (1,0) = [0.5,1.5): pixel (0,0) and (1,0) each overlap 0.5 → equal-weight mean
///                            (10·0.125 + 20·0.125) / 0.25 = 15.0
///   cell (2,0) = [1.5,2.5): only pixel (1,0) overlaps (1.0) → 20.0
#[test]
fn test_square_kernel_jacobian_weighted_average() {
    let mut pixels = vec![0.0f32; 4 * 4];
    pixels[0] = 10.0; // pixel (0,0)
    pixels[1] = 20.0; // pixel (1,0)
    let image = AstroImage::from_pixels(ImageDimensions::new((4, 4), 1), pixels);

    let config = DrizzleConfig {
        scale: 2.0,
        pixfrac: 1.0,
        kernel: DrizzleKernel::Square,
        fill_value: 0.0,
        min_coverage: 0.0,
    };
    let mut acc = accumulator(ImageDimensions::new((4, 4), 1), config);
    acc.add_image(image, &Transform::identity(), 1.0, None);
    let result = acc.finalize();
    let out = result.image.channel(0);

    // cell (0,0): only pixel (0,0) → 10.0
    assert!(
        (out[0] - 10.0).abs() < 1e-3,
        "cell (0,0) = 10.0, got {}",
        out[0]
    );
    // cell (1,0): equal-weight mean of 10 and 20 (each overlaps 0.5) → 15.0
    assert!(
        (out[1] - 15.0).abs() < 1e-3,
        "cell (1,0) = 15.0, got {}",
        out[1]
    );
    // cell (2,0): only pixel (1,0) → 20.0
    assert!(
        (out[2] - 20.0).abs() < 1e-3,
        "cell (2,0) = 20.0, got {}",
        out[2]
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
fn test_boxer_rotated_partial_clip() {
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

    let x: [f64; 4] = std::array::from_fn(|i| cx + corners[i].0 * cos30 - corners[i].1 * sin30);
    let y: [f64; 4] = std::array::from_fn(|i| cy + corners[i].0 * sin30 + corners[i].1 * cos30);

    let area = boxer(0.0, 0.0, &x, &y);

    // The rotated square extends beyond [0,1]×[0,1], so overlap < 1.0
    assert!(
        area < 1.0,
        "30° rotated square should partially clip, got area {}",
        area
    );
    // But the center is at (0.5,0.5) so most of the area is inside
    assert!(
        area > 0.8,
        "Most of the rotated square should be inside, got area {}",
        area
    );

    // Verified by Python reference implementation of sgarea/boxer:
    //   Edge 0→1: sgarea(0.3170,-0.1830,1.1830,0.3170) = 0.038675
    //   Edge 1→2: sgarea(1.1830,0.3170,0.6830,1.1830) = -0.278312
    //   Edge 2→3: sgarea(0.6830,1.1830,-0.1830,0.6830) = -0.644338
    //   Edge 3→0: sgarea(-0.1830,0.6830,0.3170,-0.1830) = 0.038675
    //   Sum = -0.845299, abs = 0.845299
    let expected = 0.845299;
    assert!(
        (area - expected).abs() < 1e-4,
        "Expected overlap ~{:.6}, got {:.6}",
        expected,
        area
    );
}

/// Test that Square kernel produces DIFFERENT output from Turbo under rotation.
///
/// This is the entire motivation for the Square kernel: with rotation, the
/// quadrilateral footprint is not axis-aligned, so Turbo (axis-aligned box) is wrong.
///
/// Use a non-uniform image (gradient) with a small rotation. The Turbo kernel
/// approximates the drop as an axis-aligned box, while the Square kernel correctly
/// computes the rotated quadrilateral overlap. The outputs must differ.
#[test]
fn test_square_differs_from_turbo_under_rotation() {
    // Horizontal gradient: value increases with x
    let pixels: Vec<f32> = (0..100)
        .map(|i| {
            let x = i % 10;
            x as f32
        })
        .collect();
    let image1 = AstroImage::from_pixels(ImageDimensions::new((10, 10), 1), pixels.clone());
    let image2 = AstroImage::from_pixels(ImageDimensions::new((10, 10), 1), pixels);

    // 15° rotation around center — enough to produce measurable overlap differences
    let angle = 15.0_f64.to_radians();
    let cos_a = angle.cos();
    let sin_a = angle.sin();
    let cx = 5.0;
    let cy = 5.0;
    use crate::math::dmat3::DMat3;
    use crate::stacking::registration::transform::TransformType;
    let matrix = DMat3::from_rows(
        [cos_a, -sin_a, cx * (1.0 - cos_a) + cy * sin_a],
        [sin_a, cos_a, -cx * sin_a + cy * (1.0 - cos_a)],
        [0.0, 0.0, 1.0],
    );
    let transform = Transform::from_matrix(matrix, TransformType::Euclidean);

    let config_turbo = DrizzleConfig {
        scale: 1.0,
        pixfrac: 1.0,
        kernel: DrizzleKernel::Turbo,
        fill_value: 0.0,
        min_coverage: 0.0,
    };
    let mut acc_turbo = accumulator(ImageDimensions::new((10, 10), 1), config_turbo);
    acc_turbo.add_image(image1, &transform, 1.0, None);
    let result_turbo = acc_turbo.finalize();

    let config_square = DrizzleConfig {
        scale: 1.0,
        pixfrac: 1.0,
        kernel: DrizzleKernel::Square,
        fill_value: 0.0,
        min_coverage: 0.0,
    };
    let mut acc_square = accumulator(ImageDimensions::new((10, 10), 1), config_square);
    acc_square.add_image(image2, &transform, 1.0, None);
    let result_square = acc_square.finalize();

    let out_turbo = result_turbo.image.channel(0);
    let out_square = result_square.image.channel(0);

    // Find the maximum difference between Turbo and Square on covered pixels
    let mut max_diff = 0.0f32;
    let mut n_covered = 0;
    for i in 0..out_turbo.len() {
        // Only compare covered pixels (both non-zero)
        if out_turbo[i].abs() > 0.01 && out_square[i].abs() > 0.01 {
            let diff = (out_turbo[i] - out_square[i]).abs();
            if diff > max_diff {
                max_diff = diff;
            }
            n_covered += 1;
        }
    }

    assert!(
        n_covered > 20,
        "Should have many covered pixels, got {}",
        n_covered
    );
    assert!(
        max_diff > 0.01,
        "Square and Turbo should differ under 5° rotation with gradient image, max_diff={}",
        max_diff
    );
}

/// Test flux conservation under rotation with non-uniform image.
///
/// A 20×20 image with a bright 4×4 patch (value=10.0) at center, rest=1.0.
/// Rotate 15° around center. Total input flux = 4*4*10 + (400-16)*1 = 544.
///
/// After drizzle at scale=1, pixfrac=1 with Square kernel, the total weighted flux
/// should be conserved: sum(data) = sum(original_flux * weight), and since each
/// input pixel's total weight contribution sums to ~1.0 (for interior pixels where
/// the full drop lands on the output grid), the total output flux should approximate
/// the total input flux.
///
/// We check sum(output * coverage_weight) ≈ sum(input).
/// More precisely: sum(data_buf) should equal sum(input * per_pixel_total_weight).
/// For fully-covered interior pixels, each input pixel's overlap sums to jaco
/// (the drop area in output space), and weight = overlap/jaco, so total weight = 1.0.
/// Therefore sum(output_pixel * weight) ≈ sum(input_pixel) for interior pixels.
#[test]
fn test_square_kernel_flux_conservation() {
    let mut pixels = vec![1.0f32; 20 * 20];
    // Bright 4×4 patch at center (8..12, 8..12)
    for y in 8..12 {
        for x in 8..12 {
            pixels[y * 20 + x] = 10.0;
        }
    }
    let total_input_flux: f32 = pixels.iter().sum();
    // = 16 * 10 + 384 * 1 = 544.0
    assert!((total_input_flux - 544.0).abs() < f32::EPSILON);

    let image = AstroImage::from_pixels(ImageDimensions::new((20, 20), 1), pixels);

    // 15° rotation around center
    let angle = 15.0_f64.to_radians();
    let cos_a = angle.cos();
    let sin_a = angle.sin();
    let cx = 10.0;
    let cy = 10.0;
    use crate::math::dmat3::DMat3;
    use crate::stacking::registration::transform::TransformType;
    let matrix = DMat3::from_rows(
        [cos_a, -sin_a, cx * (1.0 - cos_a) + cy * sin_a],
        [sin_a, cos_a, -cx * sin_a + cy * (1.0 - cos_a)],
        [0.0, 0.0, 1.0],
    );
    let transform = Transform::from_matrix(matrix, TransformType::Euclidean);

    let config = DrizzleConfig {
        scale: 1.0,
        pixfrac: 1.0,
        kernel: DrizzleKernel::Square,
        fill_value: 0.0,
        min_coverage: 0.0,
    };
    let mut acc = accumulator(ImageDimensions::new((20, 20), 1), config);
    acc.add_image(image, &transform, 1.0, None);

    // Before finalize, check raw weighted flux sum.
    // data[c] = sum(flux * weight) per output pixel, weights[c] = sum(weight).
    // total_data = sum over all output pixels of data = sum over all input pixels of
    //   flux * sum_over_output_of(overlap/jaco)
    // For each input pixel, sum_over_output_of(overlap) = total_overlap = jaco (the quad
    // area), so sum_over_output_of(overlap/jaco) = 1.0 for fully interior pixels.
    // Therefore total_data ≈ sum(flux) = 544 for interior pixels.
    let data_sum = accumulated_flux_sum(&acc, 0);

    // Allow ~10% margin for edge effects (rotated image has border pixels that
    // partially fall outside the output grid)
    assert!(
        (data_sum - total_input_flux).abs() / total_input_flux < 0.10,
        "Total weighted flux should be ~{}, got {} (error {:.1}%)",
        total_input_flux,
        data_sum,
        (data_sum - total_input_flux).abs() / total_input_flux * 100.0
    );

    // Now finalize and check the bright patch is still bright
    let result = acc.finalize();
    let out = result.image.channel(0);

    // The center pixel (10,10) should be close to 10.0 since it's in the middle of
    // the bright patch (rotation around center preserves center pixel)
    let center_val = out[10 * 20 + 10];
    assert!(
        (center_val - 10.0).abs() < 0.5,
        "Center pixel should be ~10.0 (bright patch), got {}",
        center_val
    );
}

/// Test two-frame weighted mean with Square kernel.
///
/// Frame 1: uniform 2.0, weight 1.0
/// Frame 2: uniform 8.0, weight 3.0
/// scale=1, pixfrac=1, identity transform.
///
/// Expected: (2.0 * 1.0 + 8.0 * 3.0) / (1.0 + 3.0) = 26.0 / 4.0 = 6.5
#[test]
fn test_square_kernel_two_frame_weighted_mean() {
    let image1 = AstroImage::from_pixels(ImageDimensions::new((6, 6), 1), vec![2.0; 36]);
    let image2 = AstroImage::from_pixels(ImageDimensions::new((6, 6), 1), vec![8.0; 36]);

    let config = DrizzleConfig {
        scale: 1.0,
        pixfrac: 1.0,
        kernel: DrizzleKernel::Square,
        fill_value: 0.0,
        min_coverage: 0.0,
    };
    let mut acc = accumulator(ImageDimensions::new((6, 6), 1), config);
    acc.add_image(image1, &Transform::identity(), 1.0, None);
    acc.add_image(image2, &Transform::identity(), 3.0, None);
    let result = acc.finalize();
    let out = result.image.channel(0);

    // All interior pixels should be 6.5
    // (2*1 + 8*3) / (1+3) = 26/4 = 6.5
    let center = out[3 * 6 + 3];
    assert!(
        (center - 6.5).abs() < 1e-4,
        "Weighted mean should be 6.5, got {}",
        center
    );

    // Check a corner pixel too
    assert!(
        (out[0] - 6.5).abs() < 1e-4,
        "Corner pixel should also be 6.5, got {}",
        out[0]
    );
}

#[test]
fn test_local_jacobian_identity_scale1() {
    // Identity transform, scale=1: one input pixel maps to exactly one output pixel.
    // Jacobian = |det(I)| * 1² = 1.0
    let transform = Transform::identity();
    let center = transform.apply(DVec2::new(5.0, 5.0));
    let jaco = local_jacobian(&transform, center, 5, 5, 1.0);
    assert!(
        (jaco - 1.0).abs() < 1e-10,
        "Identity scale=1: expected 1.0, got {jaco}"
    );
}

#[test]
fn test_local_jacobian_identity_scale2() {
    // Identity transform, scale=2: one input pixel maps to a 2×2 area in output.
    // Jacobian = |det(I)| * 2² = 4.0
    let transform = Transform::identity();
    let center = transform.apply(DVec2::new(5.0, 5.0));
    let jaco = local_jacobian(&transform, center, 5, 5, 2.0);
    assert!(
        (jaco - 4.0).abs() < 1e-10,
        "Identity scale=2: expected 4.0, got {jaco}"
    );
}

#[test]
fn test_local_jacobian_rotation_preserves_area() {
    // Pure rotation around origin: area is preserved, Jacobian = scale².
    // Rotation by 30° around (0, 0), scale=1.
    use crate::math::dmat3::DMat3;
    use crate::stacking::registration::transform::TransformType;
    let angle = 30.0_f64.to_radians();
    let (sin_a, cos_a) = angle.sin_cos();
    let matrix = DMat3::from_rows([cos_a, -sin_a, 0.0], [sin_a, cos_a, 0.0], [0.0, 0.0, 1.0]);
    let transform = Transform::from_matrix(matrix, TransformType::Euclidean);
    let center = transform.apply(DVec2::new(50.0, 50.0));
    let jaco = local_jacobian(&transform, center, 50, 50, 1.0);
    // Rotation preserves area: Jacobian = 1.0
    assert!(
        (jaco - 1.0).abs() < 1e-10,
        "30° rotation: expected 1.0, got {jaco}"
    );
}

#[test]
fn test_local_jacobian_anisotropic_scale() {
    // Scale by 2x in x, 3x in y: area magnification = 6.
    use crate::math::dmat3::DMat3;
    use crate::stacking::registration::transform::TransformType;
    let matrix = DMat3::from_rows([2.0, 0.0, 0.0], [0.0, 3.0, 0.0], [0.0, 0.0, 1.0]);
    let transform = Transform::from_matrix(matrix, TransformType::Affine);
    let center = transform.apply(DVec2::new(5.0, 5.0));
    // Jacobian = |det([2,0;0,3])| * scale² = 6 * 1 = 6.0
    let jaco = local_jacobian(&transform, center, 5, 5, 1.0);
    assert!(
        (jaco - 6.0).abs() < 1e-10,
        "2x×3x scale: expected 6.0, got {jaco}"
    );
}

#[test]
fn test_local_jacobian_perspective_varies_spatially() {
    // Perspective transform: Jacobian should differ at different image locations.
    // Homography with small perspective terms.
    use crate::math::dmat3::DMat3;
    use crate::stacking::registration::transform::TransformType;
    let matrix = DMat3::from_rows(
        [1.0, 0.0, 0.0],
        [0.0, 1.0, 0.0],
        [1e-4, 0.0, 1.0], // perspective: w = 1e-4 * x + 1
    );
    let transform = Transform::from_matrix(matrix, TransformType::Homography);

    // At x=0: w ≈ 1, minimal distortion
    let c0 = transform.apply(DVec2::new(0.0, 0.0));
    let jaco_left = local_jacobian(&transform, c0, 0, 0, 1.0);

    // At x=1000: w ≈ 1.1, noticeable distortion
    let c1000 = transform.apply(DVec2::new(1000.0, 0.0));
    let jaco_right = local_jacobian(&transform, c1000, 1000, 0, 1.0);

    // Jacobians must differ for perspective transform
    assert!(
        (jaco_left - jaco_right).abs() > 0.01,
        "Perspective Jacobian should vary: left={jaco_left:.6}, right={jaco_right:.6}"
    );
    // At x=0, w≈1 so jaco≈1. At x=1000, w≈1.1 so area shrinks → jaco < 1.
    assert!(
        jaco_left > jaco_right,
        "Left side should have larger Jacobian than right: {jaco_left:.6} vs {jaco_right:.6}"
    );
}

//
