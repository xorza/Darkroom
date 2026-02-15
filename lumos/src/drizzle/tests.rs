use super::*;

#[test]
fn test_drizzle_config_default() {
    let config = DrizzleConfig::default();
    assert!((config.scale - 2.0).abs() < f32::EPSILON);
    assert!((config.pixfrac - 0.8).abs() < f32::EPSILON);
    assert_eq!(config.kernel, DrizzleKernel::Turbo);
}

#[test]
fn test_drizzle_config_presets() {
    let x1_5 = DrizzleConfig::x1_5();
    assert!((x1_5.scale - 1.5).abs() < f32::EPSILON);

    let x2 = DrizzleConfig::x2();
    assert!((x2.scale - 2.0).abs() < f32::EPSILON);

    let x3 = DrizzleConfig::x3();
    assert!((x3.scale - 3.0).abs() < f32::EPSILON);
}

#[test]
fn test_drizzle_config_builder() {
    let config = DrizzleConfig::default()
        .with_pixfrac(0.5)
        .with_kernel(DrizzleKernel::Gaussian)
        .with_min_coverage(0.2);

    assert!((config.pixfrac - 0.5).abs() < f32::EPSILON);
    assert_eq!(config.kernel, DrizzleKernel::Gaussian);
    assert!((config.min_coverage - 0.2).abs() < f32::EPSILON);
}

#[test]
#[should_panic(expected = "pixfrac must be between")]
fn test_drizzle_config_invalid_pixfrac() {
    DrizzleConfig::default().with_pixfrac(1.5);
}

#[test]
fn test_drizzle_accumulator_dimensions() {
    let config = DrizzleConfig::x2();
    let acc = DrizzleAccumulator::new(ImageDimensions::new(100, 80, 3), config);
    let dims = acc.dimensions();
    assert_eq!(dims.width, 200);
    assert_eq!(dims.height, 160);
    assert_eq!(dims.channels, 3);
}

#[test]
fn test_compute_square_overlap() {
    // Full overlap (unit squares at same position)
    let overlap = compute_square_overlap(0.0, 0.0, 1.0, 1.0, 0.0, 0.0, 1.0, 1.0);
    assert!((overlap - 1.0).abs() < f32::EPSILON);

    // No overlap
    let overlap = compute_square_overlap(0.0, 0.0, 1.0, 1.0, 2.0, 2.0, 3.0, 3.0);
    assert!((overlap - 0.0).abs() < f32::EPSILON);

    // Partial overlap (half)
    let overlap = compute_square_overlap(0.0, 0.0, 1.0, 1.0, 0.5, 0.0, 1.5, 1.0);
    assert!((overlap - 0.5).abs() < f32::EPSILON);

    // Quarter overlap
    let overlap = compute_square_overlap(0.0, 0.0, 1.0, 1.0, 0.5, 0.5, 1.5, 1.5);
    assert!((overlap - 0.25).abs() < f32::EPSILON);
}

#[test]
fn test_lanczos_kernel() {
    // Center value
    assert!((lanczos_kernel(0.0, 3.0) - 1.0).abs() < f32::EPSILON);

    // Outside support
    assert!((lanczos_kernel(3.5, 3.0) - 0.0).abs() < f32::EPSILON);

    // Symmetry
    let pos = lanczos_kernel(1.5, 3.0);
    let neg = lanczos_kernel(-1.5, 3.0);
    assert!((pos - neg).abs() < 1e-6);
}

#[test]
fn test_drizzle_single_image() {
    // Create a simple test image
    let image = AstroImage::from_pixels(ImageDimensions::new(100, 100, 1), vec![0.5; 100 * 100]);

    let config = DrizzleConfig::x2();
    let mut acc = DrizzleAccumulator::new(ImageDimensions::new(100, 100, 1), config);

    let identity = Transform::identity();
    acc.add_image(image, &identity, 1.0, None);

    let result = acc.finalize();

    // Output should be 200x200
    assert_eq!(result.image.width(), 200);
    assert_eq!(result.image.height(), 200);

    // With scale=2, pixfrac=0.8: drop_size = 0.8*2 = 1.6 output pixels.
    // Input pixel (ix,iy) center at (ix+0.5, iy+0.5), scaled to (2*ix+1, 2*iy+1).
    // Drop covers (center ± 0.8), spanning a 2×2 output block.
    // Each overlap = 0.8 * 0.8 = 0.64, inv_area = 1/2.56, weight = 0.64/2.56 = 0.25.
    // Interior output pixels receive exactly one contribution.
    // Finalized output = (0.5 * 0.25) / 0.25 = 0.5
    let pixels = result.image.channel(0);
    let avg: f32 = pixels.iter().sum::<f32>() / pixels.len() as f32;
    assert!(
        (avg - 0.5).abs() < 1e-5,
        "Average should be 0.5, got {}",
        avg
    );
    // Verify specific pixels
    assert!(
        (pixels[0] - 0.5).abs() < 1e-5,
        "Pixel (0,0) should be 0.5, got {}",
        pixels[0]
    );
}

#[test]
fn test_drizzle_point_kernel() {
    let image = AstroImage::from_pixels(ImageDimensions::new(10, 10, 1), vec![1.0; 10 * 10]);

    let config = DrizzleConfig::x2().with_kernel(DrizzleKernel::Point);
    let mut acc = DrizzleAccumulator::new(ImageDimensions::new(10, 10, 1), config);

    let identity = Transform::identity();
    acc.add_image(image, &identity, 1.0, None);

    let result = acc.finalize();
    assert_eq!(result.image.width(), 20);
    assert_eq!(result.image.height(), 20);

    // Point kernel: input (ix,iy) center at (ix+0.5, iy+0.5)
    // scaled → output floor((ix+0.5)*2) = 2*ix+1, floor((iy+0.5)*2) = 2*iy+1
    // Covered pixels (odd x, odd y): value = 1.0/1.0 = 1.0
    // Uncovered pixels: fill_value = 0.0
    let pixels = result.image.channel(0);
    let w = 20;
    // (1,1) ← input (0,0): value = 1.0
    assert!((pixels[w + 1] - 1.0).abs() < f32::EPSILON);
    // (3,1) ← input (1,0): value = 1.0
    assert!((pixels[w + 3] - 1.0).abs() < f32::EPSILON);
    // (0,0): no coverage → fill_value = 0.0
    assert!((pixels[0]).abs() < f32::EPSILON);
    // (2,2): even coords, no coverage → 0.0
    assert!((pixels[2 * w + 2]).abs() < f32::EPSILON);
    // Exactly 100 covered pixels (10×10 input maps to 10×10 odd-coordinate outputs)
    let covered = pixels.iter().filter(|&&v| v > 0.5).count();
    assert_eq!(covered, 100);
}

#[test]
fn test_drizzle_stack_empty_paths() {
    let paths: Vec<std::path::PathBuf> = vec![];
    let transforms: Vec<Transform> = vec![];
    let config = DrizzleConfig::default();

    let result = drizzle_stack(
        &paths,
        &transforms,
        None,
        None,
        &config,
        ProgressCallback::default(),
    );
    assert!(matches!(result.unwrap_err(), Error::NoPaths));
}

#[test]
fn test_drizzle_rgb_image() {
    // Create a simple RGB test image
    let mut pixels = vec![0.0f32; 50 * 50 * 3];
    for y in 0..50 {
        for x in 0..50 {
            let idx = (y * 50 + x) * 3;
            pixels[idx] = 0.5; // R
            pixels[idx + 1] = 0.3; // G
            pixels[idx + 2] = 0.7; // B
        }
    }
    let image = AstroImage::from_pixels(ImageDimensions::new(50, 50, 3), pixels);

    let config = DrizzleConfig::x2();
    let mut acc = DrizzleAccumulator::new(ImageDimensions::new(50, 50, 3), config);

    let identity = Transform::identity();
    acc.add_image(image, &identity, 1.0, None);

    let result = acc.finalize();

    assert_eq!(result.image.width(), 100);
    assert_eq!(result.image.height(), 100);
    assert_eq!(result.image.channels(), 3);
}

#[test]
fn test_drizzle_with_translation() {
    // Single bright pixel at (10,10), all others zero
    let mut pixels = vec![0.0f32; 20 * 20];
    pixels[10 * 20 + 10] = 1.0;
    let image = AstroImage::from_pixels(ImageDimensions::new(20, 20, 1), pixels);

    // scale=2, pixfrac=0.8: drop_size = 0.8*2 = 1.6, half_drop = 0.8
    let config = DrizzleConfig::x2();
    let mut acc = DrizzleAccumulator::new(ImageDimensions::new(20, 20, 1), config);

    // Translation (0.5, 0.5): pixel (10,10) center at (10.5+0.5, 10.5+0.5) = (11, 11)
    // Scaled to output: (22, 22). Drop from (21.2, 21.2) to (22.8, 22.8).
    // Overlaps output pixels (21,21), (22,21), (21,22), (22,22).
    // Each overlap = 0.8 * 0.8 = 0.64, inv_area = 1/2.56, weight = 0.25.
    let transform = Transform::translation(DVec2::new(0.5, 0.5));
    acc.add_image(image, &transform, 1.0, None);

    let result = acc.finalize();
    assert_eq!(result.image.width(), 40);
    assert_eq!(result.image.height(), 40);

    let out = result.image.channel(0);
    // The 4 output pixels receiving flux = 1.0*0.25/0.25 = 1.0
    assert!(
        (out[21 * 40 + 21] - 1.0).abs() < 1e-5,
        "Expected 1.0 at (21,21), got {}",
        out[21 * 40 + 21]
    );
    assert!(
        (out[21 * 40 + 22] - 1.0).abs() < 1e-5,
        "Expected 1.0 at (22,21), got {}",
        out[21 * 40 + 22]
    );
    assert!(
        (out[22 * 40 + 21] - 1.0).abs() < 1e-5,
        "Expected 1.0 at (21,22), got {}",
        out[22 * 40 + 21]
    );
    assert!(
        (out[22 * 40 + 22] - 1.0).abs() < 1e-5,
        "Expected 1.0 at (22,22), got {}",
        out[22 * 40 + 22]
    );
    // Pixel far from the bright spot should be 0.0
    assert!((out[0]).abs() < 1e-5);
}

