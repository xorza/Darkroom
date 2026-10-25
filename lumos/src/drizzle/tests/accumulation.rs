#![expect(
    clippy::cast_sign_loss,
    reason = "test fixtures are small images, with non-negative coordinates and offsets of a few dozen pixels"
)]

use std::path::PathBuf;

use common::{CancelToken, TempDir};
use glam::DVec2;

use super::*;
use crate::combine::error::{StackConfigError, StackError};
use crate::frame::frame_plane::FramePlane;
use crate::frame_store::decode_cache::DecodeCache;
use crate::image::channel_layout::ChannelLayout;
use crate::ingest::error::AdmissionError;
use crate::internals::images::rgb_image;
use crate::io::decoder::linear_decoder::LinearDecoder;
use crate::progress::progress_callback::ProgressCallback;
use crate::registration::distortion::sip::SipModel;
use crate::registration::distortion::sip::sip_order::SipOrder;
use common::Vec2us;

/// At scale 2 and pixfrac 0.8 every cell holds a quarter of exactly one drop (see
/// `one_bright_pixel_fills_its_own_block`), so a flat frame covers the whole grid at weight 0.25
/// and reads back exactly, the first and last rows and columns included.
#[test]
fn a_flat_frame_at_scale_2_covers_every_cell() {
    let size = Size2us::new(100, 100);
    let product = drizzle_one(
        DrizzleConfig::default(),
        &DrizzleFrame::new(constant_image(size, 0.5), warp_of(Transform::identity())),
    );
    assert!(product.image.channel(0).pixels().iter().all(|&p| p == 0.5));
    assert!(weight_plane(&product).pixels().iter().all(|&w| w == 0.25));
}

/// No frames is `NoFrames`, and an invalid config — its own, or a manual weight count that is not
/// the frame count — is refused before the first frame is decoded: the path here does not exist,
/// so a decode would have reported `ImageLoad` instead.
#[test]
fn drizzle_paths_refuses_before_decoding() {
    let result = DrizzleConfig {
        stack: plain_stack(),
        ..DrizzleConfig::default()
    }
    .drizzle_paths(
        Vec::<DrizzleFrame<PathBuf>>::new(),
        &IngestConfig::default(),
        ProgressMonitor::unattended(),
    );
    assert!(matches!(
        result.unwrap_err(),
        DrizzleError::Stack(StackError::NoFrames)
    ));

    let invalid = DrizzleConfig {
        pixfrac: 0.0,
        ..DrizzleConfig::default()
    };
    let result = DrizzleConfig {
        stack: plain_stack(),
        ..invalid
    }
    .drizzle_paths(
        vec![DrizzleFrame::new(
            PathBuf::from("does-not-exist.tiff"),
            warp_of(Transform::identity()),
        )],
        &IngestConfig::default(),
        ProgressMonitor::unattended(),
    );
    assert!(matches!(result.unwrap_err(), DrizzleError::Config(_)));

    let result = DrizzleConfig {
        stack: StackConfig {
            weighting: Weighting::Manual(vec![1.0, 2.0]),
            ..plain_stack()
        },
        ..DrizzleConfig::default()
    }
    .drizzle_paths(
        vec![DrizzleFrame::new(
            PathBuf::from("does-not-exist.tiff"),
            warp_of(Transform::identity()),
        )],
        &IngestConfig::default(),
        ProgressMonitor::unattended(),
    );
    assert!(matches!(
        result.unwrap_err(),
        DrizzleError::Stack(StackError::Config(
            StackConfigError::ManualWeightCountMismatch {
                expected: 1,
                actual: 2
            }
        ))
    ));
}

