mod mem_budget_probe;

use crate::internals::prelude::*;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use crate::calibration_masters::CalibrationMasters;
use crate::calibration_masters::cosmic_ray::config::{CosmicRayConfig, NoiseEstimation};
use crate::combine::config::{CombineMethod, StackConfig, Weighting};
use crate::combine::error::{Error as StackError, StackConfigError};
use crate::combine::rejection::Rejection;
use crate::error::FrameDimensionMismatch;
use crate::frame_store::frame_stats::FrameStats;
use crate::internals::synthetic::fixtures::star_field;
use crate::io::image::cfa::CfaType;
use crate::io::image::fits::cfa::save_cfa_fits;
use crate::pipeline::align::{align_and_stack, register_warp_and_stack};
use crate::pipeline::calibrate::calibrate_align_stack;
use crate::pipeline::config::{AlignStackConfig, Reference};
use crate::pipeline::frame::{DetectedFrame, PipelineFrame};
use crate::pipeline::result::{AlignStackResult, Error};
use crate::pipeline::tier::{FrameTier, StagePlan};
use crate::progress::{ProgressCallback, StackingProgress, StackingStage};
use crate::registration::config::Config as RegistrationConfig;
use crate::registration::resample::warp;
use crate::registration::transform::{Transform, TransformModel, TransformType, WarpTransform};
use crate::star_detection::config::Config as StarDetectionConfig;
use crate::star_detection::config::detection_config::DetectionConfig;
use crate::star_detection::detector::StarDetector;
use common::TempDir;

use crate::internals::assertions::bits;
use crate::internals::cfa::make_cfa;

fn base_field() -> LinearImage {
    star_field(Size2us::new(256, 256), 40, 66666).image
}

/// Warp `base` by a pure translation to fake a dithered exposure.
fn shifted(base: &LinearImage, dx: f64, dy: f64) -> LinearImage {
    let t = Transform::translation(DVec2::new(dx, dy));
    warp(
        base,
        &WarpTransform::new(t),
        RegistrationConfig::default().warp,
    )
    .image
}

#[test]
fn aligns_shifted_frames_into_a_sharp_stack() {
    let base = base_field();
    let frames = vec![
        base.clone(),
        shifted(&base, 8.0, -5.0),
        shifted(&base, -6.0, 7.0),
    ];

    let config = AlignStackConfig {
        reference: Reference::Index(0),
        ..Default::default()
    };
    let reports = Arc::new(Mutex::new(Vec::new()));
    let progress = ProgressCallback::new({
        let reports = Arc::clone(&reports);
        move |report: StackingProgress| {
            reports
                .lock()
                .unwrap()
                .push((report.stage, report.current, report.total));
        }
    });
    let result = align_and_stack(frames, &config, progress, CancelToken::never()).expect("stack");

    assert_eq!(result.alignment.reference, 0);
    assert_eq!(
        result.alignment.registered, 3,
        "all three frames should stack"
    );
    assert!(
        result.alignment.dropped.is_empty(),
        "dropped: {:?}",
        result.alignment.dropped
    );

    // The detection funnel reaches the caller instead of only the log: one entry per input frame,
    // in input order, each one internally consistent.
    assert_eq!(
        result.detection.len(),
        3,
        "one funnel entry per input frame"
    );
    for (frame, diagnostics) in result.detection.iter().enumerate() {
        assert!(
            diagnostics.stars_after_centroid > 0,
            "frame {frame} measured no stars"
        );
        assert!(
            diagnostics.stars_after_centroid
                <= diagnostics.candidates_after_filtering + diagnostics.deblended_components,
            "frame {frame}: centroids cannot exceed candidates plus deblends"
        );
    }

    // The pipeline's own stages reach the callback, not just the combine's. Counters come off
    // shared atomics, so the reports arrive in any order — sort before comparing.
    let reports = reports.lock().unwrap();
    // In arrival order: the parallel stages report through one counter, so the callback sees each
    // count once and in order, however the workers finish.
    let currents = |wanted: StackingStage| {
        reports
            .iter()
            .filter(|(stage, ..)| *stage == wanted)
            .map(|(_, current, _)| *current)
            .collect::<Vec<usize>>()
    };
    let totals = |wanted: StackingStage| {
        reports
            .iter()
            .filter(|(stage, ..)| *stage == wanted)
            .map(|(.., total)| *total)
            .collect::<Vec<_>>()
    };

    assert_eq!(currents(StackingStage::Preparing), [1, 2, 3]);
    assert_eq!(totals(StackingStage::Preparing), [3, 3, 3]);
    // Registration counts every frame but the reference, which needs none.
    assert_eq!(currents(StackingStage::Registering), [1, 2]);
    assert_eq!(totals(StackingStage::Registering), [2, 2]);
    // The combine counts chunk-channel pairs, from 1 to its total, with no report before work.
    let combining = currents(StackingStage::Combining);
    let combining_total = totals(StackingStage::Combining)[0];
    assert_eq!(combining, (1..=combining_total).collect::<Vec<_>>());
    assert_eq!(
        currents(StackingStage::Drizzling),
        Vec::<usize>::new(),
        "a statistical combine must not report the drizzle stage"
    );

    // Alignment check: every frame was warped back to the reference, so the reference's
    // brightest star must reappear at the same place in the combined image.
    let mut det = StarDetector::from_config(StarDetectionConfig::default()).unwrap();
    let ref_pos = det.detect(&base).stars[0].pos;
    let stack_stars = det.detect(&result.product.image).stars;
    let nearest = stack_stars
        .iter()
        .map(|s| (s.pos - ref_pos).length())
        .fold(f64::MAX, f64::min);
    assert!(
        nearest < 0.5,
        "reference's brightest star not aligned in the stack: nearest {nearest:.3} px"
    );
}