#[test]
fn test_coverage_at() {
    // Point kernel with identity: covered at odd coords, uncovered at even
    let image = AstroImage::from_pixels(ImageDimensions::new(4, 4, 1), vec![1.0; 16]);
    let config = DrizzleConfig::x2().with_kernel(DrizzleKernel::Point);
    let mut acc = DrizzleAccumulator::new(ImageDimensions::new(4, 4, 1), config);
    acc.add_image(image, &Transform::identity(), 1.0, None);
    let result = acc.finalize();

    // Output 8×8. Covered pixels at (2*ix+1, 2*iy+1) for ix,iy=0..3
    // Normalized coverage: max_coverage = 1.0
    assert!((result.coverage_at(1, 1) - 1.0).abs() < f32::EPSILON); // covered
    assert!((result.coverage_at(0, 0)).abs() < f32::EPSILON); // uncovered
    assert!((result.coverage_at(3, 3) - 1.0).abs() < f32::EPSILON); // covered
    assert!((result.coverage_at(2, 2)).abs() < f32::EPSILON); // uncovered
}

/// Test turbo kernel drop size and overlap with hand-computed values.
///
/// Setup: 4×4 input, scale=2, pixfrac=0.8 → drop_size = 1.6, half_drop = 0.8
/// Output: 8×8. Input pixel (1,1) center at (1.5, 1.5), scaled to (3.0, 3.0).
/// Drop covers (2.2, 2.2) to (3.8, 3.8).
///
/// Overlapping output pixels and their overlap areas:
///   (2,2): x_overlap = min(3.8,3)-max(2.2,2) = 0.8, y = 0.8, area = 0.64
///   (3,2): x_overlap = min(3.8,4)-max(2.2,3) = 0.8, y = 0.8, area = 0.64
///   (2,3): same as (3,2) by symmetry, area = 0.64
///   (3,3): x = min(3.8,4)-max(2.2,3) = 0.8, y = 0.8, area = 0.64
/// Total area = 4 * 0.64 = 2.56 = 1.6^2 ✓
/// inv_area = 1/2.56 ≈ 0.390625
/// pixel_weight = overlap * inv_area = 0.64 * 0.390625 = 0.25
#[test]
fn test_turbo_kernel_overlap_exact() {
    // Single bright pixel at (1,1) with value 2.0
    let mut pixels = vec![0.0f32; 4 * 4];
    pixels[5] = 2.0; // pixel (1,1) = index 1*4+1 = 5
    let image = AstroImage::from_pixels(ImageDimensions::new(4, 4, 1), pixels);

    let config = DrizzleConfig::x2(); // scale=2, pixfrac=0.8
    let mut acc = DrizzleAccumulator::new(ImageDimensions::new(4, 4, 1), config);
    acc.add_image(image, &Transform::identity(), 1.0, None);

    let result = acc.finalize();
    let out = result.image.channel(0);
    let w = 8usize;

    // Drop from (2.2,2.2) to (3.8,3.8) → overlaps (2,2), (3,2), (2,3), (3,3).
    // Each receives: flux=2.0, weight=0.25 → finalized = 2.0*0.25/0.25 = 2.0
    assert!(
        (out[2 * w + 2] - 2.0).abs() < 1e-5,
        "Expected 2.0 at (2,2), got {}",
        out[2 * w + 2]
    );
    assert!(
        (out[2 * w + 3] - 2.0).abs() < 1e-5,
        "Expected 2.0 at (3,2), got {}",
        out[2 * w + 3]
    );
    assert!(
        (out[3 * w + 2] - 2.0).abs() < 1e-5,
        "Expected 2.0 at (2,3), got {}",
        out[3 * w + 2]
    );
    assert!(
        (out[3 * w + 3] - 2.0).abs() < 1e-5,
        "Expected 2.0 at (3,3), got {}",
        out[3 * w + 3]
    );

    // Adjacent pixels outside the drop should be zero (or fill_value)
    assert!(out[w + 2].abs() < 1e-5, "No flux at (2,1)");
    assert!(out[4 * w + 3].abs() < 1e-5, "No flux at (3,4)");
}

/// Test turbo kernel with non-integer translation producing asymmetric overlap.
///
/// Uniform image value=1.0, translation=(0.25, 0.0).
/// scale=2, pixfrac=1.0 → drop_size = 2.0, half_drop = 1.0
/// Pixel (0,0) center at (0.5+0.25, 0.5) = (0.75, 0.5), scaled to (1.5, 1.0).
/// Drop from (0.5, 0.0) to (2.5, 2.0).
///
/// Output pixel (0,0) receives contributions from input (0,0) only:
///   area = 0.5 * 1.0 = 0.5, weight = 0.5 * 0.25 = 0.125
/// Output pixel (1,0) receives from (0,0) and (1,0):
///   from (0,0): area=1.0, weight=0.25; from (1,0): area=1.0, weight=0.25
///   total data = 1.0*0.25 + 1.0*0.25 = 0.5, total weight = 0.5 → output = 1.0
///
/// For uniform image, all covered pixels should be 1.0 (weighted mean of same value).
/// Coverage varies: (0,0) has weight 0.125, (1,0) has weight 0.50.
#[test]
fn test_turbo_kernel_fractional_shift() {
    // Uniform image so weighted mean is always 1.0 regardless of overlap pattern
    let image = AstroImage::from_pixels(ImageDimensions::new(4, 4, 1), vec![1.0; 4 * 4]);

    let config = DrizzleConfig::x2().with_pixfrac(1.0); // drop_size = 2.0
    let mut acc = DrizzleAccumulator::new(ImageDimensions::new(4, 4, 1), config);
    let transform = Transform::translation(DVec2::new(0.25, 0.0));
    acc.add_image(image, &transform, 1.0, None);

    let result = acc.finalize();
    let out = result.image.channel(0);
    let w = 8usize;

    // All covered interior pixels should be 1.0
    assert!(
        (out[2 * w + 3] - 1.0).abs() < 1e-5,
        "Interior pixel should be 1.0, got {}",
        out[2 * w + 3]
    );

    // Verify asymmetric coverage from the fractional shift.
    // Input pixel (0,0) → output center (1.5, 1.0). Drop (0.5, 0.0)→(2.5, 2.0).
    // Output (0,0) gets weight 0.5*1.0*0.25 = 0.125 from input (0,0) only.
    // Output (1,0) gets weight 1.0*1.0*0.25 = 0.25 from input (0,0) AND (1,0).
    // So total weight at (1,0) = 0.25+0.25 = 0.50.
    // Coverage normalization: max_weight across all pixels.
    // Interior pixel (3,2) gets contributions from two input pixels, total weight 0.50.
    // Coverage at edge (0,0) = 0.125 / max_weight.
    // The exact max depends on full overlap pattern — just verify (0,0) < (1,0).
    assert!(
        result.coverage_at(0, 0) < result.coverage_at(1, 0),
        "Edge pixel should have less coverage than interior"
    );
}

/// Test that min_coverage works with normalized weights.
///
/// With pixfrac=1.0, scale=2: single frame, max weight = 0.25.
/// min_coverage=0.6: threshold = 0.6 * 0.25 = 0.15.
/// A pixel with overlap 0.5 → weight = 0.5*0.25 = 0.125 < 0.15 → rejected.
/// A pixel with overlap 1.0 → weight = 1.0*0.25 = 0.25 >= 0.15 → kept.
#[test]
fn test_min_coverage_normalized() {
    // Single pixel at (0,0) with fractional shift to create unequal overlaps
    let mut pixels = vec![0.0f32; 4 * 4];
    pixels[0] = 1.0;
    let image = AstroImage::from_pixels(ImageDimensions::new(4, 4, 1), pixels);

    let config = DrizzleConfig::x2().with_pixfrac(1.0).with_min_coverage(0.6);
    let mut acc = DrizzleAccumulator::new(ImageDimensions::new(4, 4, 1), config);

    // Translation (0.25, 0): creates overlaps of 0.5 and 1.0 (see previous test)
    let transform = Transform::translation(DVec2::new(0.25, 0.0));
    acc.add_image(image, &transform, 1.0, None);
    let result = acc.finalize();
    let out = result.image.channel(0);

    // Pixel (1,0) has weight 0.25 (max), pixel (0,0) has weight 0.125.
    // Threshold = 0.6 * 0.25 = 0.15. So (0,0) with 0.125 < 0.15 → fill_value.
    // Pixel (1,0) with 0.25 >= 0.15 → kept.
    assert!((out[1] - 1.0).abs() < 1e-5, "Pixel (1,0) kept: {}", out[1]);
    assert!(
        out[0].abs() < 1e-5,
        "Pixel (0,0) rejected (below min_coverage): {}",
        out[0]
    );
}

/// Test Gaussian kernel produces flux-preserving smooth output.
///
/// A uniform 10×10 image with value 3.0 through Gaussian kernel should produce
/// approximately 3.0 everywhere in the interior (edges may differ due to truncation).
#[test]
fn test_gaussian_kernel_uniform_preserves_value() {
    let image = AstroImage::from_pixels(ImageDimensions::new(10, 10, 1), vec![3.0; 10 * 10]);

    let config = DrizzleConfig::x2().with_kernel(DrizzleKernel::Gaussian);
    let mut acc = DrizzleAccumulator::new(ImageDimensions::new(10, 10, 1), config);
    acc.add_image(image, &Transform::identity(), 1.0, None);
    let result = acc.finalize();

    // Interior pixels should be ≈ 3.0 (Gaussian is normalized per-pixel)
    let out = result.image.channel(0);
    let w = 20usize;
    // Check a pixel well inside the interior
    let center_val = out[10 * w + 10];
    assert!(
        (center_val - 3.0).abs() < 0.05,
        "Interior Gaussian value should be ~3.0, got {}",
        center_val
    );
}