/// From paths, a drizzle gives what the same frames give in memory once loaded, and with
/// `keep_cache` reads them back from the decode cache on the next run; a file that does not load is
/// `ImageLoad`; and the run's token governs the decode — a cancelled run reports `Cancelled`.
#[test]
fn drizzle_paths_loads_its_frames_under_the_run_token() {
    let scratch = TempDir::new("lumos_drizzle_paths");
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
            .write_raster(&path)
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
        .map(|path| LoadCtx::default().linear(path).unwrap())
        .collect();
    let in_memory = config
        .drizzle_images(
            drizzle_frames(loaded, &transforms),
            &IngestConfig::default(),
            ProgressMonitor::unattended(),
        )
        .unwrap();
    let cancelled = CancelToken::new();
    cancelled.cancel();
    let from_paths = config
        .drizzle_paths(
            frames(&paths),
            &IngestConfig::default(),
            ProgressMonitor::unattended(),
        )
        .unwrap();
    assert_eq!(
        from_paths.image.channel(0).pixels(),
        in_memory.image.channel(0).pixels()
    );

    // Kept, the frames are written on the first run and read back by the next: a kept frame edited
    // on disk moves the second, which it would not if that run decoded the source.
    let kept = IngestConfig {
        keep_cache: true,
        ..IngestConfig::with_cache_dir(scratch.join("kept"))
    };
    let run_kept = || {
        config
            .drizzle_paths(frames(&paths), &kept, ProgressMonitor::unattended())
            .unwrap()
    };
    assert_eq!(
        run_kept().image.channel(0).pixels(),
        in_memory.image.channel(0).pixels()
    );
    DecodeCache::open(&kept.cache_dir)
        .unwrap()
        .edit_kept::<LinearDecoder>(&paths[1], &LoadCtx::default(), |plane| {
            for sample in plane.iter_mut() {
                *sample += 0.125;
            }
        });
    assert_ne!(
        run_kept().image.channel(0).pixels(),
        in_memory.image.channel(0).pixels(),
        "the edited kept frame is read back"
    );

    let missing = [paths[0].clone(), scratch.join("missing.tiff")];
    assert!(matches!(
        config.drizzle_paths(
            frames(&missing),
            &IngestConfig::default(),
            ProgressMonitor::unattended()
        ),
        Err(DrizzleError::Stack(StackError::ImageLoad(_)))
    ));
    assert!(matches!(
        config.drizzle_paths(
            frames(&paths),
            &IngestConfig::default(),
            ProgressMonitor::new(ProgressCallback::default(), cancelled)
        ),
        Err(DrizzleError::Stack(StackError::Cancelled))
    ));
}

#[test]
fn drizzle_images_empty() {
    let result = DrizzleConfig {
        stack: plain_stack(),
        ..DrizzleConfig::default()
    }
    .drizzle_images(
        Vec::new(),
        &IngestConfig::default(),
        ProgressMonitor::unattended(),
    );
    assert!(matches!(
        result.unwrap_err(),
        DrizzleError::Stack(StackError::NoFrames)
    ));
}