#[test]
fn drops_unregisterable_frame_and_stacks_the_rest() {
    let base = base_field();
    let dims = base.dimensions();
    // A flat frame has no stars → registration fails → it is dropped, not fatal. Two of them, at
    // non-adjacent indices, so `dropped` also pins its documented ascending order — no sort
    // produces that, only rayon's order-preserving indexed `collect`.
    let blank = || LinearImage::from_pixels(dims, vec![0.1; dims.pixel_count()]);
    let frames = vec![
        base.clone(),
        blank(),
        shifted(&base, 5.0, 3.0),
        blank(),
        shifted(&base, -4.0, 6.0),
    ];

    let config = AlignStackConfig {
        reference: Reference::Index(0),
        ..Default::default()
    };
    let result = align_and_stack(
        frames,
        &config,
        ProgressCallback::default(),
        CancelToken::never(),
    )
    .expect("stack");

    assert_eq!(
        result.alignment.dropped,
        vec![1, 3],
        "both blank frames should be dropped, in ascending index order"
    );
    assert_eq!(
        result.alignment.registered, 3,
        "reference + two aligned frames"
    );

    // Manual weights are one per input light; the dropped frames take theirs with them, so the
    // three survivors combine under three weights rather than failing on five.
    let weighted = AlignStackConfig {
        stack: StackConfig {
            weighting: Weighting::Manual(vec![1.0, 9.0, 2.0, 9.0, 3.0]),
            ..Default::default()
        },
        ..config
    };
    let frames = vec![
        base.clone(),
        blank(),
        shifted(&base, 5.0, 3.0),
        blank(),
        shifted(&base, -4.0, 6.0),
    ];
    let result = align_and_stack(
        frames,
        &weighted,
        ProgressCallback::default(),
        CancelToken::never(),
    )
    .expect("manual weights follow the survivors");
    assert_eq!(result.alignment.dropped, vec![1, 3]);
}

#[test]
fn stacked_master_inherits_reference_frame_metadata() {
    // The master's metadata comes from the reference frame (the alignment anchor), not frame 0,
    // so the RAM and streaming tiers agree. With reference = index 1, frame 0 is a (warped)
    // non-reference frame whose metadata must NOT win.
    let base = base_field();
    let mut f0 = shifted(&base, 5.0, 3.0);
    let mut f1 = base.clone(); // the reference (index 1)
    let mut f2 = shifted(&base, -4.0, 6.0);
    f0.metadata.exposure_time = Some(10.0);
    f1.metadata.exposure_time = Some(20.0);
    f2.metadata.exposure_time = Some(30.0);
    f0.metadata.camera_white_balance = Some([1.5, 1.0, 2.0, 1.0]);
    f1.metadata.camera_white_balance = Some([2.0, 1.0, 1.25, 1.0]);
    f2.metadata.camera_white_balance = Some([1.25, 1.0, 1.75, 1.0]);

    let config = AlignStackConfig {
        reference: Reference::Index(1),
        ..Default::default()
    };
    let result = align_and_stack(
        vec![f0, f1, f2],
        &config,
        ProgressCallback::default(),
        CancelToken::never(),
    )
    .expect("stack");

    assert_eq!(result.alignment.reference, 1);
    assert_eq!(
        result.product.image.metadata.exposure_time,
        Some(20.0),
        "master must inherit the reference (index 1) metadata, not frame 0's"
    );
    assert_eq!(
        result.product.image.metadata.camera_white_balance,
        Some([2.0, 1.0, 1.25, 1.0])
    );
}