/// Test Lanczos kernel with scale=1, pixfrac=1.
///
/// For a uniform image at scale=1 pixfrac=1, Lanczos should also produce the same
/// uniform value, since the normalized Lanczos weights sum to 1.
#[test]
fn test_lanczos_kernel_uniform_preserves_value() {
    let image = AstroImage::from_pixels(ImageDimensions::new(20, 20, 1), vec![5.0; 20 * 20]);

    let config = DrizzleConfig {
        scale: 1.0,
        pixfrac: 1.0,
        kernel: DrizzleKernel::Lanczos,
        fill_value: 0.0,
        min_coverage: 0.0,
    };
    let mut acc = DrizzleAccumulator::new(ImageDimensions::new(20, 20, 1), config);
    acc.add_image(image, &Transform::identity(), 1.0, None);
    let result = acc.finalize();

    let out = result.image.channel(0);
    // Interior pixel well away from borders
    let center_val = out[10 * 20 + 10];
    assert!(
        (center_val - 5.0).abs() < 0.01,
        "Lanczos uniform should be ~5.0, got {}",
        center_val
    );
}

/// Test that Lanczos clamping prevents negative output.
///
/// A single bright pixel surrounded by zeros will produce negative lobes
/// in the Lanczos output. After clamping, all output should be >= 0.
#[test]
fn test_lanczos_clamping_no_negative_output() {
    let mut pixels = vec![0.0f32; 20 * 20];
    pixels[10 * 20 + 10] = 100.0; // bright point source
    let image = AstroImage::from_pixels(ImageDimensions::new(20, 20, 1), pixels);

    let config = DrizzleConfig {
        scale: 1.0,
        pixfrac: 1.0,
        kernel: DrizzleKernel::Lanczos,
        fill_value: 0.0,
        min_coverage: 0.0,
    };
    let mut acc = DrizzleAccumulator::new(ImageDimensions::new(20, 20, 1), config);
    acc.add_image(image, &Transform::identity(), 1.0, None);
    let result = acc.finalize();

    let out = result.image.channel(0);
    let min_val = out.iter().copied().fold(f32::INFINITY, f32::min);
    assert!(
        min_val >= 0.0,
        "Lanczos output should be clamped to >= 0.0, got min={}",
        min_val
    );
}

/// Test two-frame accumulation with different weights.
///
/// Frame 1: uniform value 2.0, weight 1.0
/// Frame 2: uniform value 6.0, weight 3.0
/// Expected weighted mean: (2.0*w1 + 6.0*w3) / (w1+w3) at each pixel.
/// Since pixel_weight = frame_weight * overlap * inv_area, and both frames
/// have the same overlap geometry, the weighted mean simplifies to:
/// (2.0 * 1.0 + 6.0 * 3.0) / (1.0 + 3.0) = (2 + 18) / 4 = 5.0
#[test]
fn test_two_frame_weighted_mean() {
    let image1 = AstroImage::from_pixels(ImageDimensions::new(10, 10, 1), vec![2.0; 10 * 10]);
    let image2 = AstroImage::from_pixels(ImageDimensions::new(10, 10, 1), vec![6.0; 10 * 10]);

    let config = DrizzleConfig::x2();
    let mut acc = DrizzleAccumulator::new(ImageDimensions::new(10, 10, 1), config);
    acc.add_image(image1, &Transform::identity(), 1.0, None);
    acc.add_image(image2, &Transform::identity(), 3.0, None);
    let result = acc.finalize();

    let out = result.image.channel(0);
    // Interior pixel
    let center_val = out[10 * 20 + 10];
    assert!(
        (center_val - 5.0).abs() < 1e-5,
        "Weighted mean should be 5.0, got {}",
        center_val
    );
}

/// Test pixfrac changes the drop size and thus the weight distribution.
///
/// With scale=2 and non-integer-centered drop (via translation):
/// pixfrac=1.0 → drop_size=2.0: large drop hits many output pixels
/// pixfrac=0.3 → drop_size=0.6: small drop hits fewer output pixels
#[test]
fn test_pixfrac_changes_weight_distribution() {
    // Use translation (0.1, 0.1) to avoid integer-centered drops
    let transform = Transform::translation(DVec2::new(0.1, 0.1));

    // pixfrac=1.0: drop_size=2.0, half=1.0
    // Input (2,2) center (2.6,2.6) → output (5.2,5.2). Drop (4.2,4.2)→(6.2,6.2).
    // Covers output pixels (4,4),(5,4),(6,4),(4,5),(5,5),(6,5),(4,6),(5,6),(6,6) = 9 pixels
    let mut pixels1 = vec![0.0f32; 6 * 6];
    pixels1[2 * 6 + 2] = 1.0;
    let image1 = AstroImage::from_pixels(ImageDimensions::new(6, 6, 1), pixels1);

    let config1 = DrizzleConfig::x2().with_pixfrac(1.0);
    let mut acc1 = DrizzleAccumulator::new(ImageDimensions::new(6, 6, 1), config1);
    acc1.add_image(image1, &transform, 1.0, None);
    let r1 = acc1.finalize();
    let out1 = r1.image.channel(0);
    let covered_1 = out1.iter().filter(|&&v| v > 0.01).count();

    // pixfrac=0.3: drop_size=0.6, half=0.3
    // Drop (4.9,4.9)→(5.5,5.5): overlaps (4,4),(5,4),(4,5),(5,5) = 4 pixels
    let mut pixels2 = vec![0.0f32; 6 * 6];
    pixels2[2 * 6 + 2] = 1.0;
    let image2 = AstroImage::from_pixels(ImageDimensions::new(6, 6, 1), pixels2);

    let config2 = DrizzleConfig::x2().with_pixfrac(0.3);
    let mut acc2 = DrizzleAccumulator::new(ImageDimensions::new(6, 6, 1), config2);
    acc2.add_image(image2, &transform, 1.0, None);
    let r2 = acc2.finalize();
    let out2 = r2.image.channel(0);
    let covered_2 = out2.iter().filter(|&&v| v > 0.01).count();

    // pixfrac=1.0 should cover more output pixels than pixfrac=0.3
    assert!(
        covered_1 > covered_2,
        "pixfrac=1.0 should cover more pixels ({}) than pixfrac=0.3 ({})",
        covered_1,
        covered_2
    );
}

/// Test RGB channels are handled independently.
#[test]
fn test_rgb_channels_independent() {
    let mut pixels = vec![0.0f32; 4 * 4 * 3];
    // Set pixel (1,1) to (1.0, 2.0, 3.0). Index = (1*4+1)*3 = 15.
    let idx = 15;
    pixels[idx] = 1.0;
    pixels[idx + 1] = 2.0;
    pixels[idx + 2] = 3.0;
    let image = AstroImage::from_pixels(ImageDimensions::new(4, 4, 3), pixels);

    let config = DrizzleConfig::x2();
    let mut acc = DrizzleAccumulator::new(ImageDimensions::new(4, 4, 3), config);
    acc.add_image(image, &Transform::identity(), 1.0, None);
    let result = acc.finalize();

    // Pixel (2,2) in output should have (1.0, 2.0, 3.0) (normalized by equal weight)
    let r = result.image.channel(0);
    let g = result.image.channel(1);
    let b = result.image.channel(2);
    let w = 8usize;
    assert!(
        (r[2 * w + 2] - 1.0).abs() < 1e-5,
        "R should be 1.0, got {}",
        r[2 * w + 2]
    );
    assert!(
        (g[2 * w + 2] - 2.0).abs() < 1e-5,
        "G should be 2.0, got {}",
        g[2 * w + 2]
    );
    assert!(
        (b[2 * w + 2] - 3.0).abs() < 1e-5,
        "B should be 3.0, got {}",
        b[2 * w + 2]
    );
}

/// Test scale=1 with pixfrac=1 (shift-and-add equivalent).
///
/// At scale=1, pixfrac=1: drop_size=1.0, covering exactly one output pixel per input pixel.
/// With identity transform, output should exactly equal input.
#[test]
fn test_scale1_pixfrac1_identity() {
    let pixels: Vec<f32> = (0..25).map(|i| i as f32).collect();
    let image = AstroImage::from_pixels(ImageDimensions::new(5, 5, 1), pixels.clone());

    let config = DrizzleConfig {
        scale: 1.0,
        pixfrac: 1.0,
        kernel: DrizzleKernel::Turbo,
        fill_value: 0.0,
        min_coverage: 0.0,
    };
    let mut acc = DrizzleAccumulator::new(ImageDimensions::new(5, 5, 1), config);
    acc.add_image(image, &Transform::identity(), 1.0, None);
    let result = acc.finalize();

    let out = result.image.channel(0);
    // Each input pixel center (ix+0.5, iy+0.5) → same in output.
    // drop_size=1.0, half=0.5: drop from (ix, iy) to (ix+1, iy+1).
    // Overlaps exactly output pixel (ix, iy) with area 1.0.
    // inv_area = 1.0. weight = 1.0. output = value * 1.0 / 1.0 = value.
    for (i, (&actual, &expected)) in out.iter().zip(pixels.iter()).enumerate() {
        assert!(
            (actual - expected).abs() < 1e-5,
            "Pixel {} should be {}, got {}",
            i,
            expected,
            actual
        );
    }
}

/// Test that custom fill_value appears in uncovered pixels.
///
/// Point kernel at scale=2 leaves gaps (even-coordinate pixels uncovered).
/// With fill_value = -999.0, those gaps should contain -999.0 instead of 0.0.
#[test]
fn test_fill_value_in_uncovered_pixels() {
    let image = AstroImage::from_pixels(ImageDimensions::new(4, 4, 1), vec![1.0; 16]);

    let config = DrizzleConfig {
        scale: 2.0,
        pixfrac: 0.8,
        kernel: DrizzleKernel::Point,
        fill_value: -999.0,
        min_coverage: 0.0,
    };
    let mut acc = DrizzleAccumulator::new(ImageDimensions::new(4, 4, 1), config);
    acc.add_image(image, &Transform::identity(), 1.0, None);
    let result = acc.finalize();

    let out = result.image.channel(0);
    let w = 8usize;

    // Point kernel: input (ix,iy) → output (2*ix+1, 2*iy+1).
    // Covered pixel (1,1): value = 1.0
    assert!(
        (out[w + 1] - 1.0).abs() < f32::EPSILON,
        "Covered pixel should be 1.0, got {}",
        out[w + 1]
    );
    // Uncovered pixel (0,0): fill_value = -999.0
    assert!(
        (out[0] - (-999.0)).abs() < f32::EPSILON,
        "Uncovered pixel should be -999.0, got {}",
        out[0]
    );
    // Uncovered pixel (2,2): fill_value = -999.0
    assert!(
        (out[2 * w + 2] - (-999.0)).abs() < f32::EPSILON,
        "Uncovered pixel (2,2) should be -999.0, got {}",
        out[2 * w + 2]
    );
}