/// Every frame is admitted as the combine admits it, before its drops spread: a NaN at pixel 6 of
/// frame 1 would turn every output pixel its drop reaches into NaN, and is named instead.
#[test]
fn drizzle_admits_each_frame_before_it_deposits() {
    let mut pixels = vec![1.0; 16];
    pixels[6] = f32::NAN;
    let frames = vec![
        DrizzleFrame::new(
            constant_image(Size2us::new(4, 4), 1.0),
            WarpTransform::new(Transform::identity()),
        ),
        DrizzleFrame::new(
            gray_image(Size2us::new(4, 4), pixels),
            WarpTransform::new(Transform::identity()),
        ),
    ];
    let error = DrizzleConfig {
        stack: plain_stack(),
        ..DrizzleConfig::default()
    }
    .drizzle_images(
        frames,
        &IngestConfig::default(),
        ProgressMonitor::unattended(),
    )
    .unwrap_err();
    assert!(
        matches!(
            error,
            DrizzleError::Stack(StackError::Admission(AdmissionError::NonFiniteImageSample {
                idx: 1,
                channel: 0,
                pixel: 6,
                value,
            })) if value.is_nan()
        ),
        "{error:?}"
    );
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

    let result = DrizzleConfig {
        stack: plain_stack(),
        ..DrizzleConfig::default()
    }
    .drizzle_images(
        frames,
        &IngestConfig::default(),
        ProgressMonitor::new(ProgressCallback::default(), cancel),
    );
    assert!(
        result.unwrap_err().is_cancelled(),
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
        DrizzleConfig {
            stack: plain_stack(),
            ..DrizzleConfig::default()
        }
        .drizzle_images(
            frames,
            &IngestConfig::default(),
            ProgressMonitor::unattended()
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
    let sip = SipModel {
        order: SipOrder::Three,
        origin: center,
    }
    .fitted_under(&transform, &reference, &target);
    WarpTransform::with_sip(transform, sip)
}

/// A frame registered with SIP drizzles through the warp's inverse: `drizzle_images` and the
/// accumulator give the same image bit for bit — the combine of one frame is its own weighted mean,
/// `W·x̄/W` exact in f64 — every pixel converges, and a lone bright pixel at
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

    let from_images = config
        .drizzle_images(
            vec![DrizzleFrame::new(image.clone(), warp.clone())],
            &IngestConfig::default(),
            ProgressMonitor::unattended(),
        )
        .unwrap();
    let mut acc = accumulator(ImageDimensions::new(size, ChannelLayout::Mono), config);
    acc.add_frame(&DrizzleFrame::new(image, warp.clone()))
        .unwrap();
    assert_eq!(acc.unconverged_points(), 0);
    let from_accumulator = acc.finalize();
    assert_eq!(from_images.report.unconverged_points, 0);
    let bits = |product: &StackProduct| -> Vec<u32> {
        product
            .image
            .channel(0)
            .iter()
            .map(|v| v.to_bits())
            .collect()
    };
    assert_eq!(bits(&from_images), bits(&from_accumulator));

    let mut pixels = vec![0.0f32; size.pixel_count()];
    let t = Vec2us::new(20, 9);
    pixels[size.idx_of(t)] = 1.0;
    let point = kernel_config(DrizzleKernel::Point, 1.0, 1.0);
    let mut acc = accumulator(ImageDimensions::new(size, ChannelLayout::Mono), point);
    acc.add_frame(&DrizzleFrame::new(gray_image(size, pixels), warp.clone()))
        .unwrap();
    let out = acc.finalize();
    let r = warp
        .inverse()
        .apply(DVec2::new(t.x as f64, t.y as f64))
        .unwrap()
        .pos;
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
        ImageDimensions::new(size, ChannelLayout::Mono),
        kernel_config(DrizzleKernel::Turbo, 1.0, 1.0),
    );
    acc.add_frame_with_band_rows(&DrizzleFrame::new(constant_image(size, 1.0), warp), 4);
    assert_eq!(acc.unconverged_points(), failing);
    let result = acc.finalize();
    // No position was guessed for a failing pixel: every covered output pixel holds the constant
    // exactly.
    assert!(
        result
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
    let result = DrizzleConfig {
        stack: plain_stack(),
        ..DrizzleConfig::default()
    }
    .drizzle_images(
        drizzle_frames(vec![a, b], &[Transform::identity(), Transform::identity()]),
        &IngestConfig::default(),
        ProgressMonitor::unattended(),
    );
    assert!(matches!(
        result.unwrap_err(),
        DrizzleError::Stack(StackError::Admission(AdmissionError::DimensionMismatch(
            FrameDimensionMismatch { idx: 1, .. }
        )))
    ));
}

/// An RGB drizzle gives each channel its own inverse variance, as its noise is its own: channels
/// whose quantization σ is 1/8 and whose drops are a single sample each read `(Σw)²/Σw²v` = 64 at
/// scale 1. The drops' weight is the same in every channel.
#[test]
fn drizzle_rgb_gives_each_channel_its_variance() {
    let size = Size2us::new(8, 8);
    let mut image = rgb_image(
        size,
        [
            vec![0.5; size.pixel_count()],
            vec![0.3; size.pixel_count()],
            vec![0.7; size.pixel_count()],
        ],
    );
    image.metadata.derived.set_quantization_sigma(Some(0.125));
    let product = drizzle_plain(
        drizzle_frames(vec![image], &[Transform::identity()]),
        &kernel_config(DrizzleKernel::Turbo, 1.0, 1.0),
    )
    .unwrap();
    let weight = product.weight.as_ref().unwrap();
    let inverse_variance = product.inverse_variance.as_ref().unwrap();
    assert!(matches!(inverse_variance, QualityMap::PerChannel(_)));
    for channel in 0..3 {
        assert_eq!(weight.channel(channel)[(3, 3)], 1.0, "channel {channel}");
        assert_eq!(
            inverse_variance.channel(channel)[(3, 3)],
            64.0,
            "channel {channel}"
        );
    }
}

/// The weight and inverse variance planes at scale 1 and pixfrac 1, where every input pixel is its
/// own output pixel, of drop weight 1 and Kish size 1. The frames are constant, so their measured
/// noise is 0, and a quantization σ of 1 gives each sample unit variance. Three frames of weight 1
/// give `Σw` = 3 and an inverse variance `(Σw)²/Σw²·1` = 9/3 = 3 — an average of three, the noise
/// reduction the identical frames' zero RMS cannot show. Frames of weight 1 and 3 give `Σw` = 4 and
/// 16/(1 + 9) = 1.6, below the 2 of an equal pair: concentrating weight on fewer frames costs.
/// Every value is a correctly rounded quotient of small integers.
#[test]
fn weight_and_inverse_variance_maps() {
    let size = Size2us::new(4, 4);
    let at = (2, 2);
    for (frame_weights, weight, inverse_variance) in [
        (vec![1.0, 1.0, 1.0], 3.0, 3.0f32),
        (vec![1.0, 3.0], 4.0, 1.6),
    ] {
        let images: Vec<LinearImage> = frame_weights
            .iter()
            .map(|_| {
                let mut image = constant_image(size, 5.0);
                image.metadata.derived.set_quantization_sigma(Some(1.0));
                image
            })
            .collect();
        let transforms = vec![Transform::identity(); images.len()];
        let product = DrizzleConfig {
            stack: StackConfig {
                weighting: Weighting::Manual(frame_weights.clone()),
                ..plain_stack()
            },
            ..kernel_config(DrizzleKernel::Turbo, 1.0, 1.0)
        }
        .drizzle_images(
            drizzle_frames(images, &transforms),
            &IngestConfig::default(),
            ProgressMonitor::unattended(),
        )
        .unwrap();
        assert_eq!(weight_plane(&product)[at], weight, "{frame_weights:?}");
        assert_eq!(
            product.inverse_variance.as_ref().unwrap().channel(0)[at],
            inverse_variance,
            "{frame_weights:?}"
        );
        assert_eq!(product.image.channel(0)[at], 5.0);
    }
}

/// A declined plane is not produced, and declining it changes nothing about the image.
///
/// The fill gate reads the drops' depth, not the weight plane, so the risk this pins is that
/// declining the *outputs* disturbs the combine or the gate. Run with a non-zero
/// `min_weight_fraction` and a transform that leaves the frame's edge thinly covered, so the fill
/// gate is actually exercised while coverage is declined.
#[test]
fn declined_quality_planes_are_absent_and_do_not_disturb_the_image() {
    let size = Size2us::new(24, 24);
    let image = constant_image(size, 0.5);
    let transform = Transform::translation(DVec2::new(1.7, -2.3));
    let product = |quality| {
        let config = DrizzleConfig {
            min_weight_fraction: 0.5,
            ..DrizzleConfig::default()
        };
        DrizzleConfig {
            stack: StackConfig {
                quality,
                ..plain_stack()
            },
            ..config
        }
        .drizzle_images(
            drizzle_frames(vec![image.clone()], &[transform]),
            &IngestConfig::default(),
            ProgressMonitor::unattended(),
        )
        .unwrap()
    };

    let all = product(QualityPlanes::ALL);
    assert!(all.coverage.is_some() && all.weight.is_some() && all.inverse_variance.is_some());
    assert!(all.dispersion.is_some());

    let bare = product(QualityPlanes::IMAGE_ONLY);
    assert!(bare.coverage.is_none() && bare.weight.is_none() && bare.inverse_variance.is_none());

    // Each is independent of the others.
    let coverage_only = product(QualityPlanes {
        coverage: true,
        weight: false,
        inverse_variance: false,
        dispersion: false,
    });
    assert!(coverage_only.coverage.is_some());
    assert!(coverage_only.weight.is_none() && coverage_only.inverse_variance.is_none());

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
        all.coverage.as_ref().unwrap().to_plane(0).pixels(),
        coverage_only
            .coverage
            .as_ref()
            .unwrap()
            .to_plane(0)
            .pixels(),
        "coverage differed when the other planes were declined"
    );
}