#[test]
fn mismatched_frame_dimensions_are_rejected_before_registration() {
    // `warp` reprojects into the source frame's own grid, so a frame from a different sensor
    // would reach the combine as a differently-sized plane rather than as an error. The guard
    // sits ahead of registration; frame 1 is the first mismatch even though frame 2 also differs.
    let base = base_field();
    let odd = LinearImage::from_pixels(ImageDimensions::new((128, 128), 1), vec![0.1; 128 * 128]);
    let odder = LinearImage::from_pixels(ImageDimensions::new((64, 64), 1), vec![0.1; 64 * 64]);

    let error = align_and_stack(
        vec![base, odd, odder],
        &AlignStackConfig::default(),
        ProgressCallback::default(),
        CancelToken::never(),
    )
    .unwrap_err();

    let Error::Stack(StackError::DimensionMismatch(FrameDimensionMismatch {
        index,
        expected,
        actual,
    })) = error
    else {
        panic!("expected a dimension mismatch, got {error:?}");
    };
    assert_eq!(index, 1);
    assert_eq!(expected, ImageDimensions::new((256, 256), 1));
    assert_eq!(actual, ImageDimensions::new((128, 128), 1));
}

#[test]
fn an_invalid_registration_config_is_reported_as_one() {
    // `register` returns the same error type for "this config is invalid" and "these two
    // catalogs did not match", and the pipeline reads the latter as a frame to drop — so before
    // the config was validated up front, a bad registration config made every frame "fail to
    // register" and surfaced as `AllFramesDropped`, blaming the data.
    let base = base_field();
    let frames = vec![
        base.clone(),
        shifted(&base, 5.0, 3.0),
        shifted(&base, -4.0, 6.0),
    ];

    let mut config = AlignStackConfig {
        reference: Reference::Index(0),
        ..Default::default()
    };
    // Homography needs four points, so a three-match floor can never be satisfied.
    config.registration.transform_type = TransformModel::Fixed(TransformType::Homography);
    config.registration.matching.min_matches = 3;
    assert!(
        config.registration.validate().is_err(),
        "premise: this registration config must be invalid"
    );

    let error = align_and_stack(
        frames,
        &config,
        ProgressCallback::default(),
        CancelToken::never(),
    )
    .unwrap_err();
    assert!(
        matches!(error, Error::RegistrationConfig(_)),
        "expected the config to be blamed, got {error:?}"
    );
}

#[test]
fn a_bad_registration_config_is_never_mistaken_for_frames_that_would_not_match() {
    // The up-front `validate` is a fast fail, not the guarantee: `register` reports an invalid
    // config with the same error type as "these two catalogs did not match", which the per-frame
    // loop drops. Enter through the shared body so no up-front check runs, and the loop itself
    // has to tell them apart.
    let base = base_field();
    let images = vec![
        base.clone(),
        shifted(&base, 5.0, 3.0),
        shifted(&base, -4.0, 6.0),
    ];

    let mut config = AlignStackConfig {
        reference: Reference::Index(0),
        ..Default::default()
    };
    // Homography needs four points, so a three-match floor can never be satisfied.
    config.registration.transform_type = TransformModel::Fixed(TransformType::Homography);
    config.registration.matching.min_matches = 3;
    assert!(
        config.registration.validate().is_err(),
        "premise: this registration config must be invalid"
    );

    let mut detector = StarDetector::from_config(config.detection.clone()).unwrap();
    let detected = images
        .into_iter()
        .map(|image| {
            let result = detector.detect(&image);
            DetectedFrame {
                stars: result.stars,
                diagnostics: result.diagnostics,
                stats: FrameStats::measure(&image),
                image: PipelineFrame::Resident(image),
            }
        })
        .collect();

    let error = register_warp_and_stack(
        detected,
        &config,
        StagePlan {
            tier: FrameTier::Ram,
            warp_concurrency: 1,
        },
        ProgressCallback::default(),
        CancelToken::never(),
    )
    .unwrap_err();
    let Error::RegistrationConfig(invalid) = error else {
        panic!("expected the config to be blamed, got {error:?}")
    };
    assert_eq!(invalid.field, "min_matches");
}

