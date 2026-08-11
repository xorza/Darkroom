use super::*;

#[test]
fn drizzle_single_image() {
    // Create a simple test image
    let image = constant_mono_image(Size2us::new(100, 100), 0.5);

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
    // Only the thin high-edge band (coverage < min_weight_fraction) falls back to fill_value.
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
fn drizzle_point_kernel() {
    let image = constant_mono_image(Size2us::new(10, 10), 1.0);

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
    assert_eq!(pixels[0], 1.0);
    // (2,0) ← input (1,0): value = 1.0
    assert_eq!(pixels[2], 1.0);
    // (1,1): odd coords, no coverage → 0.0
    assert!((pixels[w + 1]).abs() < f32::EPSILON);
    // (3,3): odd coords, no coverage → 0.0
    assert!((pixels[3 * w + 3]).abs() < f32::EPSILON);
    // Exactly 100 covered pixels (10×10 input maps to 10×10 even-coordinate outputs)
    let covered = pixels.iter().filter(|&&v| v > 0.5).count();
    assert_eq!(covered, 100);
}

#[test]
fn drizzle_stack_empty_paths() {
    let config = DrizzleConfig::default();

    let result = drizzle_stack(
        Vec::<DrizzleFrame<std::path::PathBuf>>::new(),
        &config,
        &LoadContext::default(),
        ProgressCallback::default(),
        CancelToken::never(),
    );
    assert!(matches!(result.unwrap_err(), DrizzleError::NoFrames));
}

#[test]
fn drizzle_images_empty() {
    let result = drizzle_images(
        Vec::new(),
        &DrizzleConfig::default(),
        ProgressCallback::default(),
        CancelToken::never(),
    );
    assert!(matches!(result.unwrap_err(), DrizzleError::NoFrames));
}

#[test]
fn drizzle_stops_between_frames_when_cancelled() {
    // Cancellation is checked between frames, so a run already cancelled distributes the frame
    // the accumulator was sized from and then stops rather than walking the rest of the set.
    let cancel = CancelToken::new();
    cancel.cancel();
    let frames: Vec<_> = (0..3)
        .map(|_| {
            DrizzleFrame::new(
                constant_mono_image(Size2us::new(16, 16), 0.5),
                Transform::identity(),
            )
        })
        .collect();

    let result = drizzle_images(
        frames,
        &DrizzleConfig::default(),
        ProgressCallback::default(),
        cancel,
    );
    assert!(
        matches!(result.unwrap_err(), DrizzleError::Cancelled),
        "a cancelled drizzle must report cancellation, not a partial product"
    );

    // The same set completes when the run is live, so the guard is what stopped it.
    let frames: Vec<_> = (0..3)
        .map(|_| {
            DrizzleFrame::new(
                constant_mono_image(Size2us::new(16, 16), 0.5),
                Transform::identity(),
            )
        })
        .collect();
    assert!(
        drizzle_images(
            frames,
            &DrizzleConfig::default(),
            ProgressCallback::default(),
            CancelToken::never(),
        )
        .is_ok()
    );
}

#[test]
fn drizzle_images_matches_accumulator() {
    // drizzle_images with one identity-transformed frame must reproduce the
    // single-image accumulator path: 200x200 output, interior pixels = 0.5.
    let image = constant_mono_image(Size2us::new(100, 100), 0.5);
    let result = drizzle_images(
        vec![DrizzleFrame::new(image, Transform::identity())],
        &DrizzleConfig::x2(),
        ProgressCallback::default(),
        CancelToken::never(),
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
fn drizzle_images_dimension_mismatch() {
    let a = constant_mono_image(Size2us::new(20, 20), 0.5);
    let b = constant_mono_image(Size2us::new(10, 10), 0.5);
    let result = drizzle_images(
        drizzle_frames(vec![a, b], &[Transform::identity(), Transform::identity()]),
        &DrizzleConfig::default(),
        ProgressCallback::default(),
        CancelToken::never(),
    );
    assert!(matches!(
        result.unwrap_err(),
        DrizzleError::DimensionMismatch(FrameDimensionMismatch { index: 1, .. })
    ));
}

#[test]
fn drizzle_rgb_uses_shared_quality_planes() {
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
    let image = LinearImage::from_pixels(ImageDimensions::new((50, 50), 3), pixels);

    let config = DrizzleConfig::x2();
    let mut acc = accumulator(ImageDimensions::new((50, 50), 3), config);

    let identity = Transform::identity();
    acc.add_image(image, &identity, 1.0, None);

    let result = acc.finalize();

    assert_eq!(result.image.width(), 100);
    assert_eq!(result.image.height(), 100);
    assert_eq!(result.image.channels(), 3);
    let Some(QualityMap::Shared(weight)) = &result.weight else {
        panic!("drizzle weight must be channel-independent");
    };
    let QualityMap::Shared(linear_variance) = result.linear_variance.as_ref().unwrap() else {
        panic!("drizzle linear variance must be channel-independent");
    };
    assert_eq!((weight.width(), weight.height()), (100, 100));
    assert_eq!(
        (linear_variance.width(), linear_variance.height()),
        (100, 100)
    );
    assert!(std::ptr::eq(
        result.weight.as_ref().unwrap().channel(0),
        result.weight.as_ref().unwrap().channel(2)
    ));
}

#[test]
fn drizzle_with_translation() {
    // Single bright pixel at (10,10), all others zero
    let mut pixels = vec![0.0f32; 20 * 20];
    pixels[10 * 20 + 10] = 1.0;
    let image = mono_image(Size2us::new(20, 20), pixels);

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
fn coverage_map() {
    // Point kernel with identity: covered at even coords, uncovered at odd
    let image = constant_mono_image(Size2us::new(4, 4), 1.0);
    let config = DrizzleConfig::x2().with_kernel(DrizzleKernel::Point);
    let mut acc = accumulator(ImageDimensions::new((4, 4), 1), config);
    acc.add_image(image, &Transform::identity(), 1.0, None);
    let result = acc.finalize();

    // Output 8×8. Covered pixels at (2*ix, 2*iy) for ix,iy=0..3 (even coords).
    // Normalized coverage: max_coverage = 1.0
    assert!((result.coverage.as_ref().unwrap()[(0, 0)] - 1.0).abs() < f32::EPSILON); // covered
    assert!((result.coverage.as_ref().unwrap()[(1, 1)]).abs() < f32::EPSILON); // uncovered (odd)
    assert!((result.coverage.as_ref().unwrap()[(2, 2)] - 1.0).abs() < f32::EPSILON); // covered
    assert!((result.coverage.as_ref().unwrap()[(3, 3)]).abs() < f32::EPSILON); // uncovered (odd)
}

#[test]
fn weight_and_linear_variance_maps() {
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
            LinearImage::from_pixels(dims, vec![5.0; 16]),
            &Transform::identity(),
            1.0,
            None,
        );
    }
    let equal = acc.finalize();
    let equal_linear_variance = equal.linear_variance.as_ref().unwrap();
    assert!(
        (equal.weight.as_ref().unwrap().channel(0).pixels()[idx] - 3.0).abs() < 1e-5,
        "Σw should be 3, got {}",
        equal.weight.as_ref().unwrap().channel(0).pixels()[idx]
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
        LinearImage::from_pixels(dims, vec![10.0; 16]),
        &Transform::identity(),
        1.0,
        None,
    );
    acc.add_image(
        LinearImage::from_pixels(dims, vec![10.0; 16]),
        &Transform::identity(),
        3.0,
        None,
    );
    let unequal = acc.finalize();
    let unequal_linear_variance = unequal.linear_variance.as_ref().unwrap();
    assert!(
        (unequal.weight.as_ref().unwrap().channel(0).pixels()[idx] - 4.0).abs() < 1e-5,
        "Σw should be 4, got {}",
        unequal.weight.as_ref().unwrap().channel(0).pixels()[idx]
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

/// A declined plane is not produced, and declining it changes nothing about the image.
///
/// The weight map is not optional internally — the image is `Σfluxᵢwᵢ / Σwᵢ` and `min_weight_fraction`
/// gates fill against its maximum — so the risk this pins is that gating the *outputs* disturbs the
/// combine. Run with a non-zero `min_weight_fraction` and a transform that leaves the frame's edge thinly
/// covered, so the fill gate is actually exercised while coverage is declined.
#[test]
fn declined_quality_planes_are_absent_and_do_not_disturb_the_image() {
    let side = 24;
    let image = constant_mono_image(Size2us::new(side, side), 0.5);
    let transform = Transform::translation(DVec2::new(1.7, -2.3));
    let product = |quality| {
        let config = DrizzleConfig {
            min_weight_fraction: 0.5,
            quality,
            ..DrizzleConfig::x2()
        };
        drizzle_one(side, config, image.clone(), &transform, None)
    };

    let all = product(QualityPlanes::ALL);
    assert!(all.coverage.is_some() && all.weight.is_some() && all.linear_variance.is_some());

    let bare = product(QualityPlanes::IMAGE_ONLY);
    assert!(bare.coverage.is_none() && bare.weight.is_none() && bare.linear_variance.is_none());

    // Each is independent of the others, and `variance` is the one that also drops an accumulator.
    let coverage_only = product(QualityPlanes {
        coverage: true,
        weight: false,
        variance: false,
    });
    assert!(coverage_only.coverage.is_some());
    assert!(coverage_only.weight.is_none() && coverage_only.linear_variance.is_none());

    // The fill gate has to have fired, or the min_weight_fraction path is untested here.
    let filled = all
        .image
        .channel(0)
        .iter()
        .filter(|value| **value == 0.0)
        .count();
    assert!(
        filled > 0,
        "min_weight_fraction dropped no pixels, so nothing was gated"
    );

    for (label, other) in [("image only", &bare), ("coverage only", &coverage_only)] {
        assert_eq!(
            other.image.channel(0).pixels(),
            all.image.channel(0).pixels(),
            "{label}: declining planes changed the combined image"
        );
    }
    assert_eq!(
        all.coverage.as_ref().unwrap().per_pixel().unwrap().pixels(),
        coverage_only
            .coverage
            .as_ref()
            .unwrap()
            .per_pixel()
            .unwrap()
            .pixels(),
        "coverage differed when the other planes were declined"
    );
}

/// Drizzle reports coverage as the share of *frames* that reached a pixel — the same quantity the
/// statistical combine reports, so a `StackProduct` means one thing whichever produced it.
///
/// The fixture separates that from the accumulated weight it used to be normalized against: two
/// frames overlapping on part of the grid, the second carrying three times the frame weight of the
/// first. In the band only the first frame reaches, one of two frames contributed — coverage 0.5 —
/// while the weight there is a quarter of the deepest pixel's. The old `weight / max_weight` read
/// 0.25 for that band.
#[test]
fn coverage_counts_frames_rather_than_accumulated_weight() {
    let side = 12;
    let overlap_from = 4;
    let config = DrizzleConfig {
        scale: 1.0,
        pixfrac: 1.0,
        kernel: DrizzleKernel::Turbo,
        min_weight_fraction: 0.0,
        ..Default::default()
    };
    let mut acc = accumulator(ImageDimensions::new((side, side), 1), config);
    acc.add_image(
        constant_mono_image(Size2us::new(side, side), 1.0),
        &Transform::identity(),
        1.0,
        None,
    );
    acc.add_image(
        constant_mono_image(Size2us::new(side, side), 1.0),
        &Transform::translation(DVec2::new(overlap_from as f64, 0.0)),
        3.0,
        None,
    );
    let product = acc.finalize();
    let coverage = product.coverage.as_ref().expect("coverage was requested");
    let weight = product
        .weight
        .as_ref()
        .expect("weight was requested")
        .channel(0);

    let max_weight = weight.pixels().iter().copied().fold(0.0f32, f32::max);
    assert_eq!(max_weight, 4.0, "frame weights 1 + 3 over the overlap");

    for y in 0..side {
        for x in 0..side {
            let (frames, expected_weight) = if x < overlap_from {
                (1.0, 1.0)
            } else {
                (2.0, 4.0)
            };
            assert_eq!(weight[(x, y)], expected_weight, "weight at ({x}, {y})");
            assert_eq!(
                coverage[(x, y)],
                frames / 2.0,
                "coverage at ({x}, {y}) must be the share of frames"
            );
        }
    }

    // The two measures genuinely disagree here, which is what makes the assertions above a test of
    // the normalization rather than of a fixture where both answer alike.
    assert_eq!(coverage[(0, 0)], 0.5);
    assert_eq!(weight[(0, 0)] / max_weight, 0.25);
}

/// One output band and many must produce the identical result, bit for bit.
///
/// Parallelizing a float accumulation is only sound because each output pixel belongs to exactly one
/// band and a band walks its inputs in the order the serial loop did, so every pixel's contributions
/// are summed in the same sequence whatever the band count. Run over transforms whose bands need
/// *different* input rows — a rotation makes a band's input strip diagonal, so a row estimate that
/// was even one row too tight would drop flux and show up here as a mismatch.
#[test]
fn band_count_does_not_change_the_result() {
    let image = star_field(Size2us::new(64, 64), 24, 4242).image;
    let dimensions = image.dimensions();

    let transforms = [
        ("translation", Transform::translation(DVec2::new(3.7, -2.4))),
        (
            "rotation",
            Transform::euclidean(DVec2::new(5.0, -3.0), 0.05),
        ),
        (
            "similarity",
            Transform::similarity(DVec2::new(2.0, 1.0), -0.03, 1.02),
        ),
        (
            "homography",
            Transform::homography([1.0, 0.002, 4.0, -0.001, 1.0, -2.0, 2e-5, 1e-5]),
        ),
    ];
    let kernels = [
        DrizzleKernel::Square,
        DrizzleKernel::Turbo,
        DrizzleKernel::Point,
        DrizzleKernel::Gaussian,
        DrizzleKernel::Lanczos,
    ];

    for (name, transform) in transforms {
        for kernel in kernels {
            // Lanczos is only valid at scale 1 / pixfrac 1, which its config validation enforces.
            let (scale, pixfrac) = match kernel {
                DrizzleKernel::Lanczos => (1.0, 1.0),
                _ => (2.0, 0.8),
            };
            let drizzle = |band_rows: usize| {
                let config = DrizzleConfig {
                    scale,
                    pixfrac,
                    kernel,
                    quality: QualityPlanes::ALL,
                    ..DrizzleConfig::default()
                };
                let mut accumulator = accumulator(dimensions, config);
                add_image_with_band_rows(&mut accumulator, image.clone(), &transform, band_rows);
                accumulator.finalize()
            };

            // One band is the serial walk; 5 rows over a 64- or 128-row output is a dozen or more of
            // them, so most drops land inside a band and some straddle a boundary.
            let single = drizzle(dimensions.height() * 2);
            let many = drizzle(5);

            let case = format!("{name}/{kernel:?}");
            for channel in 0..dimensions.channels() {
                assert_eq!(
                    single.image.channel(channel),
                    many.image.channel(channel),
                    "{case}: image channel {channel}"
                );
            }
            assert_eq!(
                single.coverage.as_ref().map(Coverage::to_plane),
                many.coverage.as_ref().map(Coverage::to_plane),
                "{case}: coverage"
            );
            for (label, single, many) in [
                ("weight", &single.weight, &many.weight),
                ("variance", &single.linear_variance, &many.linear_variance),
            ] {
                let plane = |map: &Option<QualityMap>| {
                    map.as_ref().map(|map| map.channel(0).pixels().to_vec())
                };
                assert_eq!(plane(single), plane(many), "{case}: {label}");
            }
        }
    }
}
