use crate::stacking::drizzle::tests::*;

/// Test turbo kernel drop size and overlap with hand-computed values.
///
/// Setup: 4×4 input, scale=2, pixfrac=0.8 → drop_size = 1.6, half_drop = 0.8.
/// Integer-center: input pixel (1,1) center (1,1), ×scale 2 → output center (2,2).
/// Output: 8×8. Drop covers [1.2, 2.8]² → cells 1,2,3 with per-axis overlaps 0.3/1.0/0.3.
///
/// The center cell (2,2) is fully inside the drop (1.0×1.0) and no neighbouring input
/// pixel's drop reaches it, so it reads the undiluted bright value. Edge cells (e.g. (1,2))
/// are shared 50/50 with a zero-valued neighbour's drop, so a lone bright pixel reads half.
#[test]
fn test_turbo_kernel_overlap_exact() {
    // Single bright pixel at (1,1) with value 2.0
    let mut pixels = vec![0.0f32; 4 * 4];
    pixels[5] = 2.0; // pixel (1,1) = index 1*4+1 = 5
    let image = mono_image(4, 4, pixels);

    let config = DrizzleConfig::x2(); // scale=2, pixfrac=0.8
    let mut acc = accumulator(ImageDimensions::new((4, 4), 1), config);
    acc.add_image(image, &Transform::identity(), 1.0, None);

    let result = acc.finalize();
    let out = result.image.channel(0);
    let w = 8usize;

    let at = |x: usize, y: usize| out[y * w + x];
    // Center cell (2,2): only input (1,1) reaches it → undiluted bright value 2.0.
    assert!((at(2, 2) - 2.0).abs() < 1e-5, "center (2,2): {}", at(2, 2));
    // Edge cells: shared 50/50 with a zero-valued neighbour's drop → 2.0 / 2 = 1.0.
    assert!((at(1, 2) - 1.0).abs() < 1e-5, "edge (1,2): {}", at(1, 2));
    assert!((at(3, 2) - 1.0).abs() < 1e-5, "edge (3,2): {}", at(3, 2));
    assert!((at(2, 1) - 1.0).abs() < 1e-5, "edge (2,1): {}", at(2, 1));
    assert!((at(2, 3) - 1.0).abs() < 1e-5, "edge (2,3): {}", at(2, 3));
    // No pixel exceeds the bright value.
    assert!(out.iter().all(|&v| v <= 2.0 + 1e-5), "no pixel exceeds 2.0");
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
    let image = constant_mono_image(4, 4, 1.0);

    let config = DrizzleConfig::x2().with_pixfrac(1.0); // drop_size = 2.0
    let mut acc = accumulator(ImageDimensions::new((4, 4), 1), config);
    let transform = Transform::translation(DVec2::new(0.25, 0.0));
    acc.add_image(image, &transform, 1.0, None);

    let result = acc.finalize();
    let out = result.image.channel(0);
    let w = 8usize;

    // A flux-preserving turbo kernel reproduces the uniform input value at every covered
    // output pixel, regardless of the sub-pixel shift phase.
    assert!(
        out.iter()
            .all(|&v| v.abs() < 1e-5 || (v - 1.0).abs() < 1e-5),
        "every covered pixel reads the uniform value 1.0"
    );
    // Interior is covered (pixfrac=1 tiles the output) → input value.
    assert!(
        (out[2 * w + 3] - 1.0).abs() < 1e-5,
        "interior pixel should be 1.0, got {}",
        out[2 * w + 3]
    );
}

/// Test that min_coverage works with normalized weights.
///
/// Single bright pixel (0,0), pixfrac=0.5 (drop width 1.0), scale=2, sub-pixel shift (0.1, 0).
/// The drop centers at output (0.2, 0) and splits unevenly between cells (0,0) and (1,0):
/// x-overlaps 0.8 and 0.2. max_weight = 0.8; threshold = min_coverage(0.6) * 0.8 = 0.48.
/// cell (0,0): weight 0.8 ≥ 0.48 → kept (1.0). cell (1,0): weight 0.2 < 0.48 → fill_value.
#[test]
fn test_min_coverage_normalized() {
    let mut pixels = vec![0.0f32; 4 * 4];
    pixels[0] = 1.0;
    let image = mono_image(4, 4, pixels);

    let config = DrizzleConfig::x2().with_pixfrac(0.5).with_min_coverage(0.6);
    let mut acc = accumulator(ImageDimensions::new((4, 4), 1), config);
    let transform = Transform::translation(DVec2::new(0.1, 0.0));
    acc.add_image(image, &transform, 1.0, None);
    let result = acc.finalize();
    let out = result.image.channel(0);

    assert!((out[0] - 1.0).abs() < 1e-5, "cell (0,0) kept: {}", out[0]);
    assert!(
        out[1].abs() < 1e-5,
        "cell (1,0) rejected (below min_coverage): {}",
        out[1]
    );
}