#[test]
fn an_invalid_stack_config_is_caught_before_the_frames_are_worked() {
    // The combine validates its own config, but only after every frame has been decoded,
    // detected, registered and warped — so a run whose frames also fail to register would
    // report `AllFramesDropped` and never mention the config at all. Validating up front means
    // the config is blamed, and nothing upstream is paid for.
    let base = base_field();
    let dims = base.dimensions();
    let blank = || LinearImage::from_pixels(dims, vec![0.1; dims.pixel_count()]);
    let frames = vec![base, blank(), blank()];

    let config = AlignStackConfig {
        reference: Reference::Index(0),
        stack: StackConfig {
            method: CombineMethod::Mean(Rejection::sigma_clip(f32::NAN)),
            ..Default::default()
        },
        ..Default::default()
    };

    let error = align_and_stack(
        frames,
        &config,
        ProgressCallback::default(),
        CancelToken::never(),
    )
    .unwrap_err();
    assert!(
        matches!(
            error,
            Error::Stack(StackError::Config(StackConfigError::Field(invalid)))
                if invalid.field == "sigma_low"
        ),
        "expected the stack config to be blamed rather than the frames, got {error:?}"
    );
}

#[test]
fn all_non_reference_frames_dropped_errors() {
    // With the reference produced in-place (it survives in `frames`), "nothing aligned" means
    // only the reference remains — guard the changed `frames.len() <= 1` condition.
    let base = base_field();
    let dims = base.dimensions();
    let blank = || LinearImage::from_pixels(dims, vec![0.1; dims.pixel_count()]);
    // Reference has stars; both others are blank → both fail to register → nothing aligns.
    let frames = vec![base, blank(), blank()];

    let config = AlignStackConfig {
        reference: Reference::Index(0),
        ..Default::default()
    };
    let err = align_and_stack(
        frames,
        &config,
        ProgressCallback::default(),
        CancelToken::never(),
    )
    .unwrap_err();
    assert!(
        matches!(err, Error::AllFramesDropped { count: 2 }),
        "all non-reference frames dropped → AllFramesDropped {{ count: 2 }}, got {err:?}"
    );
}

#[test]
fn auto_reference_picks_the_richest_frame() {
    let base = base_field();
    // Frame 1 (full field) has far more stars than frame 0 (a near-blank), so Auto must
    // anchor on frame 1.
    let dims = base.dimensions();
    let sparse = LinearImage::from_pixels(dims, vec![0.1; dims.pixel_count()]);
    let frames = vec![sparse, base.clone(), shifted(&base, 4.0, -3.0)];

    let result = align_and_stack(
        frames,
        &AlignStackConfig::default(),
        ProgressCallback::default(),
        CancelToken::never(),
    )
    .expect("stack");
    assert_ne!(
        result.alignment.reference, 0,
        "Auto must not anchor on the near-blank frame"
    );
    assert_eq!(
        result.alignment.dropped,
        vec![0],
        "the near-blank frame can't register"
    );
}