/// Drizzle reports coverage as the share of *frames* that reached a pixel — the same quantity the
/// statistical combine reports, so a `StackProduct` means one thing whichever produced it.
///
/// The fixture tells that apart from the share of the accumulated weight: two frames overlapping on
/// part of the grid, the second carrying three times the frame weight of the first. In the band
/// only the first frame reaches, one of two frames contributed — coverage 0.5 — while the weight
/// there is a quarter of the deepest pixel's, which a coverage of `weight / max_weight` would read.
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
    let product = DrizzleConfig {
        stack: StackConfig {
            weighting: Weighting::Manual(vec![1.0, 3.0]),
            ..plain_stack()
        },
        ..config
    }
    .drizzle_images(
        drizzle_frames(
            vec![
                constant_image(Size2us::new(side, side), 1.0),
                constant_image(Size2us::new(side, side), 1.0),
            ],
            &[
                Transform::identity(),
                Transform::translation(DVec2::new(overlap_from as f64, 0.0)),
            ],
        ),
        &IngestConfig::default(),
        ProgressMonitor::unattended(),
    )
    .unwrap();
    let coverage = product
        .coverage
        .as_ref()
        .expect("coverage was requested")
        .to_plane(0);
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
/// Parallelizing a float accumulation is only sound because each output pixel belongs to exactly
/// one band and a band walks its inputs in the order the serial loop did, so every pixel's
/// contributions are summed in the same sequence whatever the band count. Run over warps whose
/// bands need *different* input pixels — a rotation makes a band's input strip diagonal, a quarter
/// turn makes it a few columns of every row, a SIP field bends it — so a scan that was even one row
/// or one column too tight would drop flux and show up here as a mismatch. One band reads the
/// whole output, so it scans every pixel whose drop lands anywhere on it.
#[test]
fn band_count_does_not_change_the_result() {
    let image = star_field(Size2us::new(64, 64), 24, 4242).image;
    let dimensions = image.dimensions();
    let centre = DVec2::new(32.0, 32.0);
    let warp = |transform: Transform| WarpTransform::new(transform.inverse());

    let warps = [
        (
            "translation",
            warp(Transform::translation(DVec2::new(3.7, -2.4))),
        ),
        (
            "rotation",
            warp(Transform::euclidean(DVec2::new(5.0, -3.0), 0.05)),
        ),
        (
            "similarity",
            warp(Transform::similarity(DVec2::new(2.0, 1.0), -0.03, 1.02)),
        ),
        (
            "homography",
            warp(Transform::homography([
                1.0, 0.002, 4.0, -0.001, 1.0, -2.0, 2e-5, 1e-5,
            ])),
        ),
        (
            "45°",
            warp(Transform::rotation_around(
                centre,
                std::f64::consts::FRAC_PI_4,
            )),
        ),
        (
            "quarter turn",
            warp(Transform::translation(DVec2::new(0.3, -0.6)).compose(
                &Transform::rotation_around(centre, std::f64::consts::FRAC_PI_2),
            )),
        ),
        (
            "sip",
            sip_warp(
                dimensions.size(),
                Transform::similarity(DVec2::new(0.7, -0.4), 0.3, 1.0),
                |d| d * 2e-5 * d.length_squared(),
            ),
        ),
    ];

    for (name, warp) in &warps {
        for kernel in DrizzleKernel::ALL {
            let drizzle = |band_rows: usize| {
                let mut accumulator = accumulator(dimensions, usual_config(kernel));
                let frame = DrizzleFrame {
                    source: image.clone(),
                    warp: warp.clone(),
                    pixel_weight_map: None,
                };
                accumulator.add_frame_with_band_rows(&frame, band_rows);
                let unconverged = accumulator.unconverged_points();
                (accumulator.finalize(), unconverged)
            };

            // One band is the serial walk; 5 rows over a 64- or 128-row output is a dozen or more
            // of them, so most drops land inside a band and some straddle a boundary.
            let (single, single_unconverged) = drizzle(dimensions.height() * 2);
            let (many, many_unconverged) = drizzle(5);

            let case = format!("{name}/{kernel:?}");
            for channel in 0..dimensions.channels() {
                assert_eq!(
                    single.image.channel(channel),
                    many.image.channel(channel),
                    "{case}: image channel {channel}"
                );
            }
            assert_eq!(weight_plane(&single), weight_plane(&many), "{case}: weight");
            assert_eq!(single_unconverged, many_unconverged, "{case}: unconverged");
        }
    }
}