/// Test Gaussian kernel produces flux-preserving smooth output.
///
/// A uniform 10×10 image with value 3.0 through Gaussian kernel should produce
/// approximately 3.0 everywhere in the interior (edges may differ due to truncation).
#[test]
fn test_gaussian_kernel_uniform_preserves_value() {
    let image = constant_mono_image(10, 10, 3.0);

    let config = DrizzleConfig::x2().with_kernel(DrizzleKernel::Gaussian);
    let mut acc = accumulator(ImageDimensions::new((10, 10), 1), config);
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
    let image = constant_mono_image(20, 20, 5.0);

    let config = DrizzleConfig {
        scale: 1.0,
        pixfrac: 1.0,
        kernel: DrizzleKernel::Lanczos,
        fill_value: 0.0,
        min_coverage: 0.0,
    };
    let mut acc = accumulator(ImageDimensions::new((20, 20), 1), config);
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
    let image = mono_image(20, 20, pixels);

    let config = DrizzleConfig {
        scale: 1.0,
        pixfrac: 1.0,
        kernel: DrizzleKernel::Lanczos,
        fill_value: 0.0,
        min_coverage: 0.0,
    };
    let mut acc = accumulator(ImageDimensions::new((20, 20), 1), config);
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
    let image1 = constant_mono_image(10, 10, 2.0);
    let image2 = constant_mono_image(10, 10, 6.0);

    let config = DrizzleConfig::x2();
    let mut acc = accumulator(ImageDimensions::new((10, 10), 1), config);
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
    let image1 = mono_image(6, 6, pixels1);

    let config1 = DrizzleConfig::x2().with_pixfrac(1.0);
    let mut acc1 = accumulator(ImageDimensions::new((6, 6), 1), config1);
    acc1.add_image(image1, &transform, 1.0, None);
    let r1 = acc1.finalize();
    let out1 = r1.image.channel(0);
    let covered_1 = out1.iter().filter(|&&v| v > 0.01).count();

    // pixfrac=0.3: drop_size=0.6, half=0.3
    // Drop (4.9,4.9)→(5.5,5.5): overlaps (4,4),(5,4),(4,5),(5,5) = 4 pixels
    let mut pixels2 = vec![0.0f32; 6 * 6];
    pixels2[2 * 6 + 2] = 1.0;
    let image2 = mono_image(6, 6, pixels2);

    let config2 = DrizzleConfig::x2().with_pixfrac(0.3);
    let mut acc2 = accumulator(ImageDimensions::new((6, 6), 1), config2);
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
    let image = LinearImage::from_pixels(ImageDimensions::new((4, 4), 3), pixels);

    let config = DrizzleConfig::x2();
    let mut acc = accumulator(ImageDimensions::new((4, 4), 3), config);
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
    let image = mono_image(5, 5, pixels.clone());

    let config = DrizzleConfig {
        scale: 1.0,
        pixfrac: 1.0,
        kernel: DrizzleKernel::Turbo,
        fill_value: 0.0,
        min_coverage: 0.0,
    };
    let mut acc = accumulator(ImageDimensions::new((5, 5), 1), config);
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
    let image = constant_mono_image(4, 4, 1.0);

    let config = DrizzleConfig {
        scale: 2.0,
        pixfrac: 0.8,
        kernel: DrizzleKernel::Point,
        fill_value: -999.0,
        min_coverage: 0.0,
    };
    let mut acc = accumulator(ImageDimensions::new((4, 4), 1), config);
    acc.add_image(image, &Transform::identity(), 1.0, None);
    let result = acc.finalize();

    let out = result.image.channel(0);
    let w = 8usize;

    // Point kernel (integer-center): input (ix,iy) → output (2*ix, 2*iy) (even coords).
    // Covered pixel (0,0): value = 1.0
    assert!(
        (out[0] - 1.0).abs() < f32::EPSILON,
        "Covered pixel (0,0) should be 1.0, got {}",
        out[0]
    );
    // Uncovered pixel (1,1) (odd coords): fill_value = -999.0
    assert!(
        (out[w + 1] - (-999.0)).abs() < f32::EPSILON,
        "Uncovered pixel (1,1) should be -999.0, got {}",
        out[w + 1]
    );
    // Covered pixel (2,2) ← input (1,1): value = 1.0
    assert!(
        (out[2 * w + 2] - 1.0).abs() < f32::EPSILON,
        "Covered pixel (2,2) should be 1.0, got {}",
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
    let image1 = constant_mono_image(8, 8, 3.0);
    let image2 = constant_mono_image(8, 8, 100.0);

    let config = DrizzleConfig::x2();
    let mut acc = accumulator(ImageDimensions::new((8, 8), 1), config);
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
    let image = constant_mono_image(12, 12, 4.0);

    let config = DrizzleConfig::x2().with_kernel(DrizzleKernel::Gaussian);
    let mut acc = accumulator(ImageDimensions::new((12, 12), 1), config);
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
    let image = constant_mono_image(20, 20, 7.0);

    let config = DrizzleConfig {
        scale: 1.0,
        pixfrac: 1.0,
        kernel: DrizzleKernel::Lanczos,
        fill_value: 0.0,
        min_coverage: 0.0,
    };
    let mut acc = accumulator(ImageDimensions::new((20, 20), 1), config);
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
    let image = mono_image(4, 4, pixels);

    let mut pw = Buffer2::new_filled(4, 4, 1.0f32);
    *pw.get_mut(1, 1) = 0.0; // Exclude (1,1)

    let config = DrizzleConfig {
        scale: 1.0,
        pixfrac: 1.0,
        kernel: DrizzleKernel::Turbo,
        fill_value: 0.0,
        min_coverage: 0.0,
    };
    let mut acc = accumulator(ImageDimensions::new((4, 4), 1), config);
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
    let image1 = constant_mono_image(4, 4, 2.0);
    let image2 = constant_mono_image(4, 4, 6.0);

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
    let mut acc = accumulator(ImageDimensions::new((4, 4), 1), config);
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
    let image = constant_mono_image(8, 8, 5.0);

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
    let mut acc = accumulator(ImageDimensions::new((8, 8), 1), config);
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
/// scale=2 (integer-center): input (ix,iy) → output (2*ix, 2*iy). Point kernel = 1 pixel.
/// Pixel (1,1) = 10.0, weight = 0.0. Should not appear at output (2,2).
/// Pixel (0,0) = 3.0, weight = 1.0. Should appear at output (0,0) = 3.0.
#[test]
fn test_pixel_weight_with_point_kernel() {
    let mut pixels = vec![1.0f32; 4 * 4];
    pixels[5] = 10.0; // (1,1) bad pixel
    pixels[0] = 3.0;
    let image = mono_image(4, 4, pixels);

    let mut pw = Buffer2::new_filled(4, 4, 1.0f32);
    *pw.get_mut(1, 1) = 0.0;

    let config = DrizzleConfig::x2().with_kernel(DrizzleKernel::Point);
    let mut acc = accumulator(ImageDimensions::new((4, 4), 1), config);
    acc.add_image(image, &Transform::identity(), 1.0, Some(&pw));
    let result = acc.finalize();
    let out = result.image.channel(0);
    let w = 8usize;

    // Output (2,2) ← input (1,1): excluded by weight=0 → fill_value 0.0
    assert!(
        out[2 * w + 2].abs() < 1e-5,
        "Excluded pixel output (2,2) should be 0.0, got {}",
        out[2 * w + 2]
    );
    // Output (0,0) ← input (0,0): weight=1.0, value=3.0
    assert!(
        (out[0] - 3.0).abs() < 1e-5,
        "Good pixel output (0,0) should be 3.0, got {}",
        out[0]
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
    let image = constant_mono_image(12, 12, 4.0);

    let mut pw = Buffer2::new_filled(12, 12, 1.0f32);
    *pw.get_mut(5, 5) = 0.0;

    let config = DrizzleConfig::x2().with_kernel(DrizzleKernel::Gaussian);
    let mut acc = accumulator(ImageDimensions::new((12, 12), 1), config);
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