#[test]
fn public_input_errors() {
    let err = align_and_stack(
        Vec::new(),
        &AlignStackConfig::default(),
        ProgressCallback::default(),
        CancelToken::never(),
    )
    .unwrap_err();
    assert!(matches!(err, Error::NoFrames));

    let config = AlignStackConfig {
        detection: StarDetectionConfig {
            detection: DetectionConfig {
                sigma_threshold: 0.0,
                ..Default::default()
            },
            ..StarDetectionConfig::default()
        },
        ..AlignStackConfig::default()
    };
    let image = LinearImage::from_pixels(ImageDimensions::new((1, 1), 1), vec![0.0]);
    let error = align_and_stack(
        vec![image],
        &config,
        ProgressCallback::default(),
        CancelToken::never(),
    )
    .unwrap_err();
    let Error::DetectionConfig(invalid) = error else {
        panic!("expected a detection config error, got {error:?}")
    };
    assert_eq!((invalid.field, invalid.value), ("sigma_threshold", 0.0));

    // Checked at entry, before any frame is worked: a weight count that does not match the
    // lights, a cosmic-ray config the detector cannot run, and a light with a non-finite sample.
    let dims = ImageDimensions::new((4, 4), 1);
    let flat = || LinearImage::from_pixels(dims, vec![0.1; 16]);
    let run = |lights: Vec<LinearImage>, config: &AlignStackConfig| {
        align_and_stack(
            lights,
            config,
            ProgressCallback::default(),
            CancelToken::never(),
        )
        .unwrap_err()
    };
    let manual = AlignStackConfig {
        stack: StackConfig {
            weighting: Weighting::Manual(vec![1.0]),
            ..Default::default()
        },
        ..AlignStackConfig::default()
    };
    let error = run(vec![flat(), flat()], &manual);
    assert!(
        matches!(
            error,
            Error::Stack(StackError::Config(
                StackConfigError::ManualWeightCountMismatch {
                    expected: 2,
                    actual: 1
                }
            ))
        ),
        "{error:?}"
    );

    let cosmic = AlignStackConfig {
        cosmic_ray: Some(CosmicRayConfig {
            niter: 0,
            ..Default::default()
        }),
        ..AlignStackConfig::default()
    };
    let Error::CosmicRayConfig(invalid) = run(vec![flat()], &cosmic) else {
        panic!("expected a cosmic-ray config error")
    };
    assert_eq!(invalid.field, "cosmic-ray niter");

    let mut pixels = vec![0.1; 16];
    pixels[5] = f32::NAN;
    let error = run(
        vec![flat(), LinearImage::from_pixels(dims, pixels)],
        &AlignStackConfig::default(),
    );
    assert!(
        matches!(
            error,
            Error::Stack(StackError::NonFiniteImageSample {
                index: 1,
                channel: 0,
                pixel: 5,
                ..
            })
        ),
        "{error:?}"
    );
}

/// Persist `image` as a single-channel (`CfaType::Mono`) Lumos CFA FITS light — the cheapest
/// input `calibrate_align_stack` accepts, since the mono demosaic is a passthrough and the frame
/// reaches detection unchanged. `exposure_time` is distinct per frame so the stacked master's
/// metadata identifies which frame it was inherited from.
fn write_mono_cfa_light(directory: &Path, index: usize, image: &LinearImage) -> PathBuf {
    let path = directory.join(format!("light_{index}.fits"));
    let mut cfa = make_cfa(
        Size2us::new(image.width(), image.height()),
        image.channel(0).pixels().to_vec(),
        CfaType::Mono,
    );
    cfa.metadata.exposure_time = Some(10.0 + index as f64);
    save_cfa_fits(&path, &cfa).expect("write synthetic CFA FITS light");
    path
}

/// The RAW front end checks each light as it decodes: a light whose dimensions differ from the
/// first light's header is refused at its own index, and parametric cosmic-ray noise on a light
/// whose decoder recorded no ADC step — a float FITS — names that light.
#[test]
fn the_raw_front_end_checks_each_light_at_decode() {
    let scratch = TempDir::new("lumos_decode_checks");
    let flat = |side: usize| {
        LinearImage::from_pixels(
            ImageDimensions::new((side, side), 1),
            vec![0.1; side * side],
        )
    };
    let paths = [
        write_mono_cfa_light(scratch.path(), 0, &flat(32)),
        write_mono_cfa_light(scratch.path(), 1, &flat(32)),
        write_mono_cfa_light(scratch.path(), 2, &flat(16)),
    ];
    let run = |paths: &[PathBuf], config: &AlignStackConfig| {
        calibrate_align_stack(
            paths,
            &CalibrationMasters::default(),
            config,
            ProgressCallback::default(),
            CancelToken::never(),
        )
        .unwrap_err()
    };

    let error = run(&paths, &AlignStackConfig::default());
    let Error::Stack(StackError::DimensionMismatch(FrameDimensionMismatch {
        index,
        expected,
        actual,
    })) = error
    else {
        panic!("expected a dimension mismatch, got {error:?}");
    };
    assert_eq!(
        (index, expected, actual),
        (
            2,
            ImageDimensions::new((32, 32), 1),
            ImageDimensions::new((16, 16), 1)
        )
    );

    let parametric = AlignStackConfig {
        cosmic_ray: Some(CosmicRayConfig {
            noise: NoiseEstimation::Gain {
                electrons_per_adu: 1.5,
            },
            ..Default::default()
        }),
        ..AlignStackConfig::default()
    };
    let error = run(&paths[..1], &parametric);
    let Error::CosmicRay { path, .. } = error else {
        panic!("expected the light without an ADC step to be named, got {error:?}");
    };
    assert_eq!(path, paths[0]);
}