/// The band's scan covers every drop that can reach the band, and a band straddling the
/// transform's vanishing line scans the whole frame.
///
/// `band_scan` bounds the input by inverse-mapping the band's four corners, which encloses the
/// interior only while the homogeneous divisor keeps one sign across the band. Where it changes
/// sign the mapped region is unbounded and four corners bound nothing, so the scan has to widen to
/// the frame — a tight answer there drops flux with no diagnostic.
#[test]
fn a_band_scan_covers_every_reaching_drop() {
    let image = constant_image(Size2us::new(16, 12), 1.0);
    let scan = |warp: &WarpTransform, width: usize, output: f64, input: f64| {
        let frame = DrizzleFrame::new(image.clone(), warp.clone());
        band_scan(&frame, width, DropReach { output, input }).collect::<Vec<_>>()
    };
    let rows = |warp: &WarpTransform, width: usize, output_margin: f64, input_margin: f64| {
        let scanned = scan(warp, width, output_margin, input_margin);
        scanned
            .first()
            .map_or(0..0, |first| first.0..scanned.last().unwrap().0 + 1)
    };

    // Inverse divisor `1 − 0.01·x`, which is zero at output column 100 and so takes both signs over
    // a 200-wide grid.
    // The warp goes reference → input, so the input rows come back through the homography itself.
    let straddling = WarpTransform::new(Transform::homography([
        1.0, 0.0, 0.0, 0.0, 1.0, 0.0, -0.01, 0.0,
    ]));
    assert_eq!(rows(&straddling, 200, 0.5, 0.0), 0..12);
    assert!(
        scan(&straddling, 200, 0.5, 0.0)
            .iter()
            .all(|(_, cols)| *cols == (0..16)),
        "an unbounded band scans every column"
    );

    // A grid narrow enough to stay on one side of it keeps the corner bound. It is not the linear
    // answer: the divisor is 0.51 at column 49, so the far corner of output rows [-0.5, 3.5] maps
    // back to input row 3.5/0.51 = 6.86, and the scan runs to 8 rather than 4.
    assert_eq!(rows(&straddling, 50, 0.5, 0.0), 0..8);

    // And the linear case, whose divisor is a constant 1: output rows [-0.5, 3.5] shifted up by
    // two are input rows [-2.5, 1.5], so rows 0, 1 and 2. A drop reaching 1.5 input rows from its
    // centre — the square kernel's — widens that to [-4, 3], rows 0 to 3.
    let shifted = WarpTransform::new(Transform::translation(DVec2::new(0.0, -2.0)));
    assert_eq!(rows(&shifted, 200, 0.5, 0.0), 0..3);
    assert_eq!(rows(&shifted, 200, 0.5, 1.5), 0..4);

    // A quarter turn sends output pixel (x, y) to input (−y, x − 2.25), so the input rows come
    // from the band's horizontal extent and its columns from the band's rows. A Lanczos-3 drop
    // reaches 3.5 output pixels, so the columns widen to [−3.5, 8.5] on a 6-wide grid and the rows
    // to [−5.75, 6.25]: rows 0 to 7. Without the column margin they stop at [−2.25, 2.75], rows 0
    // to 3, and the drops centred just right of the grid's last column lose the taps that land
    // inside it. The band's rows [−3.5, 6.5] come back as input columns [−6.5, 3.5]: in every row
    // the outline spans, columns 0 to 4 of the 16, not all of them, and none in the slack row 7
    // past it.
    let quarter_turn = WarpTransform::new(Transform::euclidean(
        DVec2::new(0.0, -2.25),
        std::f64::consts::FRAC_PI_2,
    ));
    assert_eq!(rows(&quarter_turn, 6, 3.5, 0.0), 0..8);
    let cols: Vec<_> = scan(&quarter_turn, 6, 3.5, 0.0)
        .into_iter()
        .map(|(_, cols)| cols)
        .collect();
    assert!(cols[..7].iter().all(|cols| *cols == (0..5)), "{cols:?}");
    assert!(cols[7].is_empty(), "the slack row reaches no column");
}

