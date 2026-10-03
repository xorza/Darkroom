use super::*;
use crate::stacking::registration::distortion::sip::{SipConfig, SipPolynomial};
use std::path::PathBuf;
use std::ptr;

#[test]
fn drizzle_single_image() {
    // Create a simple test image
    let image = constant_mono_image(Size2us::new(100, 100), 0.5);

    let config = DrizzleConfig::x2();
    let mut acc = accumulator(ImageDimensions::new((100, 100), 1), config);

    let identity = Transform::identity();
    acc.add_image(image, &identity, 1.0, None);

    let result = acc.finalize().product;

    // Output should be 200x200
    assert_eq!(result.image.width(), 200);
    assert_eq!(result.image.height(), 200);

    // With scale=2, pixfrac=0.8: drop_size = 0.8*2 = 1.6 output pixels, centred on (2·ix + ½,
    // 2·iy + ½). A single flat image is reproduced wherever there is coverage: value = val·w / w.
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
    // Input pixel (0,0)'s drop covers 0.8 of output (0,0) on each axis → reads the input value.
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

    let result = acc.finalize().product;
    assert_eq!(result.image.width(), 20);
    assert_eq!(result.image.height(), 20);

    // Point kernel: input (ix,iy) lands at (2·ix + ½, 2·iy + ½), the corner its 2×2 block of
    // output cells shares — a tie `f64::round` breaks upward, onto the odd cell (2·ix + 1, 2·iy + 1).
    // Covered pixels read 1.0/1.0 = 1.0; the rest take fill_value = 0.0.
    let pixels = result.image.channel(0);
    let w = 20;
    // (1,1) ← input (0,0), and (3,1) ← input (1,0).
    assert_eq!(pixels[w + 1], 1.0);
    assert_eq!(pixels[w + 3], 1.0);
    // Even coordinates: no drop lands there.
    assert_eq!(pixels[0], 0.0);
    assert_eq!(pixels[2 * w + 2], 0.0);
    // Exactly 100 covered pixels: 10×10 inputs onto 10×10 odd-coordinate outputs.
    let covered = pixels.iter().filter(|&&v| v > 0.5).count();
    assert_eq!(covered, 100);
}