/// Both front ends report the same stages for the same work, so a progress consumer can read the
/// stream without knowing which entry point was called.
///
/// This is what `StackingStage::Preparing` is for: the raw path decodes and detects in one pass
/// over the frames, the already-decoded path only detects, and both report that pass once per
/// frame under the same name. With a variant per route, a consumer would have to know the route
/// to line the two streams up.
#[test]
fn both_front_ends_report_the_same_stages() {
    let scratch = TempDir::new("lumos_stage_parity");
    let base = base_field();
    let frames = [
        base.clone(),
        shifted(&base, 4.0, -3.0),
        shifted(&base, -2.0, 5.0),
    ];

    let stages = |reports: &Mutex<Vec<(StackingStage, usize, usize)>>| {
        let mut seen: Vec<StackingStage> = reports
            .lock()
            .unwrap()
            .iter()
            .map(|(stage, ..)| *stage)
            .collect();
        seen.sort_by_key(|stage| format!("{stage:?}"));
        seen.dedup();
        seen
    };
    let recorder = || {
        let reports = Arc::new(Mutex::new(Vec::new()));
        let callback = ProgressCallback::new({
            let reports = Arc::clone(&reports);
            move |progress| {
                reports
                    .lock()
                    .unwrap()
                    .push((progress.stage, progress.current, progress.total));
            }
        });
        (reports, callback)
    };

    let mut config = AlignStackConfig::default();
    config.registration.ransac.seed = Some(0x5EED_0F5E);

    let (raw_reports, raw_progress) = recorder();
    let paths: Vec<PathBuf> = frames
        .iter()
        .enumerate()
        .map(|(index, frame)| write_mono_cfa_light(scratch.path(), index, frame))
        .collect();
    calibrate_align_stack(
        &paths,
        &CalibrationMasters::default(),
        &config,
        raw_progress,
        CancelToken::never(),
    )
    .expect("raw-path stack");

    let (decoded_reports, decoded_progress) = recorder();
    align_and_stack(
        frames.to_vec(),
        &config,
        decoded_progress,
        CancelToken::never(),
    )
    .expect("decoded-path stack");

    assert_eq!(
        stages(&raw_reports),
        stages(&decoded_reports),
        "the two front ends emitted different stage sets for the same work"
    );
    assert!(
        stages(&raw_reports).contains(&StackingStage::Preparing),
        "neither front end reported the preparing pass"
    );
}