#[test]
fn drizzle_accumulator_rejects_invalid_frame_inputs() {
    let config = DrizzleConfig::default();
    let mut acc = accumulator(ImageDimensions::new((4, 4), ChannelLayout::Mono), config);

    let mut frame = DrizzleFrame::new(
        constant_image(Size2us::new(4, 4), 1.0),
        WarpTransform::new(Transform::identity()),
    );
    frame.pixel_weight_map = Some(Buffer2::new_filled((3, 3), 1.0));
    let error = acc.add_frame(&frame).unwrap_err();
    assert!(matches!(
        error,
        DrizzleError::Stack(StackError::PlaneDimensionMismatch {
            idx: 0,
            plane: FramePlane::PixelWeight,
            expected: Size2us {
                width: 4,
                height: 4
            },
            actual: Size2us {
                width: 3,
                height: 3
            },
        })
    ));

    // Every refusal left the accumulator as it was.
    assert!(acc.accumulated_weights().pixels().iter().all(|&w| w == 0.0));

    let mut pixel_weights = vec![1.0; 16];
    pixel_weights[5] = -0.25;
    let mut frame = DrizzleFrame::new(
        constant_image(Size2us::new(4, 4), 1.0),
        WarpTransform::new(Transform::identity()),
    );
    frame.pixel_weight_map = Some(Buffer2::new((4, 4), pixel_weights));
    let error = acc.add_frame(&frame).unwrap_err();
    assert!(matches!(
        error,
        DrizzleError::Stack(StackError::Admission(
            AdmissionError::InvalidQualityPlaneValue {
                idx: 0,
                plane: FramePlane::PixelWeight,
                pixel: 5,
                value: -0.25,
            }
        ))
    ));
}

