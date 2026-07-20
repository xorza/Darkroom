use crate::stacking::drizzle::tests::*;

#[test]
fn test_drizzle_single_image() {
    // Create a simple test image
    let image = constant_mono_image(100, 100, 0.5);

    let config = DrizzleConfig::x2();
    let mut acc = accumulator(ImageDimensions::new((100, 100), 1), config);

    let identity = Transform::identity();
    acc.add_image(image, &identity, 1.0, None);

    let result = acc.finalize();

    // Output should be 200x200
    assert_eq!(result.image.width(), 200);
    assert_eq!(result.image.height(), 200);

    // With scale=2, pixfrac=0.8: drop_size = 0.8*2 = 1.6 output pixels.
    // Integer-center: input pixel (ix,iy) center at (ix,iy), scaled to (2*ix, 2*iy).
    // A single flat image is reproduced wherever there is coverage: value = val·w / w = val.
    // Only the thin high-edge band (coverage < min_coverage) falls back to fill_value.
    let pixels = result.image.channel(0);
    assert!(
        pixels
            .iter()
            .all(|&p| p.abs() < 1e-5 || (p - 0.5).abs() < 1e-5),
        "every pixel must be fill_value or the input value 0.5"
    );
    let covered = pixels.iter().filter(|&&p| (p - 0.5).abs() < 1e-5).count();
    assert!(
        covered as f32 / pixels.len() as f32 > 0.97,
        "interior should be fully covered (only edges fill): {covered}/{}",
        pixels.len()
    );
    // Input pixel (0,0)'s drop is centered on output (0,0) → covered, reads the input value.
    assert!(
        (pixels[0] - 0.5).abs() < 1e-5,
        "Pixel (0,0) should be 0.5, got {}",
        pixels[0]
    );
}

#[test]
fn test_drizzle_point_kernel() {
    let image = constant_mono_image(10, 10, 1.0);

    let config = DrizzleConfig::x2().with_kernel(DrizzleKernel::Point);
    let mut acc = accumulator(ImageDimensions::new((10, 10), 1), config);

    let identity = Transform::identity();
    acc.add_image(image, &identity, 1.0, None);

    let result = acc.finalize();
    assert_eq!(result.image.width(), 20);
    assert_eq!(result.image.height(), 20);

    // Point kernel (integer-center): input (ix,iy) center at (ix,iy),
    // scaled → output round(ix*2) = 2*ix, round(iy*2) = 2*iy (even coords).
    // Covered pixels (even x, even y): value = 1.0/1.0 = 1.0; others fill_value = 0.0.
    let pixels = result.image.channel(0);
    let w = 20;
    // (0,0) ← input (0,0): value = 1.0
    assert!((pixels[0] - 1.0).abs() < f32::EPSILON);
    // (2,0) ← input (1,0): value = 1.0
    assert!((pixels[2] - 1.0).abs() < f32::EPSILON);
    // (1,1): odd coords, no coverage → 0.0
    assert!((pixels[w + 1]).abs() < f32::EPSILON);
    // (3,3): odd coords, no coverage → 0.0
    assert!((pixels[3 * w + 3]).abs() < f32::EPSILON);
    // Exactly 100 covered pixels (10×10 input maps to 10×10 even-coordinate outputs)
    let covered = pixels.iter().filter(|&&v| v > 0.5).count();
    assert_eq!(covered, 100);
}

#[test]
fn test_drizzle_stack_empty_paths() {
    let config = DrizzleConfig::default();

    let result = drizzle_stack(
        Vec::<DrizzleFrame<std::path::PathBuf>>::new(),
        &config,
        ProgressCallback::default(),
    );
    assert!(matches!(result.unwrap_err(), DrizzleError::NoFrames));
}

#[test]
fn test_drizzle_images_empty() {
    let result = drizzle_images(
        Vec::new(),
        &DrizzleConfig::default(),
        ProgressCallback::default(),
    );
    assert!(matches!(result.unwrap_err(), DrizzleError::NoFrames));
}