/// The all-RAM and memory-bounded runs must produce the same stack, in two passes and in one.
/// [`FrameTier`] decides only where a frame lives: a resident frame moves out of `PipelineFrame`, a
/// spilled one is read back from its memory map, and the combined result has to be bit-identical
/// either way. A named reference takes each light through one pass, and a found one through two,
/// and they too agree.
///
/// Both runs read the same mono-CFA FITS lights and differ only in `memory_override`, the input
/// `MemoryPlan::plan` keys its tier decision on. RANSAC is seeded, removing the pipeline's only
/// other source of nondeterminism, so any difference the assertions find is a real divergence.
#[test]
fn ram_and_streaming_tiers_produce_identical_stacks() {
    let scratch = TempDir::new("lumos_tier_equivalence");
    // Smaller than `base_field`, since the lights are stacked four times.
    let base = star_field(Size2us::new(160, 160), 24, 66666).image;

    // Five dithered exposures: five clears `StackConfig`'s default `SmallN::median_below(5)`, so
    // the σ-clipped mean actually runs and the combine emits a linear-variance plane — without
    // that the comparison would silently skip one of the four output planes. Two starless frames
    // at non-adjacent indices fail registration on both tiers, so the drop bookkeeping and its
    // ascending order are compared too.
    let dims = base.dimensions();
    let blank = || LinearImage::from_pixels(dims, vec![0.1; dims.pixel_count()]);
    let frames = [
        base.clone(),
        shifted(&base, 6.0, -4.0),
        blank(),
        shifted(&base, -5.0, 7.0),
        shifted(&base, 3.0, 9.0),
        blank(),
        shifted(&base, -8.0, -2.0),
    ];
    // Frames 1 and 3 declare a block of pixels null, so both tiers carry a mask from decode to
    // warp: the spill tier has to bring it back from disk for the two stacks to agree.
    let paths: Vec<PathBuf> = frames
        .iter()
        .enumerate()
        .map(|(index, frame)| {
            let mut frame = frame.clone();
            if index == 1 || index == 3 {
                let width = frame.width();
                let pixels = frame.channel_mut(0).pixels_mut();
                for y in 10..18 {
                    pixels[y * width + 20..y * width + 30].fill(f32::NAN);
                }
            }
            write_mono_cfa_light(scratch.path(), index, &frame)
        })
        .collect();

    let mut config = AlignStackConfig::default();
    config.registration.ransac.seed = Some(0x5EED_0F5E);

    let mut ram_config = config.clone();
    ram_config.stack.ingest.memory_override = Some(u64::MAX);
    ram_config.stack.ingest.cache_dir = scratch.join("ram_cache");

    let mut streaming_config = config;
    streaming_config.stack.ingest.memory_override = Some(1);
    streaming_config.stack.ingest.cache_dir = scratch.join("streaming_cache");
    // Kept so the premise assertion below can observe that the spill tier really ran; the whole
    // scratch tree goes away when `scratch` drops.
    streaming_config.stack.ingest.keep_cache = true;

    let masters = CalibrationMasters::default();
    let run = |config: &AlignStackConfig| {
        calibrate_align_stack(
            &paths,
            &masters,
            config,
            ProgressCallback::default(),
            CancelToken::never(),
        )
        .expect("stack the lights")
    };
    let ram = run(&ram_config);
    let streaming = run(&streaming_config);

    // Premise: the two budgets must straddle the tier boundary. Only the streaming path creates a
    // spill directory, so its presence — and the RAM path's lack of one — is what proves this test
    // exercised two code paths rather than the same one twice.
    assert!(
        streaming_config.stack.ingest.cache_dir.is_dir(),
        "streaming tier never spilled; both runs took the RAM path"
    );
    assert!(
        !ram_config.stack.ingest.cache_dir.exists(),
        "RAM tier spilled to disk; both runs took the streaming path"
    );
    assert_eq!(
        ram.alignment.dropped,
        vec![2, 5],
        "the two starless frames should drop, in ascending index order"
    );
    assert_eq!(
        ram.alignment.registered, 5,
        "every dithered frame should register against the reference"
    );
    // The master inherits the reference frame's metadata. Distinct per-frame exposure times make
    // the comparison below non-vacuous.
    assert!(
        ram.product.image.metadata.exposure_time.is_some(),
        "per-frame exposure time did not survive the FITS round-trip; \
         the metadata comparison would be vacuous"
    );

    // Naming the reference the automatic choice found takes each light through one pass, on both
    // tiers, and the stack is the same. The spilled one-pass run writes each light once, warped:
    // no calibrated light is parked before its registration, as the two-pass run parks every one.
    let one_pass = |config: &AlignStackConfig, cache: &str| {
        let mut config = config.clone();
        config.reference = Reference::Index(ram.alignment.reference);
        config.stack.ingest.cache_dir = scratch.join(cache);
        (run(&config), config.stack.ingest.cache_dir)
    };
    let (ram_one_pass, _) = one_pass(&ram_config, "ram_one_pass_cache");
    let (streaming_one_pass, one_pass_dir) = one_pass(&streaming_config, "one_pass_cache");
    let spilled_names = |directory: &Path| {
        let mut names = Vec::new();
        let mut pending = vec![directory.to_path_buf()];
        while let Some(directory) = pending.pop() {
            for entry in std::fs::read_dir(&directory).unwrap() {
                let path = entry.unwrap().path();
                if path.is_dir() {
                    pending.push(path);
                } else {
                    names.push(path.file_name().unwrap().to_string_lossy().into_owned());
                }
            }
        }
        names
    };
    let two_pass_names = spilled_names(&streaming_config.stack.ingest.cache_dir);
    let one_pass_names = spilled_names(&one_pass_dir);
    assert!(two_pass_names.iter().any(|name| name.starts_with("calib_")));
    assert!(
        one_pass_names
            .iter()
            .any(|name| name.starts_with("warped_"))
    );
    assert!(
        !one_pass_names.iter().any(|name| name.starts_with("calib_")),
        "{one_pass_names:?}"
    );

    for (other, label) in [
        (&streaming, "streaming"),
        (&ram_one_pass, "RAM one-pass"),
        (&streaming_one_pass, "streaming one-pass"),
    ] {
        assert_same_stack(&ram, other, label);
    }
}