#[test]
fn drizzle_stack_empty_paths() {
    let config = DrizzleConfig::default();

    let result = drizzle_stack(
        Vec::<DrizzleFrame<PathBuf>>::new(),
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
                WarpTransform::new(Transform::identity()),
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
                WarpTransform::new(Transform::identity()),
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

/// A SIP fit of `field` about the centre of a `size` frame, on `transform`, as registration would
/// report it.
fn sip_warp(size: Size2us, transform: Transform, field: impl Fn(DVec2) -> DVec2) -> WarpTransform {
    let center = DVec2::new(size.width as f64 / 2.0, size.height as f64 / 2.0);
    let mut reference = Vec::new();
    for y in (0..size.height).step_by(2) {
        for x in (0..size.width).step_by(2) {
            reference.push(DVec2::new(x as f64, y as f64));
        }
    }
    let target: Vec<DVec2> = reference
        .iter()
        .map(|&r| transform.apply(r + field(r - center)))
        .collect();
    let config = SipConfig {
        order: 3,
        reference_point: Some(center),
        clip_iterations: 0,
        ..SipConfig::default()
    };
    let fit = SipPolynomial::fit_from_transform(&reference, &target, &transform, &config).unwrap();
    WarpTransform::with_sip(transform, fit.polynomial)
}

/// A frame registered with SIP drizzles through the warp's inverse: `drizzle_images` and the
/// accumulator give the same planes bit for bit, every pixel converges, and a lone bright pixel at
/// input `t` lands on the output cell nearest `s·r + (s − 1)/2`, where `r` is the warp's inverse
/// of `t` — at scale 1 and with the point kernel, the cell `round(r)`.
#[test]
fn a_sip_warp_drizzles_through_its_inverse() {
    let size = Size2us::new(32, 32);
    let warp = sip_warp(
        size,
        Transform::similarity(DVec2::new(0.7, -0.4), 0.02, 1.0),
        |d| d * 2e-5 * d.length_squared(),
    );
    let config = kernel_config(DrizzleKernel::Square, 1.5, 0.7);
    let image = mono_image(
        size,
        (0..size.pixel_count())
            .map(|i| (i % 7) as f32 * 0.25)
            .collect(),
    );

    let from_images = drizzle_images(
        vec![DrizzleFrame::new(image.clone(), warp.clone())],
        &config,
        ProgressCallback::default(),
        CancelToken::never(),
    )
    .unwrap();
    let mut acc = accumulator(ImageDimensions::new(size, 1), config);
    acc.add_frame(DrizzleFrame::new(image, warp.clone()))
        .unwrap();
    let from_accumulator = acc.finalize();
    assert_eq!(from_images.unconverged_points, 0);
    assert_eq!(from_accumulator.unconverged_points, 0);
    let bits = |product: &StackProduct| -> Vec<u32> {
        product
            .image
            .channel(0)
            .iter()
            .map(|v| v.to_bits())
            .collect()
    };
    assert_eq!(bits(&from_images.product), bits(&from_accumulator.product));

    let mut pixels = vec![0.0f32; size.pixel_count()];
    let t = Vec2us::new(20, 9);
    pixels[size.index_of(t)] = 1.0;
    let point = kernel_config(DrizzleKernel::Point, 1.0, 1.0);
    let mut acc = accumulator(ImageDimensions::new(size, 1), point);
    acc.add_frame(DrizzleFrame::new(mono_image(size, pixels), warp.clone()))
        .unwrap();
    let out = acc.finalize().product;
    let r = warp
        .inverse()
        .apply(DVec2::new(t.x as f64, t.y as f64))
        .unwrap()
        .position;
    let landing = Vec2us::new(r.x.round() as usize, r.y.round() as usize);
    assert_eq!(
        out.image.channel(0)[(landing.x, landing.y)],
        1.0,
        "at {r:?}"
    );
}

/// Past a fold the warp has no inverse, and those input pixels deposit nothing and are counted —
/// once each, though the 4-row bands' scans overlap. `c(x) = −0.03·x²` about the centre folds the
/// field where `x + c(x)` peaks, `x = 1/(2·0.03)` ≈ 16.7 px right of it, at `t = 1/(4·0.03)` ≈ 8.3;
/// every input pixel further right than that has no preimage, and every one left of it has one.
#[test]
fn pixels_past_a_fold_are_counted_once_and_deposit_nothing() {
    let size = Size2us::new(40, 24);
    let warp = sip_warp(size, Transform::identity(), |d| {
        DVec2::new(-0.03 * d.x * d.x, 0.0)
    });
    let inverse = warp.inverse();
    let failing = (0..size.height)
        .flat_map(|y| (0..size.width).map(move |x| DVec2::new(x as f64, y as f64)))
        .filter(|&t| inverse.apply(t).is_none())
        .count();
    assert!(
        failing > 0 && failing < size.pixel_count(),
        "{failing} of the frame fail"
    );

    let mut acc = accumulator(
        ImageDimensions::new(size, 1),
        kernel_config(DrizzleKernel::Turbo, 1.0, 1.0),
    );
    acc.add_frame_with_band_rows(DrizzleFrame::new(constant_mono_image(size, 1.0), warp), 4);
    let result = acc.finalize();
    assert_eq!(result.unconverged_points, failing);
    // No position was guessed for a failing pixel: every covered output pixel holds the constant
    // exactly.
    assert!(
        result
            .product
            .image
            .channel(0)
            .iter()
            .all(|&value| value == 0.0 || value == 1.0)
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

    let result = acc.finalize().product;

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
    assert!(ptr::eq(
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

    // Input pixel (10,10), translated by (0.5,0.5) to reference (10.5,10.5), lands at output
    // 2·10.5 + ½ = 21.5: drop [20.7, 22.3]², 0.8 of cells 21 and 22 on each axis. Every input
    // pixel's drop stays inside its own 2×2 block of cells, so the block reads the bright value
    // undiluted and the cells beside it belong to zero-valued neighbours.
    let transform = Transform::translation(DVec2::new(0.5, 0.5));
    acc.add_image(image, &transform, 1.0, None);

    let result = acc.finalize().product;
    assert_eq!(result.image.width(), 40);
    assert_eq!(result.image.height(), 40);

    let out = result.image.channel(0);
    let weight = result.weight.as_ref().unwrap().channel(0);
    let at = |x: usize, y: usize| out[y * 40 + x];
    for (x, y) in [(21, 21), (22, 21), (21, 22), (22, 22)] {
        assert_eq!(at(x, y), 1.0, "block ({x},{y})");
    }
    for (x, y) in [(20, 21), (23, 21), (21, 20), (21, 23)] {
        assert_eq!(at(x, y), 0.0, "neighbour ({x},{y})");
        assert!(weight[y * 40 + x] > 0.0, "neighbour ({x},{y}) is covered");
    }
    // Far from the bright spot → 0; no pixel exceeds the input value.
    assert!(at(0, 0).abs() < 1e-5);
    assert!(
        out.iter().all(|&v| v <= 1.0 + 1e-5),
        "no pixel exceeds input max"
    );
}

#[test]
fn coverage_map() {
    let image = constant_mono_image(Size2us::new(4, 4), 1.0);
    let config = DrizzleConfig::x2().with_kernel(DrizzleKernel::Point);
    let mut acc = accumulator(ImageDimensions::new((4, 4), 1), config);
    acc.add_image(image, &Transform::identity(), 1.0, None);
    let result = acc.finalize().product;

    // Output 8×8. Each input pixel lands on the corner of its 2×2 block, (2·ix + ½, 2·iy + ½), which
    // rounds onto the odd cell (2·ix + 1, 2·iy + 1): covered by the one frame there, nowhere else.
    let coverage = result.coverage.as_ref().unwrap();
    assert_eq!(coverage[(1, 1)], 1.0);
    assert_eq!(coverage[(3, 3)], 1.0);
    assert_eq!(coverage[(0, 0)], 0.0);
    assert_eq!(coverage[(2, 2)], 0.0);
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
    let equal = acc.finalize().product;
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
    let unequal = acc.finalize().product;
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
    let product = acc.finalize().product;
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
                accumulator.add_image_with_band_rows(image.clone(), &transform, band_rows);
                accumulator.finalize().product
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

/// A band straddling the transform's vanishing line scans the whole frame.
///
/// `input_rows` bounds the input by inverse-mapping the band's four corners, which encloses the
/// interior only while the homogeneous divisor keeps one sign across the band. Where it changes sign
/// the mapped region is unbounded and four corners bound nothing, so the estimate has to widen to
/// the frame — a tight answer there drops flux with no diagnostic.
#[test]
fn input_row_estimate_widens_to_the_frame_across_the_vanishing_line() {
    let image = constant_mono_image(Size2us::new(16, 12), 1.0);

    // Inverse divisor `1 − 0.01·x`, which is zero at output column 100 and so takes both signs over
    // a 200-wide grid.
    // The warp goes reference → input, so the input rows come back through the homography itself.
    let straddling = WarpTransform::new(Transform::homography([
        1.0, 0.0, 0.0, 0.0, 1.0, 0.0, -0.01, 0.0,
    ]));
    assert_eq!(
        input_rows(&image, &straddling, 1.0, 0..4, 200, 0.5, 0.0),
        0..12
    );

    // A grid narrow enough to stay on one side of it keeps the corner bound. It is not the linear
    // answer: the divisor is 0.51 at column 49, so the far corner of output rows [-0.5, 3.5] maps
    // back to input row 3.5/0.51 = 6.86, and the estimate runs to 8 rather than 4.
    assert_eq!(
        input_rows(&image, &straddling, 1.0, 0..4, 50, 0.5, 0.0),
        0..8
    );

    // And the linear case, whose divisor is a constant 1: output rows [-0.5, 3.5] shifted up by
    // two are input rows [-2.5, 1.5], so rows 0, 1 and 2. A drop reaching 1.5 input rows from its
    // centre — the square kernel's — widens that to [-4, 3], rows 0 to 3.
    let shifted = WarpTransform::new(Transform::translation(DVec2::new(0.0, -2.0)));
    assert_eq!(input_rows(&image, &shifted, 1.0, 0..4, 200, 0.5, 0.0), 0..3);
    assert_eq!(input_rows(&image, &shifted, 1.0, 0..4, 200, 0.5, 1.5), 0..4);
}

#[test]
fn drizzle_accumulator_rejects_invalid_frame_inputs() {
    let config = DrizzleConfig::x2();
    let mut acc = accumulator(ImageDimensions::new((4, 4), 1), config);

    let mut frame = DrizzleFrame::new(
        constant_mono_image(Size2us::new(4, 4), 1.0),
        WarpTransform::new(Transform::identity()),
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
        constant_mono_image(Size2us::new(4, 4), 1.0),
        WarpTransform::new(Transform::identity()),
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
        constant_mono_image(Size2us::new(4, 4), 1.0),
        WarpTransform::new(Transform::identity()),
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