#[test]
fn test_drizzle_images_matches_accumulator() {
    // drizzle_images with one identity-transformed frame must reproduce the
    // single-image accumulator path: 200x200 output, interior pixels = 0.5.
    let image = constant_mono_image(100, 100, 0.5);
    let result = drizzle_images(
        vec![DrizzleFrame::new(image, Transform::identity())],
        &DrizzleConfig::x2(),
        ProgressCallback::default(),
    )
    .unwrap();

    assert_eq!(result.image.width(), 200);
    assert_eq!(result.image.height(), 200);
    let pixels = result.image.channel(0);
    assert!(
        (pixels[0] - 0.5).abs() < 1e-5,
        "Pixel (0,0) should be 0.5, got {}",
        pixels[0]
    );
}

#[test]
fn test_drizzle_images_dimension_mismatch() {
    let a = constant_mono_image(20, 20, 0.5);
    let b = constant_mono_image(10, 10, 0.5);
    let result = drizzle_images(
        drizzle_frames(vec![a, b], &[Transform::identity(), Transform::identity()]),
        &DrizzleConfig::default(),
        ProgressCallback::default(),
    );
    assert!(matches!(
        result.unwrap_err(),
        DrizzleError::DimensionMismatch { index: 1, .. }
    ));
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
    let image = AstroImage::from_pixels(ImageDimensions::new((50, 50), 3), pixels);

    let config = DrizzleConfig::x2();
    let mut acc = accumulator(ImageDimensions::new((50, 50), 3), config);

    let identity = Transform::identity();
    acc.add_image(image, &identity, 1.0, None);

    let result = acc.finalize();

    assert_eq!(result.image.width(), 100);
    assert_eq!(result.image.height(), 100);
    assert_eq!(result.image.channels(), 3);
    assert_eq!(result.weight.dimensions(), result.image.dimensions());
    let linear_variance = result.linear_variance.as_ref().unwrap();
    assert_eq!(linear_variance.dimensions(), result.image.dimensions());
    for channel in 1..3 {
        assert_eq!(
            result.weight.channel(channel).pixels(),
            result.weight.channel(0).pixels()
        );
        assert_eq!(
            linear_variance.channel(channel).pixels(),
            linear_variance.channel(0).pixels()
        );
    }
}

#[test]
fn test_drizzle_with_translation() {
    // Single bright pixel at (10,10), all others zero
    let mut pixels = vec![0.0f32; 20 * 20];
    pixels[10 * 20 + 10] = 1.0;
    let image = mono_image(20, 20, pixels);

    // scale=2, pixfrac=0.8: drop_size = 0.8*2 = 1.6, half_drop = 0.8
    let config = DrizzleConfig::x2();
    let mut acc = accumulator(ImageDimensions::new((20, 20), 1), config);

    // Integer-center: input pixel (10,10) center (10,10), +translation (0.5,0.5) → (10.5,10.5),
    // ×scale 2 → output center (21,21). drop_size 1.6 → drop [20.2,21.8]², covering cells
    // 20,21,22 with per-axis overlaps 0.3/1.0/0.3.
    let transform = Transform::translation(DVec2::new(0.5, 0.5));
    acc.add_image(image, &transform, 1.0, None);

    let result = acc.finalize();
    assert_eq!(result.image.width(), 40);
    assert_eq!(result.image.height(), 40);

    let out = result.image.channel(0);
    let at = |x: usize, y: usize| out[y * 40 + x];
    // Center cell (21,21): only the bright pixel's drop reaches it (1.0×1.0 overlap) → 1.0.
    assert!(
        (at(21, 21) - 1.0).abs() < 1e-5,
        "center (21,21): {}",
        at(21, 21)
    );
    // Edge cells are shared 50/50 with a neighbouring zero-valued input pixel's drop →
    // weighted mean (1.0·w + 0.0·w) / 2w = 0.5.
    assert!(
        (at(20, 21) - 0.5).abs() < 1e-5,
        "edge (20,21): {}",
        at(20, 21)
    );
    assert!(
        (at(22, 21) - 0.5).abs() < 1e-5,
        "edge (22,21): {}",
        at(22, 21)
    );
    assert!(
        (at(21, 20) - 0.5).abs() < 1e-5,
        "edge (21,20): {}",
        at(21, 20)
    );
    assert!(
        (at(21, 22) - 0.5).abs() < 1e-5,
        "edge (21,22): {}",
        at(21, 22)
    );
    // Far from the bright spot → 0; no pixel exceeds the input value.
    assert!(at(0, 0).abs() < 1e-5);
    assert!(
        out.iter().all(|&v| v <= 1.0 + 1e-5),
        "no pixel exceeds input max"
    );
}