/// Two runs stack the same lights against the same reference, bit for bit: every plane, the
/// alignment, the detection funnels and the inherited metadata.
fn assert_same_stack(expected: &AlignStackResult, actual: &AlignStackResult, label: &str) {
    assert_eq!(expected.alignment, actual.alignment, "{label}");
    assert_eq!(expected.detection, actual.detection, "{label}");
    let (expected, actual) = (&expected.product, &actual.product);
    assert_eq!(
        expected.image.dimensions(),
        actual.image.dimensions(),
        "{label}"
    );
    for channel in 0..expected.image.channels() {
        assert_eq!(
            bits(expected.image.channel(channel).pixels()),
            bits(actual.image.channel(channel).pixels()),
            "{label}: image channel {channel}"
        );
        assert_eq!(
            bits(expected.weight.as_ref().unwrap().channel(channel)),
            bits(actual.weight.as_ref().unwrap().channel(channel)),
            "{label}: weight channel {channel}"
        );
        assert_eq!(
            bits(
                expected
                    .variance
                    .as_ref()
                    .expect("a σ-clipped mean emits a variance plane")
                    .channel(channel)
                    .pixels()
            ),
            bits(actual.variance.as_ref().unwrap().channel(channel).pixels()),
            "{label}: variance channel {channel}"
        );
    }
    assert_eq!(
        bits(expected.coverage.as_ref().unwrap().to_plane().pixels()),
        bits(actual.coverage.as_ref().unwrap().to_plane().pixels()),
        "{label}: coverage"
    );
    assert_eq!(
        expected.image.metadata.exposure_time, actual.image.metadata.exposure_time,
        "{label}: the master inherits another frame's metadata"
    );
}

#[cfg(feature = "real-data")]
#[test]
fn calibrate_align_stack_runs_end_to_end_on_real_lights() {
    use crate::internals::real_data::{self, raw_frames};

    let masters = real_data::calibration_masters();
    let all = raw_frames("Lights");
    let lights = &all[..all.len().min(3)];
    assert!(lights.len() >= 2, "need ≥2 lights to exercise registration");
    let frame = real_data::raw_light(&lights[0]);

    let result = calibrate_align_stack(
        lights,
        &masters,
        &AlignStackConfig::default(),
        ProgressCallback::default(),
        CancelToken::never(),
    )
    .expect("calibrate_align_stack");

    // Three lights of one field a few minutes apart all register, and the stack has a light's
    // geometry and only finite samples.
    assert_eq!(result.alignment.dropped, Vec::<usize>::new());
    assert_eq!(result.alignment.registered, lights.len());
    assert_eq!(result.product.image.dimensions(), frame.dimensions());
    for channel in 0..frame.channels() {
        assert!(
            result
                .product
                .image
                .channel(channel)
                .iter()
                .all(|v| v.is_finite())
        );
    }
}

/// A combine failure reaches the caller on one path: a cancel, an empty set and a frame-store
/// failure as the pipeline's own variants, anything else as the combine's.
#[test]
fn combine_failures_arrive_on_one_path() {
    assert!(matches!(
        Error::from(StackError::Cancelled),
        Error::Cancelled
    ));
    assert!(matches!(Error::from(StackError::NoFrames), Error::NoFrames));
    assert!(matches!(
        Error::from(StackError::NoCommonCoverage),
        Error::Stack(StackError::NoCommonCoverage)
    ));
}
