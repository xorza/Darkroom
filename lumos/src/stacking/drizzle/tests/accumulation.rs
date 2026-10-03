use std::path::PathBuf;
use std::ptr;

use common::TempDir;

use super::*;
use crate::stacking::registration::distortion::sip::{SipConfig, SipPolynomial};

/// At scale 2 and pixfrac 0.8 every cell holds a quarter of exactly one drop (see
/// `one_bright_pixel_fills_its_own_block`), so a flat frame covers the whole grid at weight 0.25 and
/// reads back exactly, the first and last rows and columns included.
#[test]
fn a_flat_frame_at_scale_2_covers_every_cell() {
    let size = Size2us::new(100, 100);
    let product = drizzle_one(
        size,
        DrizzleConfig::x2(),
        constant_image(size, 0.5),
        &Transform::identity(),
        None,
    );
    assert!(product.image.channel(0).pixels().iter().all(|&p| p == 0.5));
    assert!(weight_plane(&product).pixels().iter().all(|&w| w == 0.25));
}

#[test]
fn drizzle_stack_empty_paths() {
    let config = DrizzleConfig::default();

    let result = drizzle_stack(
        Vec::<DrizzleFrame<PathBuf>>::new(),
        &config,
        &LoadContext::default(),
        &ProgressCallback::default(),
        &CancelToken::never(),
    );
    assert!(matches!(result.unwrap_err(), DrizzleError::NoFrames));
}

/// From paths, a drizzle gives what the same frames give in memory once loaded; a file that does
/// not load is `ImageLoad`; and the run's token, not the context's, governs the decode — a
/// cancelled run reports `Cancelled`, and a cancelled context under a live run is ignored.
#[test]
fn drizzle_stack_loads_its_frames_under_the_run_token() {
    let scratch = TempDir::new("lumos_drizzle_stack");
    let size = Size2us::new(16, 12);
    let transforms = [
        Transform::identity(),
        Transform::translation(DVec2::new(0.4, -0.3)),
    ];
    let paths: Vec<PathBuf> = (0..2)
        .map(|k| {
            let path = scratch.join(format!("frame{k}.tiff"));
            gray_image(
                size,
                (0..size.pixel_count())
                    .map(|i| ((i * 7 + k * 3) % 11) as f32 / 10.0)
                    .collect(),
            )
            .save(&path)
            .unwrap();
            path
        })
        .collect();
    let config = usual_config(DrizzleKernel::Turbo);
    let frames = |paths: &[PathBuf]| -> Vec<DrizzleFrame<PathBuf>> {
        paths
            .iter()
            .zip(transforms)
            .map(|(path, transform)| DrizzleFrame::new(path.clone(), warp_of(transform)))
            .collect()
    };

    let loaded: Vec<LinearImage> = paths
        .iter()
        .map(|path| LinearImage::from_file(path, &LoadContext::default()).unwrap())
        .collect();
    let in_memory = drizzle_images(
        drizzle_frames(loaded, &transforms),
        &config,
        &ProgressCallback::default(),
        &CancelToken::never(),
    )
    .unwrap()
    .product;
    let cancelled = CancelToken::new();
    cancelled.cancel();
    let cancelled_context = LoadContext {
        cancel: cancelled.clone(),
        ..LoadContext::default()
    };
    let from_paths = drizzle_stack(
        frames(&paths),
        &config,
        &cancelled_context,
        &ProgressCallback::default(),
        &CancelToken::never(),
    )
    .unwrap()
    .product;
    assert_eq!(
        from_paths.image.channel(0).pixels(),
        in_memory.image.channel(0).pixels()
    );

    let missing = [paths[0].clone(), scratch.join("missing.tiff")];
    assert!(matches!(
        drizzle_stack(
            frames(&missing),
            &config,
            &LoadContext::default(),
            &ProgressCallback::default(),
            &CancelToken::never(),
        ),
        Err(DrizzleError::ImageLoad(_))
    ));
    assert!(matches!(
        drizzle_stack(
            frames(&paths),
            &config,
            &LoadContext::default(),
            &ProgressCallback::default(),
            &cancelled,
        ),
        Err(DrizzleError::Cancelled)
    ));
}