/// Test that a zero-weight frame does not affect the output.
///
/// Frame 1: uniform 3.0, weight 1.0
/// Frame 2: uniform 100.0, weight 0.0
/// Result should be 3.0 everywhere (zero-weight frame contributes nothing).
#[test]
fn test_zero_weight_frame_ignored() {
    let image1 = AstroImage::from_pixels(ImageDimensions::new(8, 8, 1), vec![3.0; 64]);
    let image2 = AstroImage::from_pixels(ImageDimensions::new(8, 8, 1), vec![100.0; 64]);

    let config = DrizzleConfig::x2();
    let mut acc = DrizzleAccumulator::new(ImageDimensions::new(8, 8, 1), config);
    acc.add_image(image1, &Transform::identity(), 1.0, None);
    acc.add_image(image2, &Transform::identity(), 0.0, None);
    let result = acc.finalize();

    let out = result.image.channel(0);
    // Interior pixel: should be 3.0, not influenced by the 100.0 frame
    let center = out[8 * 16 + 8];
    assert!(
        (center - 3.0).abs() < 1e-5,
        "Zero-weight frame should not affect output, got {}",
        center
    );
}

/// Test Gaussian kernel with translation on a uniform image.
///
/// Uniform value 4.0, translation (1.0, 0.5), scale=2, pixfrac=0.8.
/// For a uniform image, the Gaussian-weighted mean at every interior pixel
/// is 4.0 (weighted average of identical values).
/// This tests that Gaussian + translation still preserves uniform values.
#[test]
fn test_gaussian_kernel_with_translation() {
    let image = AstroImage::from_pixels(ImageDimensions::new(12, 12, 1), vec![4.0; 144]);

    let config = DrizzleConfig::x2().with_kernel(DrizzleKernel::Gaussian);
    let mut acc = DrizzleAccumulator::new(ImageDimensions::new(12, 12, 1), config);
    let transform = Transform::translation(DVec2::new(1.0, 0.5));
    acc.add_image(image, &transform, 1.0, None);
    let result = acc.finalize();

    let out = result.image.channel(0);
    let w = 24usize;

    // Interior pixel well inside the translated region: should be ~4.0
    let center = out[12 * w + 14];
    assert!(
        (center - 4.0).abs() < 0.05,
        "Gaussian interior with translation should be ~4.0, got {}",
        center
    );

    // Another interior pixel
    let other = out[10 * w + 10];
    assert!(
        (other - 4.0).abs() < 0.05,
        "Gaussian interior pixel should be ~4.0, got {}",
        other
    );

    // Pixel far outside the translated input region: should be fill_value (0.0)
    // Translation (1.0, 0.5) shifts input right+down. Output pixel (0,0) is far from any input.
    assert!(
        out[0].abs() < 1e-5,
        "Far pixel should be 0.0, got {}",
        out[0]
    );
}

/// Test Lanczos kernel with translation preserves value for a uniform image.
///
/// Uniform image value 7.0, translation (0.3, -0.2), scale=1, pixfrac=1.
/// Interior pixels should still be ~7.0 since the weighted mean of uniform values is invariant.
#[test]
fn test_lanczos_kernel_with_translation() {
    let image = AstroImage::from_pixels(ImageDimensions::new(20, 20, 1), vec![7.0; 400]);

    let config = DrizzleConfig {
        scale: 1.0,
        pixfrac: 1.0,
        kernel: DrizzleKernel::Lanczos,
        fill_value: 0.0,
        min_coverage: 0.0,
    };
    let mut acc = DrizzleAccumulator::new(ImageDimensions::new(20, 20, 1), config);
    let transform = Transform::translation(DVec2::new(0.3, -0.2));
    acc.add_image(image, &transform, 1.0, None);
    let result = acc.finalize();

    let out = result.image.channel(0);
    // Interior pixel well away from borders (Lanczos radius=3, so stay 4+ pixels inside)
    let center = out[10 * 20 + 10];
    assert!(
        (center - 7.0).abs() < 0.05,
        "Lanczos with translation should preserve uniform value ~7.0, got {}",
        center
    );

    // Another interior pixel
    let other = out[8 * 20 + 12];
    assert!(
        (other - 7.0).abs() < 0.05,
        "Lanczos interior pixel should be ~7.0, got {}",
        other
    );
}

/// Test that a pixel with weight 0.0 is fully excluded from the output.
///
/// Setup: 4×4 image, pixel (1,1) = 100.0, all others = 1.0.
/// Weight map: pixel (1,1) = 0.0, all others = 1.0.
/// scale=1, pixfrac=1 (identity mapping, one-to-one).
///
/// Without weight map: output (1,1) = 100.0.
/// With weight map: pixel (1,1) contributes nothing. Output (1,1) receives
/// only flux from neighboring drops that overlap it. With scale=1,pixfrac=1
/// each input pixel maps to exactly one output pixel, so (1,1) gets no
/// contribution at all → fill_value = 0.0.
#[test]
fn test_pixel_weight_zero_excludes_pixel() {
    let mut pixels = vec![1.0f32; 4 * 4];
    pixels[5] = 100.0; // (1,1) = row 1 * 4 + col 1 = 5 (bad pixel)
    let image = AstroImage::from_pixels(ImageDimensions::new(4, 4, 1), pixels);

    let mut pw = Buffer2::new_filled(4, 4, 1.0f32);
    *pw.get_mut(1, 1) = 0.0; // Exclude (1,1)

    let config = DrizzleConfig {
        scale: 1.0,
        pixfrac: 1.0,
        kernel: DrizzleKernel::Turbo,
        fill_value: 0.0,
        min_coverage: 0.0,
    };
    let mut acc = DrizzleAccumulator::new(ImageDimensions::new(4, 4, 1), config);
    acc.add_image(image, &Transform::identity(), 1.0, Some(&pw));
    let result = acc.finalize();
    let out = result.image.channel(0);

    // (1,1) = index 5, should be fill_value (0.0) — the bad pixel was excluded
    assert!(
        out[5].abs() < 1e-5,
        "Bad pixel (1,1) should be excluded, got {}",
        out[5]
    );
    // (0,0) should be 1.0 — normal pixel unaffected
    assert!(
        (out[0] - 1.0).abs() < 1e-5,
        "Normal pixel (0,0) should be 1.0, got {}",
        out[0]
    );
    // (2,2) should be 1.0
    assert!(
        (out[2 * 4 + 2] - 1.0).abs() < 1e-5,
        "Normal pixel (2,2) should be 1.0, got {}",
        out[2 * 4 + 2]
    );
}

/// Test that per-pixel weights correctly scale contributions in weighted mean.
///
/// Two frames, same geometry (identity, scale=1, pixfrac=1):
///   Frame 1: all pixels = 2.0, pixel weight at (1,1) = 0.5
///   Frame 2: all pixels = 6.0, pixel weight at (1,1) = 1.0
///
/// At (1,1): weighted mean = (2.0 * 0.5 + 6.0 * 1.0) / (0.5 + 1.0) = 7.0 / 1.5 = 4.667
/// At other pixels: weighted mean = (2.0 * 1.0 + 6.0 * 1.0) / (1.0 + 1.0) = 4.0
#[test]
fn test_pixel_weight_scales_contribution() {
    let image1 = AstroImage::from_pixels(ImageDimensions::new(4, 4, 1), vec![2.0; 16]);
    let image2 = AstroImage::from_pixels(ImageDimensions::new(4, 4, 1), vec![6.0; 16]);

    let mut pw1 = Buffer2::new_filled(4, 4, 1.0f32);
    *pw1.get_mut(1, 1) = 0.5;
    let pw2 = Buffer2::new_filled(4, 4, 1.0f32);

    let config = DrizzleConfig {
        scale: 1.0,
        pixfrac: 1.0,
        kernel: DrizzleKernel::Turbo,
        fill_value: 0.0,
        min_coverage: 0.0,
    };
    let mut acc = DrizzleAccumulator::new(ImageDimensions::new(4, 4, 1), config);
    acc.add_image(image1, &Transform::identity(), 1.0, Some(&pw1));
    acc.add_image(image2, &Transform::identity(), 1.0, Some(&pw2));
    let result = acc.finalize();
    let out = result.image.channel(0);

    // (1,1) = index 5: (2.0*0.5 + 6.0*1.0) / (0.5+1.0) = 7.0/1.5 = 4.6667
    let expected_11 = 7.0 / 1.5;
    assert!(
        (out[5] - expected_11).abs() < 1e-5,
        "Pixel (1,1) should be {:.4}, got {}",
        expected_11,
        out[5]
    );
    // (0,0): (2.0+6.0)/2.0 = 4.0
    assert!(
        (out[0] - 4.0).abs() < 1e-5,
        "Pixel (0,0) should be 4.0, got {}",
        out[0]
    );
}