/// A pixel the source holds no measurement for deposits nothing. The last
/// column of a 6 × 4 frame is null, with a fill of 5.0 under it. Drizzled at scale 1 with a
/// whole-pixel square drop on the identity, every other output pixel is exactly its 1.0, and the
/// last column takes the fill value with no weight. Before, the 5.0 deposited at full weight.
#[test]
fn a_null_pixel_deposits_nothing() {
    let size = Size2us::new(6, 4);
    let mut pixels = vec![1.0f32; size.pixel_count()];
    let mut nulls = vec![0.0f32; size.pixel_count()];
    for y in 0..size.height {
        pixels[y * size.width + 5] = 5.0;
        nulls[y * size.width + 5] = f32::NAN;
    }
    let mut image = gray_image(size, pixels);
    image.set_flags(PixelFlags::of_non_finite(size, &[&nulls]));
    let product = drizzle_one(
        kernel_config(DrizzleKernel::Square, 1.0, 1.0),
        &DrizzleFrame::new(image, warp_of(Transform::identity())),
    );
    for y in 0..size.height {
        for x in 0..size.width {
            let null = x == 5;
            assert_eq!(
                product.image.channel(0)[(x, y)],
                if null { 0.0 } else { 1.0 },
                "({x}, {y})"
            );
            assert_eq!(
                weight_plane(&product)[(x, y)],
                if null { 0.0 } else { 1.0 },
                "({x}, {y})"
            );
        }
    }
}