#[test]
fn drizzle_images_empty() {
    let result = drizzle_images(
        Vec::new(),
        &DrizzleConfig::default(),
        &ProgressCallback::default(),
        &CancelToken::never(),
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
                constant_image(Size2us::new(16, 16), 0.5),
                WarpTransform::new(Transform::identity()),
            )
        })
        .collect();

    let result = drizzle_images(
        frames,
        &DrizzleConfig::default(),
        &ProgressCallback::default(),
        &cancel,
    );
    assert!(
        matches!(result.unwrap_err(), DrizzleError::Cancelled),
        "a cancelled drizzle must report cancellation, not a partial product"
    );

    // The same set completes when the run is live, so the guard is what stopped it.
    let frames: Vec<_> = (0..3)
        .map(|_| {
            DrizzleFrame::new(
                constant_image(Size2us::new(16, 16), 0.5),
                WarpTransform::new(Transform::identity()),
            )
        })
        .collect();
    assert!(
        drizzle_images(
            frames,
            &DrizzleConfig::default(),
            &ProgressCallback::default(),
            &CancelToken::never(),
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
    let image = gray_image(
        size,
        (0..size.pixel_count())
            .map(|i| (i % 7) as f32 * 0.25)
            .collect(),
    );

    let from_images = drizzle_images(
        vec![DrizzleFrame::new(image.clone(), warp.clone())],
        &config,
        &ProgressCallback::default(),
        &CancelToken::never(),
    )
    .unwrap();
    let mut acc = accumulator(ImageDimensions::new(size, 1), config);
    acc.add_frame(&DrizzleFrame::new(image, warp.clone()))
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
    acc.add_frame(&DrizzleFrame::new(gray_image(size, pixels), warp.clone()))
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
    acc.add_frame_with_band_rows(&DrizzleFrame::new(constant_image(size, 1.0), warp), 4);
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
    let a = constant_image(Size2us::new(20, 20), 0.5);
    let b = constant_image(Size2us::new(10, 10), 0.5);
    let result = drizzle_images(
        drizzle_frames(vec![a, b], &[Transform::identity(), Transform::identity()]),
        &DrizzleConfig::default(),
        &ProgressCallback::default(),
        &CancelToken::never(),
    );
    assert!(matches!(
        result.unwrap_err(),
        DrizzleError::DimensionMismatch(FrameDimensionMismatch { index: 1, .. })
    ));
}

/// An RGB drizzle shares one weight plane and one variance plane between its channels.
#[test]
fn drizzle_rgb_uses_shared_quality_planes() {
    let size = Size2us::new(50, 50);
    let image = rgb_image(
        size,
        vec![0.5; size.pixel_count()],
        vec![0.3; size.pixel_count()],
        vec![0.7; size.pixel_count()],
    );
    let mut acc = accumulator(ImageDimensions::new(size, 3), DrizzleConfig::x2());
    acc.add_image(image, &Transform::identity(), 1.0, None);
    let result = acc.finalize().product;

    assert!(matches!(result.weight, Some(QualityMap::Shared(_))));
    assert!(matches!(
        result.linear_variance,
        Some(QualityMap::Shared(_))
    ));
    assert!(ptr::eq(
        result.weight.as_ref().unwrap().channel(0),
        result.weight.as_ref().unwrap().channel(2)
    ));
}

/// The weight and linear-variance planes at scale 1 and pixfrac 1, where every input pixel is its
/// own output pixel at weight equal to its frame weight. Three frames of weight 1 give `Σw` = 3 and
/// `Σw²/(Σw)²` = 3/9 — an average of three, the noise reduction the identical frames' zero RMS
/// cannot show. Frames of weight 1 and 3 give `Σw` = 4 and (1 + 9)/16 = 0.625, above the 1/2 of an
/// equal pair: concentrating weight on fewer frames costs. Every value is a correctly rounded
/// quotient of small integers.
#[test]
fn weight_and_linear_variance_maps() {
    let size = Size2us::new(4, 4);
    let at = (2, 2);
    for (frames, weight, variance) in [
        (&[1.0, 1.0, 1.0][..], 3.0, 3.0f32 / 9.0),
        (&[1.0, 3.0][..], 4.0, 0.625),
    ] {
        let mut acc = accumulator(
            ImageDimensions::new(size, 1),
            kernel_config(DrizzleKernel::Turbo, 1.0, 1.0),
        );
        for &frame_weight in frames {
            acc.add_image(
                constant_image(size, 5.0),
                &Transform::identity(),
                frame_weight,
                None,
            );
        }
        let product = acc.finalize().product;
        assert_eq!(weight_plane(&product)[at], weight, "{frames:?}");
        assert_eq!(
            product.linear_variance.as_ref().unwrap().channel(0)[at],
            variance,
            "{frames:?}"
        );
        assert_eq!(product.image.channel(0)[at], 5.0);
    }
}

/// A declined plane is not produced, and declining it changes nothing about the image.
///
/// The weight map is not optional internally — the image is `Σfluxᵢwᵢ / Σwᵢ` and `min_weight_fraction`
/// gates fill against its maximum — so the risk this pins is that gating the *outputs* disturbs the
/// combine. Run with a non-zero `min_weight_fraction` and a transform that leaves the frame's edge thinly
/// covered, so the fill gate is actually exercised while coverage is declined.
#[test]
fn declined_quality_planes_are_absent_and_do_not_disturb_the_image() {
    let size = Size2us::new(24, 24);
    let image = constant_image(size, 0.5);
    let transform = Transform::translation(DVec2::new(1.7, -2.3));
    let product = |quality| {
        let config = DrizzleConfig {
            min_weight_fraction: 0.5,
            quality,
            ..DrizzleConfig::x2()
        };
        drizzle_one(size, config, image.clone(), &transform, None)
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
/// while the weight there is a quarter of the deepest pixel's, which a coverage of
/// `weight / max_weight` would read.
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
        constant_image(Size2us::new(side, side), 1.0),
        &Transform::identity(),
        1.0,
        None,
    );
    acc.add_image(
        constant_image(Size2us::new(side, side), 1.0),
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

    for (name, transform) in transforms {
        for kernel in DrizzleKernel::ALL {
            let drizzle = |band_rows: usize| {
                let mut accumulator = accumulator(dimensions, usual_config(kernel));
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
    let image = constant_image(Size2us::new(16, 12), 1.0);

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
        constant_image(Size2us::new(4, 4), 1.0),
        WarpTransform::new(Transform::identity()),
    );
    frame.pixel_weight_map = Some(Buffer2::new_filled(3, 3, 1.0));
    let error = acc.add_frame(&frame).unwrap_err();
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
        constant_image(Size2us::new(4, 4), 1.0),
        WarpTransform::new(Transform::identity()),
    );
    frame.weight = f32::NAN;
    let error = acc.add_frame(&frame).unwrap_err();
    assert!(matches!(
        error,
        DrizzleError::InvalidFrameWeight { index: 0, value } if value.is_nan()
    ));

    let mut pixel_weights = vec![1.0; 16];
    pixel_weights[5] = -0.25;
    let mut frame = DrizzleFrame::new(
        constant_image(Size2us::new(4, 4), 1.0),
        WarpTransform::new(Transform::identity()),
    );
    frame.pixel_weight_map = Some(Buffer2::new(4, 4, pixel_weights));
    let error = acc.add_frame(&frame).unwrap_err();
    assert!(matches!(
        error,
        DrizzleError::InvalidPixelWeight {
            frame_index: 0,
            pixel_index: 5,
            value: -0.25,
        }
    ));
}