/// Test bad pixel mask: multiple zero-weight pixels in a uniform field.
///
/// 8×8 uniform image = 5.0. Weight map has 3 bad pixels at (2,3), (5,1), (7,7).
/// scale=1, pixfrac=1. Bad pixels produce fill_value; all others = 5.0.
#[test]
fn test_pixel_weight_bad_pixel_mask() {
    let image = AstroImage::from_pixels(ImageDimensions::new(8, 8, 1), vec![5.0; 64]);

    let mut pw = Buffer2::new_filled(8, 8, 1.0f32);
    *pw.get_mut(2, 3) = 0.0;
    *pw.get_mut(5, 1) = 0.0;
    *pw.get_mut(7, 7) = 0.0;

    let config = DrizzleConfig {
        scale: 1.0,
        pixfrac: 1.0,
        kernel: DrizzleKernel::Turbo,
        fill_value: -1.0,
        min_coverage: 0.0,
    };
    let mut acc = DrizzleAccumulator::new(ImageDimensions::new(8, 8, 1), config);
    acc.add_image(image, &Transform::identity(), 1.0, Some(&pw));
    let result = acc.finalize();
    let out = result.image.channel(0);

    // Bad pixels → fill_value = -1.0
    assert!(
        (out[3 * 8 + 2] - (-1.0)).abs() < 1e-5,
        "Bad pixel (2,3) should be fill, got {}",
        out[3 * 8 + 2]
    );
    assert!(
        (out[13] - (-1.0)).abs() < 1e-5, // (5,1) = row 1 * 8 + col 5
        "Bad pixel (5,1) should be fill, got {}",
        out[13]
    );
    assert!(
        (out[7 * 8 + 7] - (-1.0)).abs() < 1e-5,
        "Bad pixel (7,7) should be fill, got {}",
        out[7 * 8 + 7]
    );

    // Good pixels → 5.0
    assert!(
        (out[0] - 5.0).abs() < 1e-5,
        "Good pixel (0,0) should be 5.0, got {}",
        out[0]
    );
    assert!(
        (out[4 * 8 + 4] - 5.0).abs() < 1e-5,
        "Good pixel (4,4) should be 5.0, got {}",
        out[4 * 8 + 4]
    );

    // Count: 3 bad pixels → fill, 61 good → 5.0
    let bad_count = out.iter().filter(|&&v| (v - (-1.0)).abs() < 1e-5).count();
    let good_count = out.iter().filter(|&&v| (v - 5.0).abs() < 1e-5).count();
    assert_eq!(bad_count, 3, "Expected 3 bad pixels");
    assert_eq!(good_count, 61, "Expected 61 good pixels");
}

/// Test per-pixel weights with point kernel.
///
/// scale=2: input (ix,iy) → output (2*ix+1, 2*iy+1). Point kernel = 1 output pixel.
/// Pixel (1,1) = 10.0, weight = 0.0. Should not appear at output (3,3).
/// Pixel (0,0) = 3.0, weight = 1.0. Should appear at output (1,1) = 3.0.
#[test]
fn test_pixel_weight_with_point_kernel() {
    let mut pixels = vec![1.0f32; 4 * 4];
    pixels[5] = 10.0; // (1,1) bad pixel
    pixels[0] = 3.0;
    let image = AstroImage::from_pixels(ImageDimensions::new(4, 4, 1), pixels);

    let mut pw = Buffer2::new_filled(4, 4, 1.0f32);
    *pw.get_mut(1, 1) = 0.0;

    let config = DrizzleConfig::x2().with_kernel(DrizzleKernel::Point);
    let mut acc = DrizzleAccumulator::new(ImageDimensions::new(4, 4, 1), config);
    acc.add_image(image, &Transform::identity(), 1.0, Some(&pw));
    let result = acc.finalize();
    let out = result.image.channel(0);
    let w = 8usize;

    // Output (3,3) ← input (1,1): excluded by weight=0 → fill_value 0.0
    assert!(
        out[3 * w + 3].abs() < 1e-5,
        "Excluded pixel output (3,3) should be 0.0, got {}",
        out[3 * w + 3]
    );
    // Output (1,1) ← input (0,0): weight=1.0, value=3.0
    assert!(
        (out[w + 1] - 3.0).abs() < 1e-5,
        "Good pixel output (1,1) should be 3.0, got {}",
        out[w + 1]
    );
}

/// Test per-pixel weights with Gaussian kernel on a uniform image.
///
/// Uniform image = 4.0. One bad pixel at (5,5) with weight 0.0.
/// Gaussian kernel spreads flux over multiple output pixels, so the bad pixel
/// reduces coverage near its position but doesn't contaminate values.
/// Interior pixels far from the bad pixel should still be ~4.0.
#[test]
fn test_pixel_weight_with_gaussian_kernel() {
    let image = AstroImage::from_pixels(ImageDimensions::new(12, 12, 1), vec![4.0; 12 * 12]);

    let mut pw = Buffer2::new_filled(12, 12, 1.0f32);
    *pw.get_mut(5, 5) = 0.0;

    let config = DrizzleConfig::x2().with_kernel(DrizzleKernel::Gaussian);
    let mut acc = DrizzleAccumulator::new(ImageDimensions::new(12, 12, 1), config);
    acc.add_image(image, &Transform::identity(), 1.0, Some(&pw));
    let result = acc.finalize();
    let out = result.image.channel(0);
    let w = 24usize;

    // Far from bad pixel: should be ~4.0
    let far = out[2 * w + 2];
    assert!(
        (far - 4.0).abs() < 0.05,
        "Far pixel should be ~4.0, got {}",
        far
    );

    // Another far pixel
    let far2 = out[20 * w + 20];
    assert!(
        (far2 - 4.0).abs() < 0.05,
        "Far pixel should be ~4.0, got {}",
        far2
    );
}

/// Test that pixel_weights with wrong dimensions panics.
#[test]
#[should_panic(expected = "Pixel weight map dimensions")]
fn test_pixel_weight_dimensions_mismatch_panics() {
    let image = AstroImage::from_pixels(ImageDimensions::new(4, 4, 1), vec![1.0; 16]);
    let pw = Buffer2::new_filled(3, 3, 1.0f32); // Wrong size!

    let config = DrizzleConfig::x2();
    let mut acc = DrizzleAccumulator::new(ImageDimensions::new(4, 4, 1), config);
    acc.add_image(image, &Transform::identity(), 1.0, Some(&pw));
}

// ==================== sgarea() unit tests ====================

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

// ==================== boxer() unit tests ====================

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

// ==================== Square kernel integration tests ====================