#[test]
fn test_coverage_map() {
    // Point kernel with identity: covered at even coords, uncovered at odd
    let image = constant_mono_image(4, 4, 1.0);
    let config = DrizzleConfig::x2().with_kernel(DrizzleKernel::Point);
    let mut acc = accumulator(ImageDimensions::new((4, 4), 1), config);
    acc.add_image(image, &Transform::identity(), 1.0, None);
    let result = acc.finalize();

    // Output 8×8. Covered pixels at (2*ix, 2*iy) for ix,iy=0..3 (even coords).
    // Normalized coverage: max_coverage = 1.0
    assert!((result.coverage[(0, 0)] - 1.0).abs() < f32::EPSILON); // covered
    assert!((result.coverage[(1, 1)]).abs() < f32::EPSILON); // uncovered (odd)
    assert!((result.coverage[(2, 2)] - 1.0).abs() < f32::EPSILON); // covered
    assert!((result.coverage[(3, 3)]).abs() < f32::EPSILON); // uncovered (odd)
}

#[test]
fn test_weight_and_linear_variance_maps() {
    // scale=1, pixfrac=1, Turbo, identity: each input pixel maps 1:1 onto its output pixel with
    // overlap=1 and Jacobian=1, so every contribution has weight = frame_weight exactly.
    let config = DrizzleConfig {
        scale: 1.0,
        pixfrac: 1.0,
        ..Default::default()
    };
    let dims = ImageDimensions::new((4, 4), 1);
    let idx = 2 * 4 + 2; // interior output pixel (2, 2)

    // (a) 3 equal-weight frames → Σw = 3, Σw² = 3, variance = 3/3² = 1/3 — the noise of an N=3
    // average. The image RMS of these identical frames is 0, while the linear factor correctly
    // reports that equal unit input variance would become 1/3.
    let mut acc = accumulator(dims, config.clone());
    for _ in 0..3 {
        acc.add_image(
            AstroImage::from_pixels(dims, vec![5.0; 16]),
            &Transform::identity(),
            1.0,
            None,
        );
    }
    let equal = acc.finalize();
    let equal_linear_variance = equal.linear_variance.as_ref().unwrap();
    assert!(
        (equal.weight.channel(0).pixels()[idx] - 3.0).abs() < 1e-5,
        "Σw should be 3, got {}",
        equal.weight.channel(0).pixels()[idx]
    );
    assert!(
        (equal_linear_variance.channel(0).pixels()[idx] - 1.0 / 3.0).abs() < 1e-5,
        "linear variance factor should be 1/3, got {}",
        equal_linear_variance.channel(0).pixels()[idx]
    );
    assert!((equal.image.channel(0).pixels()[idx] - 5.0).abs() < 1e-5);

    // (b) 2 frames with frame weights [1, 3] → Σw = 4, Σw² = 1 + 9 = 10, variance = 10/16 = 0.625.
    let mut acc = accumulator(dims, config);
    acc.add_image(
        AstroImage::from_pixels(dims, vec![10.0; 16]),
        &Transform::identity(),
        1.0,
        None,
    );
    acc.add_image(
        AstroImage::from_pixels(dims, vec![10.0; 16]),
        &Transform::identity(),
        3.0,
        None,
    );
    let unequal = acc.finalize();
    let unequal_linear_variance = unequal.linear_variance.as_ref().unwrap();
    assert!(
        (unequal.weight.channel(0).pixels()[idx] - 4.0).abs() < 1e-5,
        "Σw should be 4, got {}",
        unequal.weight.channel(0).pixels()[idx]
    );
    assert!(
        (unequal_linear_variance.channel(0).pixels()[idx] - 0.625).abs() < 1e-5,
        "linear variance factor should be 0.625, got {}",
        unequal_linear_variance.channel(0).pixels()[idx]
    );

    // Concentrating weight on fewer frames raises variance above the equal-weight 2-frame optimum
    // (1/2) — the map responds to the weight distribution, not just the contribution count.
    assert!(
        unequal_linear_variance.channel(0).pixels()[idx] > 0.5,
        "unequal weighting should raise variance above 1/2"
    );
}