/// Test square kernel with identity transform, uniform image, scale=1, pixfrac=1.
///
/// Each input pixel maps to exactly one output pixel with overlap = 1.0.
/// Jacobian = scale² * pixfrac² = 1.0 (area of unit square in output space).
/// Weight = overlap / jaco = 1.0 / 1.0 = 1.0.
/// Output = input value.
#[test]
fn test_square_kernel_identity_uniform() {
    let pixels: Vec<f32> = (0..25).map(|i| i as f32).collect();
    let image = AstroImage::from_pixels(ImageDimensions::new(5, 5, 1), pixels.clone());

    let config = DrizzleConfig {
        scale: 1.0,
        pixfrac: 1.0,
        kernel: DrizzleKernel::Square,
        fill_value: 0.0,
        min_coverage: 0.0,
    };
    let mut acc = DrizzleAccumulator::new(ImageDimensions::new(5, 5, 1), config);
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
    let image1 = AstroImage::from_pixels(ImageDimensions::new(10, 10, 1), pixels.clone());
    let image2 = AstroImage::from_pixels(ImageDimensions::new(10, 10, 1), pixels);
    let transform = Transform::translation(DVec2::new(0.3, -0.2));

    // Turbo
    let config_turbo = DrizzleConfig {
        scale: 2.0,
        pixfrac: 0.8,
        kernel: DrizzleKernel::Turbo,
        fill_value: 0.0,
        min_coverage: 0.0,
    };
    let mut acc_turbo = DrizzleAccumulator::new(ImageDimensions::new(10, 10, 1), config_turbo);
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
    let mut acc_square = DrizzleAccumulator::new(ImageDimensions::new(10, 10, 1), config_square);
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
    let image = AstroImage::from_pixels(ImageDimensions::new(20, 20, 1), vec![3.0; 20 * 20]);

    // 45° rotation around center (10, 10)
    let angle = std::f64::consts::FRAC_PI_4;
    let cos_a = angle.cos();
    let sin_a = angle.sin();
    let cx = 10.0;
    let cy = 10.0;
    // Rotation matrix (row-major): translate to origin, rotate, translate back
    // x' = cos*(x-cx) - sin*(y-cy) + cx
    // y' = sin*(x-cx) + cos*(y-cy) + cy
    use crate::math::DMat3;
    use crate::registration::TransformType;
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
    let mut acc = DrizzleAccumulator::new(ImageDimensions::new(20, 20, 1), config);
    acc.add_image(image, &transform, 1.0, None);
    let result = acc.finalize();
    let out = result.image.channel(0);

    // Verify coverage exists in the interior (rotated image should still cover center)
    let center_coverage = result.coverage_at(10, 10);
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
    let image1 = AstroImage::from_pixels(ImageDimensions::new(6, 6, 1), pixels1);

    let config1 = DrizzleConfig {
        scale: 2.0,
        pixfrac: 1.0,
        kernel: DrizzleKernel::Square,
        fill_value: 0.0,
        min_coverage: 0.0,
    };
    let mut acc1 = DrizzleAccumulator::new(ImageDimensions::new(6, 6, 1), config1);
    acc1.add_image(image1, &transform, 1.0, None);
    let r1 = acc1.finalize();
    let covered_1 = r1.image.channel(0).iter().filter(|&&v| v > 0.01).count();

    let mut pixels2 = vec![0.0f32; 6 * 6];
    pixels2[2 * 6 + 2] = 1.0;
    let image2 = AstroImage::from_pixels(ImageDimensions::new(6, 6, 1), pixels2);

    let config2 = DrizzleConfig {
        scale: 2.0,
        pixfrac: 0.5,
        kernel: DrizzleKernel::Square,
        fill_value: 0.0,
        min_coverage: 0.0,
    };
    let mut acc2 = DrizzleAccumulator::new(ImageDimensions::new(6, 6, 1), config2);
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
    let image = AstroImage::from_pixels(ImageDimensions::new(4, 4, 1), vec![5.0; 16]);

    let mut pw = Buffer2::new_filled(4, 4, 1.0f32);
    *pw.get_mut(1, 1) = 0.0;

    let config = DrizzleConfig {
        scale: 1.0,
        pixfrac: 1.0,
        kernel: DrizzleKernel::Square,
        fill_value: -1.0,
        min_coverage: 0.0,
    };
    let mut acc = DrizzleAccumulator::new(ImageDimensions::new(4, 4, 1), config);
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
/// scale=2, pixfrac=0.8: drop in input space is 0.8×0.8 centered at pixel center.
/// Input pixel (1,1) center at (1.5, 1.5). Drop corners:
///   (1.1, 1.1), (1.9, 1.1), (1.9, 1.9), (1.1, 1.9)
/// Identity transform, scaled to output: × 2.0 →
///   (2.2, 2.2), (3.8, 2.2), (3.8, 3.8), (2.2, 3.8)
/// Jacobian = (3.8-2.2) * (2.2-3.8) - (2.2-3.8) * (2.2-3.8)
///          ... via formula: 0.5 * ((x1-x3)*(y0-y2) - (x0-x2)*(y1-y3))
///   x1-x3 = 3.8-2.2 = 1.6, y0-y2 = 2.2-3.8 = -1.6
///   x0-x2 = 2.2-3.8 = -1.6, y1-y3 = 2.2-3.8 = -1.6
///   jaco = 0.5 * (1.6*(-1.6) - (-1.6)*(-1.6)) = 0.5 * (-2.56 - 2.56) = -2.56
///   abs_jaco = 2.56 = (1.6)² = (pixfrac * scale)²  ✓
///
/// Output pixel (2,2) overlap with quad: both span [2.2,3] in x and [2.2,3] in y → 0.8*0.8 = 0.64
/// weight = overlap / jaco = 0.64 / 2.56 = 0.25
/// Finalized = value * weight / weight = value = 2.0  (same as turbo)
#[test]
fn test_square_kernel_scale2_single_pixel() {
    let mut pixels = vec![0.0f32; 4 * 4];
    pixels[5] = 2.0; // pixel (1,1)
    let image = AstroImage::from_pixels(ImageDimensions::new(4, 4, 1), pixels);

    let config = DrizzleConfig {
        scale: 2.0,
        pixfrac: 0.8,
        kernel: DrizzleKernel::Square,
        fill_value: 0.0,
        min_coverage: 0.0,
    };
    let mut acc = DrizzleAccumulator::new(ImageDimensions::new(4, 4, 1), config);
    acc.add_image(image, &Transform::identity(), 1.0, None);
    let result = acc.finalize();
    let out = result.image.channel(0);
    let w = 8usize;

    // Output pixels (2,2), (3,2), (2,3), (3,3) should all be 2.0
    assert!(
        (out[2 * w + 2] - 2.0).abs() < 1e-4,
        "Expected 2.0 at (2,2), got {}",
        out[2 * w + 2]
    );
    assert!(
        (out[2 * w + 3] - 2.0).abs() < 1e-4,
        "Expected 2.0 at (3,2), got {}",
        out[2 * w + 3]
    );
    assert!(
        (out[3 * w + 2] - 2.0).abs() < 1e-4,
        "Expected 2.0 at (2,3), got {}",
        out[3 * w + 2]
    );
    assert!(
        (out[3 * w + 3] - 2.0).abs() < 1e-4,
        "Expected 2.0 at (3,3), got {}",
        out[3 * w + 3]
    );

    // Pixels outside the drop should be 0.0
    assert!(out[w + 2].abs() < 1e-5, "No flux at (2,1)");
    assert!(out[4 * w + 3].abs() < 1e-5, "No flux at (3,4)");
}

/// Test Jacobian correctness by verifying weights at output pixels where two
/// input pixels with different values overlap.
///
/// scale=2, pixfrac=1.0. Input: pixel (0,0)=10.0, pixel (1,0)=20.0.
/// Each input pixel has dh = 0.5. Identity transform.
///
/// Input pixel (0,0): center (0.5, 0.5), corners (0,0)→(1,0)→(1,1)→(0,1).
///   Scaled to output: (0,0)→(2,0)→(2,2)→(0,2). Jaco = 0.5*((2-0)*(0-2)-(0-2)*(0-2))
///   = 0.5*(2*(-2)-(-2)*(-2)) = 0.5*(-4-4) = -4.0. abs_jaco = 4.0.
///   At output (1,0): overlap with quad [0,2]×[0,2] on pixel [1,2]×[0,1] = 1.0×1.0 = 1.0.
///   weight_00 = 1.0 * 1.0 / 4.0 = 0.25.
///
/// Input pixel (1,0): center (1.5, 0.5), corners (1,0)→(2,0)→(2,1)→(1,1).
///   Scaled: (2,0)→(4,0)→(4,2)→(2,2). Same jaco = 4.0.
///   At output (2,0): overlap with quad [2,4]×[0,2] on pixel [2,3]×[0,1] = 1.0×1.0 = 1.0.
///   weight_10 = 0.25.
///
/// Output pixel (1,0) [spans [1,2]×[0,1]]: only pixel (0,0) contributes.
///   finalized = (10.0 * 0.25) / 0.25 = 10.0  ✓
/// Output pixel (2,0) [spans [2,3]×[0,1]]: only pixel (1,0) contributes.
///   finalized = (20.0 * 0.25) / 0.25 = 20.0  ✓
///
/// Now the critical test: output pixel at the boundary where BOTH overlap.
/// For output pixel (1,0) = [1,2]×[0,1]:
///   pixel (0,0) quad [0,2]×[0,2]: overlap = min(2,2)-max(0,1) × min(2,1)-max(0,0) = 1×1 = 1.0
///   → BUT that's only pixel (0,0). Pixel (1,0) quad [2,4]×[0,2]: overlap with [1,2]×[0,1]
///   = min(4,2)-max(2,1) = 0 (the quad starts at x=2, pixel ends at x=2, overlap = 0).
/// So output (1,0) = 10.0 only. No mixing.
///
/// For non-trivial mixing, use pixfrac=1 + fractional translation:
/// Translation (0.25, 0): pixel (0,0) center at (0.75, 0.5), scaled to (1.5, 1.0).
/// Quad corners in output: (0.5,0)→(2.5,0)→(2.5,2)→(0.5,2).
/// Pixel (1,0) center at (1.75, 0.5), scaled to (3.5, 1.0).
/// Quad corners in output: (2.5,0)→(4.5,0)→(4.5,2)→(2.5,2).
///
/// Output pixel (2,0) = [2,3]×[0,1]:
///   From pixel (0,0): overlap with [0.5,2.5]×[0,2] on [2,3]×[0,1]
///     = (min(2.5,3)-max(0.5,2)) × (min(2,1)-max(0,0)) = 0.5 × 1.0 = 0.5
///     weight = 0.5 / 4.0 = 0.125. data += 10.0 * 0.125 = 1.25
///   From pixel (1,0): overlap with [2.5,4.5]×[0,2] on [2,3]×[0,1]
///     = (min(4.5,3)-max(2.5,2)) × (min(2,1)-max(0,0)) = 0.5 × 1.0 = 0.5
///     weight = 0.5 / 4.0 = 0.125. data += 20.0 * 0.125 = 2.50
///   finalized = (1.25 + 2.50) / (0.125 + 0.125) = 3.75 / 0.25 = 15.0
///
/// This is the correct weighted average: equal overlap areas → average = (10+20)/2 = 15.0  ✓
#[test]
fn test_square_kernel_jacobian_weighted_average() {
    let mut pixels = vec![0.0f32; 4 * 4];
    pixels[0] = 10.0; // pixel (0,0)
    pixels[1] = 20.0; // pixel (1,0)
    let image = AstroImage::from_pixels(ImageDimensions::new(4, 4, 1), pixels);

    let config = DrizzleConfig {
        scale: 2.0,
        pixfrac: 1.0,
        kernel: DrizzleKernel::Square,
        fill_value: 0.0,
        min_coverage: 0.0,
    };
    let mut acc = DrizzleAccumulator::new(ImageDimensions::new(4, 4, 1), config);
    let transform = Transform::translation(DVec2::new(0.25, 0.0));
    acc.add_image(image, &transform, 1.0, None);
    let result = acc.finalize();
    let out = result.image.channel(0);

    // Output pixel (2,0): weighted average of pixel(0,0)=10 and pixel(1,0)=20
    // with equal overlap 0.5 each → (10*0.125 + 20*0.125) / 0.25 = 15.0
    assert!(
        (out[2] - 15.0).abs() < 1e-3,
        "Output (2,0) should be 15.0 (equal-weight avg of 10 and 20), got {}",
        out[2]
    );

    // Output pixel (1,0): only pixel(0,0) contributes (overlap 1.0)
    // finalized = 10.0
    assert!(
        (out[1] - 10.0).abs() < 1e-3,
        "Output (1,0) should be 10.0 (only pixel 0,0), got {}",
        out[1]
    );

    // Output pixel (3,0): only pixel(1,0) contributes (overlap 1.0)
    // finalized = 20.0
    assert!(
        (out[3] - 20.0).abs() < 1e-3,
        "Output (3,0) should be 20.0 (only pixel 1,0), got {}",
        out[3]
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
    let cos30 = (std::f64::consts::PI / 6.0).cos();
    let sin30 = (std::f64::consts::PI / 6.0).sin();
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
    let image1 = AstroImage::from_pixels(ImageDimensions::new(10, 10, 1), pixels.clone());
    let image2 = AstroImage::from_pixels(ImageDimensions::new(10, 10, 1), pixels);

    // 15° rotation around center — enough to produce measurable overlap differences
    let angle = 15.0_f64.to_radians();
    let cos_a = angle.cos();
    let sin_a = angle.sin();
    let cx = 5.0;
    let cy = 5.0;
    use crate::math::DMat3;
    use crate::registration::TransformType;
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
    let mut acc_turbo = DrizzleAccumulator::new(ImageDimensions::new(10, 10, 1), config_turbo);
    acc_turbo.add_image(image1, &transform, 1.0, None);
    let result_turbo = acc_turbo.finalize();

    let config_square = DrizzleConfig {
        scale: 1.0,
        pixfrac: 1.0,
        kernel: DrizzleKernel::Square,
        fill_value: 0.0,
        min_coverage: 0.0,
    };
    let mut acc_square = DrizzleAccumulator::new(ImageDimensions::new(10, 10, 1), config_square);
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

    let image = AstroImage::from_pixels(ImageDimensions::new(20, 20, 1), pixels);

    // 15° rotation around center
    let angle = 15.0_f64.to_radians();
    let cos_a = angle.cos();
    let sin_a = angle.sin();
    let cx = 10.0;
    let cy = 10.0;
    use crate::math::DMat3;
    use crate::registration::TransformType;
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
    let mut acc = DrizzleAccumulator::new(ImageDimensions::new(20, 20, 1), config);
    acc.add_image(image, &transform, 1.0, None);

    // Before finalize, check raw weighted flux sum.
    // data[c] = sum(flux * weight) per output pixel, weights[c] = sum(weight).
    // total_data = sum over all output pixels of data = sum over all input pixels of
    //   flux * sum_over_output_of(overlap/jaco)
    // For each input pixel, sum_over_output_of(overlap) = total_overlap = jaco (the quad
    // area), so sum_over_output_of(overlap/jaco) = 1.0 for fully interior pixels.
    // Therefore total_data ≈ sum(flux) = 544 for interior pixels.
    let data_sum: f32 = acc.data[0].pixels().iter().sum();

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
    let image1 = AstroImage::from_pixels(ImageDimensions::new(6, 6, 1), vec![2.0; 36]);
    let image2 = AstroImage::from_pixels(ImageDimensions::new(6, 6, 1), vec![8.0; 36]);

    let config = DrizzleConfig {
        scale: 1.0,
        pixfrac: 1.0,
        kernel: DrizzleKernel::Square,
        fill_value: 0.0,
        min_coverage: 0.0,
    };
    let mut acc = DrizzleAccumulator::new(ImageDimensions::new(6, 6, 1), config);
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

// ========================================================================
// Jacobian correction tests
// ========================================================================

#[test]
fn test_local_jacobian_identity_scale1() {
    // Identity transform, scale=1: one input pixel maps to exactly one output pixel.
    // Jacobian = |det(I)| * 1² = 1.0
    let transform = Transform::identity();
    let center = transform.apply(DVec2::new(5.5, 5.5));
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
    let center = transform.apply(DVec2::new(5.5, 5.5));
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
    use crate::math::DMat3;
    use crate::registration::TransformType;
    let angle = 30.0_f64.to_radians();
    let (sin_a, cos_a) = angle.sin_cos();
    let matrix = DMat3::from_rows([cos_a, -sin_a, 0.0], [sin_a, cos_a, 0.0], [0.0, 0.0, 1.0]);
    let transform = Transform::from_matrix(matrix, TransformType::Euclidean);
    let center = transform.apply(DVec2::new(50.5, 50.5));
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
    use crate::math::DMat3;
    use crate::registration::TransformType;
    let matrix = DMat3::from_rows([2.0, 0.0, 0.0], [0.0, 3.0, 0.0], [0.0, 0.0, 1.0]);
    let transform = Transform::from_matrix(matrix, TransformType::Affine);
    let center = transform.apply(DVec2::new(5.5, 5.5));
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
    use crate::math::DMat3;
    use crate::registration::TransformType;
    let matrix = DMat3::from_rows(
        [1.0, 0.0, 0.0],
        [0.0, 1.0, 0.0],
        [1e-4, 0.0, 1.0], // perspective: w = 1e-4 * x + 1
    );
    let transform = Transform::from_matrix(matrix, TransformType::Homography);

    // At x=0: w ≈ 1, minimal distortion
    let c0 = transform.apply(DVec2::new(0.5, 0.5));
    let jaco_left = local_jacobian(&transform, c0, 0, 0, 1.0);

    // At x=1000: w ≈ 1.1, noticeable distortion
    let c1000 = transform.apply(DVec2::new(1000.5, 0.5));
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

// ---- Two-frame Jacobian correctness tests ----
//
// Strategy: Two frames contribute to the same output pixel with different
// local Jacobians. Frame A uses identity (jaco=1), Frame B uses scale-2x
// transform (jaco=4). Both have frame_weight=1.
//
// For Point kernel at scale=1:
//   Frame A pixel (5,5) → center (5.5, 5.5) → output (5,5), jaco_A = 1.
//   Frame B pixel (2,2) → center (2.5, 2.5) → transformed (5.0, 5.0) → output (5,5), jaco_B = 4.
//   Weight_A = 1/jaco_A = 1.0
//   Weight_B = 1/jaco_B = 0.25
//   output = (val_A * 1.0 + val_B * 0.25) / (1.0 + 0.25) = (val_A + 0.25 * val_B) / 1.25
//
// With val_A=10, val_B=0: output = 10/1.25 = 8.0  (with Jacobian)
//   vs output = (10+0)/2 = 5.0 if both had equal weight (no Jacobian).

/// Build a scale-2x transform: maps (x,y) → (2x, 2y).
fn make_scale2x_transform() -> Transform {
    use crate::math::DMat3;
    use crate::registration::TransformType;
    let matrix = DMat3::from_rows([2.0, 0.0, 0.0], [0.0, 2.0, 0.0], [0.0, 0.0, 1.0]);
    Transform::from_matrix(matrix, TransformType::Affine)
}

#[test]
fn test_point_jacobian_two_frame_weighted_mean() {
    // Frame A: identity, val=10. Pixel(5,5) → output(5,5), jaco=1, w=1.
    // Frame B: scale-2x, val=0. Pixel(2,2) → (5,5) → output(5,5), jaco=4, w=0.25.
    // Expected output(5,5) = (10 * 1 + 0 * 0.25) / (1 + 0.25) = 8.0
    let w = 12;
    let h = 12;
    let mut pixels_a = vec![0.0f32; w * h];
    pixels_a[5 * w + 5] = 10.0;
    let image_a = AstroImage::from_pixels(ImageDimensions::new(w, h, 1), pixels_a);

    let mut pixels_b = vec![0.0f32; w * h];
    pixels_b[2 * w + 2] = 0.0; // explicitly zero
    let image_b = AstroImage::from_pixels(ImageDimensions::new(w, h, 1), pixels_b);

    let config = DrizzleConfig {
        scale: 1.0,
        pixfrac: 1.0,
        kernel: DrizzleKernel::Point,
        ..Default::default()
    };

    let mut acc = DrizzleAccumulator::new(ImageDimensions::new(w, h, 1), config);
    acc.add_image(image_a, &Transform::identity(), 1.0, None);
    acc.add_image(image_b, &make_scale2x_transform(), 1.0, None);
    let result = acc.finalize();

    // output(5,5) = (10*1 + 0*0.25) / (1+0.25) = 8.0
    let out = result.image.channel(0);
    let val = out[5 * w + 5];
    assert!(
        (val - 8.0).abs() < 0.01,
        "Point Jacobian: pixel (5,5) = {val}, expected 8.0"
    );
}

#[test]
fn test_point_jacobian_two_frame_both_nonzero() {
    // Frame A: identity, val=10. Pixel(5,5) → output(5,5), jaco=1, w=1.
    // Frame B: scale-2x, val=2. Pixel(2,2) → output(5,5), jaco=4, w=0.25.
    // Expected = (10*1 + 2*0.25) / (1+0.25) = 10.5/1.25 = 8.4
    let w = 12;
    let h = 12;
    let mut pixels_a = vec![0.0f32; w * h];
    pixels_a[5 * w + 5] = 10.0;
    let image_a = AstroImage::from_pixels(ImageDimensions::new(w, h, 1), pixels_a);

    let mut pixels_b = vec![0.0f32; w * h];
    pixels_b[2 * w + 2] = 2.0;
    let image_b = AstroImage::from_pixels(ImageDimensions::new(w, h, 1), pixels_b);

    let config = DrizzleConfig {
        scale: 1.0,
        pixfrac: 1.0,
        kernel: DrizzleKernel::Point,
        ..Default::default()
    };

    let mut acc = DrizzleAccumulator::new(ImageDimensions::new(w, h, 1), config);
    acc.add_image(image_a, &Transform::identity(), 1.0, None);
    acc.add_image(image_b, &make_scale2x_transform(), 1.0, None);
    let result = acc.finalize();

    // output(5,5) = (10*1 + 2*0.25) / (1+0.25) = 10.5/1.25 = 8.4
    let out = result.image.channel(0);
    let val = out[5 * w + 5];
    assert!(
        (val - 8.4).abs() < 0.01,
        "Point Jacobian: pixel (5,5) = {val}, expected 8.4"
    );
}

#[test]
fn test_turbo_jacobian_two_frame_weighted_mean() {
    // Turbo kernel, pixfrac=1, scale=1. Drop size = 1×1.
    // Frame A: identity, val=10. Pixel(5,5) center→(5.5,5.5). Drop covers output(5,5) fully.
    //   overlap=1.0, inv_area=1.0, jaco=1. Weight = 1*1*1/1 = 1.0
    // Frame B: scale-2x, val=2. Pixel(2,2) center→(5.0,5.0). Drop [4.5,5.5]×[4.5,5.5].
    //   overlap with output(5,5)=[5,6]: min(5.5,6)-max(4.5,5)=0.5 in x, same in y → 0.25.
    //   inv_area=1.0, jaco=4. Weight = 1*0.25*1/4 = 0.0625
    // output(5,5) = (10*1.0 + 2*0.0625) / (1.0+0.0625) = 10.125/1.0625 = 9.5294...
    let w = 12;
    let h = 12;
    let mut pixels_a = vec![0.0f32; w * h];
    pixels_a[5 * w + 5] = 10.0;
    let image_a = AstroImage::from_pixels(ImageDimensions::new(w, h, 1), pixels_a);

    let mut pixels_b = vec![0.0f32; w * h];
    pixels_b[2 * w + 2] = 2.0;
    let image_b = AstroImage::from_pixels(ImageDimensions::new(w, h, 1), pixels_b);

    let config = DrizzleConfig {
        scale: 1.0,
        pixfrac: 1.0,
        kernel: DrizzleKernel::Turbo,
        ..Default::default()
    };

    let mut acc = DrizzleAccumulator::new(ImageDimensions::new(w, h, 1), config);
    acc.add_image(image_a, &Transform::identity(), 1.0, None);
    acc.add_image(image_b, &make_scale2x_transform(), 1.0, None);
    let result = acc.finalize();

    // data = 10*1.0 + 2*0.0625 = 10.125
    // weight = 1.0 + 0.0625 = 1.0625
    // output = 10.125/1.0625 = 9.5294...
    let expected = 10.125 / 1.0625;
    let out = result.image.channel(0);
    let val = out[5 * w + 5];
    assert!(
        (val - expected as f32).abs() < 0.02,
        "Turbo Jacobian: pixel (5,5) = {val}, expected {expected:.4}"
    );
}

#[test]
fn test_turbo_matches_square_affine_with_jacobian() {
    // For affine transforms (constant Jacobian), Turbo with Jacobian and Square
    // should produce identical output on a gradient image (non-trivial content).
    // Using a translation of (0.3, 0.7) — axis-aligned, so Turbo drop = true quad.
    use crate::math::DMat3;
    use crate::registration::TransformType;

    let w = 16;
    let h = 16;
    // Gradient: val = x + y*0.5
    let pixels: Vec<f32> = (0..w * h)
        .map(|i| {
            let x = (i % w) as f32;
            let y = (i / w) as f32;
            x + y * 0.5
        })
        .collect();

    let matrix = DMat3::from_rows([1.0, 0.0, 0.3], [0.0, 1.0, 0.7], [0.0, 0.0, 1.0]);
    let transform = Transform::from_matrix(matrix, TransformType::Affine);

    let config_turbo = DrizzleConfig {
        scale: 1.0,
        pixfrac: 0.8,
        kernel: DrizzleKernel::Turbo,
        ..Default::default()
    };
    let config_square = DrizzleConfig {
        scale: 1.0,
        pixfrac: 0.8,
        kernel: DrizzleKernel::Square,
        ..Default::default()
    };

    let image_turbo = AstroImage::from_pixels(ImageDimensions::new(w, h, 1), pixels.clone());
    let image_square = AstroImage::from_pixels(ImageDimensions::new(w, h, 1), pixels);

    let mut acc_turbo = DrizzleAccumulator::new(ImageDimensions::new(w, h, 1), config_turbo);
    acc_turbo.add_image(image_turbo, &transform, 1.0, None);
    let result_turbo = acc_turbo.finalize();

    let mut acc_square = DrizzleAccumulator::new(ImageDimensions::new(w, h, 1), config_square);
    acc_square.add_image(image_square, &transform, 1.0, None);
    let result_square = acc_square.finalize();

    let out_turbo = result_turbo.image.channel(0);
    let out_square = result_square.image.channel(0);
    let ow = result_turbo.image.width();

    // Check interior pixels where both kernels have full coverage
    let mut max_diff = 0.0f32;
    let mut checked = 0;
    for y in 2..14 {
        for x in 2..14 {
            let vt = out_turbo[y * ow + x];
            let vs = out_square[y * ow + x];
            if vt > 0.0 && vs > 0.0 {
                let diff = (vt - vs).abs();
                max_diff = max_diff.max(diff);
                checked += 1;
            }
        }
    }
    assert!(
        checked > 50,
        "Need sufficient interior pixels, got {checked}"
    );
    assert!(
        max_diff < 1e-3,
        "Turbo vs Square (affine translation): max_diff={max_diff:.6}, expected <1e-3"
    );
}

#[test]
fn test_gaussian_jacobian_two_frame_weighted_mean() {
    // Two uniform frames with different transforms → Jacobian affects weighted mean.
    // Frame A: uniform 10.0, identity (jaco=1 everywhere).
    // Frame B: uniform 2.0, scale-2x (jaco=4 everywhere).
    // At scale=1, Frame A has ~12 pixels contributing to each interior output pixel
    // (all with jaco=1), Frame B has ~3 pixels (each with jaco=4, so weight ÷ 4).
    //
    // Full Python simulation of the drizzle accumulation loop gives:
    //   With Jacobian: output(5,5) = 9.5294
    //   Without Jacobian (jaco=1 for both): output = 8.4
    // The Jacobian correctly down-weights the magnified Frame B pixels.
    let w = 12;
    let h = 12;
    let image_a = AstroImage::from_pixels(ImageDimensions::new(w, h, 1), vec![10.0; w * h]);
    let image_b = AstroImage::from_pixels(ImageDimensions::new(w, h, 1), vec![2.0; w * h]);

    let config = DrizzleConfig {
        scale: 1.0,
        pixfrac: 1.0,
        kernel: DrizzleKernel::Gaussian,
        ..Default::default()
    };

    let mut acc = DrizzleAccumulator::new(ImageDimensions::new(w, h, 1), config);
    acc.add_image(image_a, &Transform::identity(), 1.0, None);
    acc.add_image(image_b, &make_scale2x_transform(), 1.0, None);
    let result = acc.finalize();

    let out = result.image.channel(0);
    let val = out[5 * w + 5];
    // With Jacobian: Frame A (jaco=1) gets 4× more weight per pixel than Frame B (jaco=4).
    // Expected ≈ 9.529 (Python simulation). Without Jacobian it would be 8.4.
    assert!(
        (val - 9.529).abs() < 0.05,
        "Gaussian Jacobian: pixel (5,5) = {val}, expected ≈9.529"
    );
    // Must differ from the no-Jacobian value of 8.4 by a significant margin
    assert!(
        val > 9.0,
        "Gaussian Jacobian: {val} should be > 9.0 (not the 8.4 no-Jacobian value)"
    );
}

#[test]
fn test_lanczos_jacobian_two_frame_weighted_mean() {
    // Same setup as Gaussian: two uniform frames, identity vs scale-2x.
    // Lanczos-3 kernel, scale=1, pixfrac=1.
    // Full Python simulation gives the same result as Gaussian because with
    // uniform images the kernel shape cancels out; only pixel count × Jacobian matters:
    //   With Jacobian: output(5,5) = 9.5294
    //   Without Jacobian: output = 8.4
    let w = 12;
    let h = 12;
    let image_a = AstroImage::from_pixels(ImageDimensions::new(w, h, 1), vec![10.0; w * h]);
    let image_b = AstroImage::from_pixels(ImageDimensions::new(w, h, 1), vec![2.0; w * h]);

    let config = DrizzleConfig {
        scale: 1.0,
        pixfrac: 1.0,
        kernel: DrizzleKernel::Lanczos,
        ..Default::default()
    };

    let mut acc = DrizzleAccumulator::new(ImageDimensions::new(w, h, 1), config);
    acc.add_image(image_a, &Transform::identity(), 1.0, None);
    acc.add_image(image_b, &make_scale2x_transform(), 1.0, None);
    let result = acc.finalize();

    let out = result.image.channel(0);
    let val = out[5 * w + 5];
    // With Jacobian: ≈9.529 (Python). Without Jacobian: 8.4.
    assert!(
        (val - 9.529).abs() < 0.05,
        "Lanczos Jacobian: pixel (5,5) = {val}, expected ≈9.529"
    );
    assert!(
        val > 9.0,
        "Lanczos Jacobian: {val} should be > 9.0 (not the 8.4 no-Jacobian value)"
    );
}

#[test]
fn test_all_kernels_jacobian_matches_square_affine() {
    // For a pure translation (affine, constant Jacobian=1), all kernels with
    // pixfrac=1, scale=1 should produce similar output to Square on a gradient
    // image. This tests that Jacobian correction doesn't break normal operation.
    use crate::math::DMat3;
    use crate::registration::TransformType;

    let w = 20;
    let h = 20;
    let pixels: Vec<f32> = (0..w * h)
        .map(|i| {
            let x = (i % w) as f32;
            let y = (i / w) as f32;
            1.0 + x * 0.1 + y * 0.05
        })
        .collect();

    // Small sub-pixel shift
    let matrix = DMat3::from_rows([1.0, 0.0, 0.25], [0.0, 1.0, 0.15], [0.0, 0.0, 1.0]);
    let transform = Transform::from_matrix(matrix, TransformType::Affine);

    // Square kernel as reference
    let config_sq = DrizzleConfig {
        scale: 1.0,
        pixfrac: 1.0,
        kernel: DrizzleKernel::Square,
        ..Default::default()
    };
    let image_sq = AstroImage::from_pixels(ImageDimensions::new(w, h, 1), pixels.clone());
    let mut acc_sq = DrizzleAccumulator::new(ImageDimensions::new(w, h, 1), config_sq);
    acc_sq.add_image(image_sq, &transform, 1.0, None);
    let result_sq = acc_sq.finalize();
    let out_sq = result_sq.image.channel(0);

    for kernel in [
        DrizzleKernel::Turbo,
        DrizzleKernel::Point,
        DrizzleKernel::Gaussian,
        DrizzleKernel::Lanczos,
    ] {
        let config = DrizzleConfig {
            scale: 1.0,
            pixfrac: 1.0,
            kernel,
            ..Default::default()
        };
        let image = AstroImage::from_pixels(ImageDimensions::new(w, h, 1), pixels.clone());
        let mut acc = DrizzleAccumulator::new(ImageDimensions::new(w, h, 1), config);
        acc.add_image(image, &transform, 1.0, None);
        let result = acc.finalize();
        let out = result.image.channel(0);
        let ow = result.image.width();

        // Check interior pixels, allowing different interpolation effects per kernel
        let mut max_diff = 0.0f32;
        let mut checked = 0;
        for y in 5..15 {
            for x in 5..15 {
                let idx = y * ow + x;
                let vs = out_sq[idx];
                let vk = out[idx];
                if vs > 0.0 && vk > 0.0 {
                    let diff = (vs - vk).abs();
                    max_diff = max_diff.max(diff);
                    checked += 1;
                }
            }
        }
        assert!(
            checked > 50,
            "{kernel:?}: not enough covered pixels ({checked})"
        );
        // Different kernels interpolate differently, but on a smooth gradient with
        // sub-pixel shift, they should all be within ~0.15 of Square kernel.
        // The important thing is Jacobian doesn't introduce large systematic errors.
        assert!(
            max_diff < 0.2,
            "{kernel:?} vs Square: max_diff={max_diff:.4}, expected < 0.2"
        );
    }
}
