use crate::combine::stack_config::{
    Combine, CombineMethod, Normalization, SmallN, StackConfig, Weighting,
};
use crate::ingest::error::AdmissionError;
use crate::ingest::ingest_config::IngestConfig;
use crate::internals::prelude::*;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use crate::calibration::calibration_masters::CalibrationMasters;
use crate::calibration::calibration_set::CalibrationSet;
use crate::calibration::cosmic_ray::cosmic_ray_config::{CosmicRayConfig, NoiseEstimation};
use crate::calibration::cosmic_ray::error::CosmicRayError;
use crate::calibration::error::CalibrationError;
use crate::combine::error::{StackConfigError, StackError};
use crate::combine::rejection::Rejection;
use crate::error::FrameDimensionMismatch;
use crate::frame::frame_stats::FrameStats;
use crate::frame_store::decode_cache::DecodeCache;
use crate::frame_store::tier::Tier;
use crate::image::cfa::cfa_type::CfaType;
use crate::internals::synthetic::fixtures::star_field;
use crate::io::fits::cfa::save_cfa_fits;
use crate::pipeline::align::register_warp_and_stack;
use crate::pipeline::align_stack_config::{AlignStackConfig, Reference};
use crate::pipeline::align_stack_result::AlignStackResult;
use crate::pipeline::detected_frame::DetectedFrame;
use crate::pipeline::error::AlignStackError;
use crate::pipeline::frame_parking::{FrameParking, StagePlan};
use crate::pipeline::frame_registration::FrameRegistration;
use crate::pipeline::pipeline_frame::PipelineFrame;
use crate::progress::progress_callback::ProgressCallback;
use crate::progress::stacking_progress::{StackingProgress, StackingStage};
use crate::registration::registration_config::RegistrationConfig;
use crate::registration::transform::warp_transform::WarpTransform;
use crate::registration::transform::{Transform, TransformModel, TransformType};
use crate::star_detection::star_detection_config::StarDetectionConfig;
use crate::star_detection::star_detection_config::extraction_config::ExtractionConfig;
use crate::star_detection::star_detector::StarDetector;
use common::TempDir;

use crate::internals::assertions::bits;
use crate::internals::cfa::{XTRANS_PATTERN, make_cfa};
use crate::internals::fits::write_fits;
use crate::io::fits::options::{FitsCubeInterpretation, FitsFloatScale};
use crate::io::load_context::LoadContext;
use fits_well::image::Image as FitsImage;

fn base_field() -> LinearImage {
    star_field(Size2us::new(256, 256), 40, 66666).image
}

/// Warp `base` by a pure translation to fake a dithered exposure.
fn shifted(base: &LinearImage, dx: f64, dy: f64) -> LinearImage {
    let t = Transform::translation(DVec2::new(dx, dy));
    WarpTransform::new(t)
        .warp(base, RegistrationConfig::default().warp)
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
    let result = config
        .align_stack_images(
            frames,
            &IngestConfig::default(),
            progress,
            CancelToken::never(),
        )
        .expect("stack");

    assert_eq!(result.alignment.reference, 0);
    assert_eq!(
        result.alignment.registered(),
        3,
        "all three frames should stack"
    );
    assert!(
        result.alignment.dropped().is_empty(),
        "dropped: {:?}",
        result.alignment.dropped()
    );
    // Each light's registration reaches the caller: the reference as itself, and each other light
    // with the warp it was resampled through, which carries the reference's grid onto the light:
    // `shifted` resamples the base through `p + shift`, so a star moves by `−shift` in the light.
    // A fit of ~30 stars centred to a few thousandths of a pixel lands within 0.05 px.
    assert!(matches!(
        result.alignment.frames[0],
        FrameRegistration::Reference
    ));
    for (frame, shift) in [(1, DVec2::new(8.0, -5.0)), (2, DVec2::new(-6.0, 7.0))] {
        let FrameRegistration::Registered {
            warp,
            inliers,
            rms_error,
        } = &result.alignment.frames[frame]
        else {
            panic!("frame {frame}: {:?}", result.alignment.frames[frame]);
        };
        assert!(*inliers >= config.registration.matching.min_matches);
        assert!(*rms_error <= config.registration.max_rms_error);
        for p in [DVec2::new(20.0, 20.0), DVec2::new(100.0, 140.0)] {
            assert!(
                warp.apply(p).distance(p - shift) <= 0.05,
                "frame {frame} at {p}"
            );
        }
    }

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
    let result = config
        .align_stack_images(
            frames,
            &IngestConfig::default(),
            ProgressCallback::default(),
            CancelToken::never(),
        )
        .expect("stack");

    assert_eq!(
        result.alignment.dropped(),
        vec![1, 3],
        "both blank frames should be dropped, in ascending index order"
    );
    assert_eq!(
        result.alignment.registered(),
        3,
        "reference + two aligned frames"
    );

    // Manual weights are one per input light; the dropped frames take theirs with them, so the
    // three survivors combine under three weights rather than failing on five.
    let weighted = AlignStackConfig {
        stack: StackConfig {
            weighting: Weighting::Manual(vec![1.0, 9.0, 2.0, 9.0, 3.0]),
            normalization: Normalization::None,
            ..StackConfig::light()
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
    let result = weighted
        .align_stack_images(
            frames,
            &IngestConfig::default(),
            ProgressCallback::default(),
            CancelToken::never(),
        )
        .expect("manual weights follow the survivors");
    assert_eq!(result.alignment.dropped(), vec![1, 3]);
    for frame in [1, 3] {
        assert!(
            matches!(
                result.alignment.frames[frame],
                FrameRegistration::Dropped(_)
            ),
            "{:?}",
            result.alignment.frames[frame]
        );
    }
}

#[test]
fn stacked_master_inherits_reference_frame_metadata() {
    // The master's metadata comes from the reference frame (the alignment anchor), not frame 0,
    // so the RAM and streaming tiers agree. With reference = index 1, frame 0 is a (warped)
    // non-reference frame whose metadata must NOT win. The exposure is the exception: the master
    // states one only where every frame shares it, and 10, 20 and 30 s do not.
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
    let result = config
        .align_stack_images(
            vec![f0, f1, f2],
            &IngestConfig::default(),
            ProgressCallback::default(),
            CancelToken::never(),
        )
        .expect("stack");

    assert_eq!(result.alignment.reference, 1);
    assert_eq!(result.product.image.metadata.exposure_time, None);
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

    let error = AlignStackConfig::default()
        .align_stack_images(
            vec![base, odd, odder],
            &IngestConfig::default(),
            ProgressCallback::default(),
            CancelToken::never(),
        )
        .unwrap_err();

    let AlignStackError::Admission(AdmissionError::DimensionMismatch(FrameDimensionMismatch {
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

    let error = config
        .align_stack_images(
            frames,
            &IngestConfig::default(),
            ProgressCallback::default(),
            CancelToken::never(),
        )
        .unwrap_err();
    assert!(
        matches!(error, AlignStackError::RegistrationConfig(_)),
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
            parking: FrameParking::new(Tier::Resident),
            warp_width: 1,
        },
        ProgressCallback::default(),
        CancelToken::never(),
    )
    .unwrap_err();
    let AlignStackError::RegistrationConfig(invalid) = error else {
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
            combine: Combine {
                method: CombineMethod::Mean(Rejection::sigma_clip(f32::NAN)),
                small_n: SmallN::median_below(5),
            },
            weighting: Weighting::Equal,
            normalization: Normalization::None,
            ..StackConfig::light()
        },
        ..Default::default()
    };

    let error = config
        .align_stack_images(
            frames,
            &IngestConfig::default(),
            ProgressCallback::default(),
            CancelToken::never(),
        )
        .unwrap_err();
    assert!(
        matches!(
            error,
            AlignStackError::Stack(StackError::Config(StackConfigError::Field(invalid)))
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
    let err = config
        .align_stack_images(
            frames,
            &IngestConfig::default(),
            ProgressCallback::default(),
            CancelToken::never(),
        )
        .unwrap_err();
    assert!(
        matches!(err, AlignStackError::AllFramesDropped { count: 2 }),
        "all non-reference frames dropped → AllFramesDropped {{ count: 2 }}, got {err:?}"
    );
}

#[test]
fn auto_reference_passes_over_a_frame_without_enough_stars() {
    let base = base_field();
    // Frame 0 is a near-blank with fewer stars than registration needs, so Auto chooses among the
    // other two, whatever its FWHM.
    let dims = base.dimensions();
    let sparse = LinearImage::from_pixels(dims, vec![0.1; dims.pixel_count()]);
    let frames = vec![sparse, base.clone(), shifted(&base, 4.0, -3.0)];

    let result = AlignStackConfig::default()
        .align_stack_images(
            frames,
            &IngestConfig::default(),
            ProgressCallback::default(),
            CancelToken::never(),
        )
        .expect("stack");
    assert_ne!(
        result.alignment.reference, 0,
        "Auto must not anchor on the near-blank frame"
    );
    assert_eq!(
        result.alignment.dropped(),
        vec![0],
        "the near-blank frame can't register"
    );
}

/// A combine error names the input light, not its place among the survivors. Of four lights the
/// third, a near-blank, fails to register, and the fourth, the field shifted and lowered by 0.5,
/// has a negative median that a multiplicative normalization cannot divide by: input 3, though it
/// is the third survivor.
#[test]
fn a_combine_error_names_the_input_light() {
    let base = base_field();
    let dims = base.dimensions();
    let blank = LinearImage::from_pixels(dims, vec![0.1; dims.pixel_count()]);
    let lowered = LinearImage::from_pixels(
        dims,
        shifted(&base, -3.0, 2.0)
            .channel(0)
            .pixels()
            .iter()
            .map(|value| value - 0.5)
            .collect(),
    );
    let frames = vec![base.clone(), shifted(&base, 4.0, -3.0), blank, lowered];
    let config = AlignStackConfig {
        reference: Reference::Index(0),
        stack: StackConfig {
            normalization: Normalization::Multiplicative,
            ..StackConfig::light()
        },
        ..AlignStackConfig::default()
    };
    let error = config
        .align_stack_images(
            frames,
            &IngestConfig::default(),
            ProgressCallback::default(),
            CancelToken::never(),
        )
        .unwrap_err();
    assert!(
        matches!(
            error,
            AlignStackError::Stack(StackError::NonPositiveMedian { index: 3, .. })
        ),
        "{error:?}"
    );
}

#[test]
fn public_input_errors() {
    let err = AlignStackConfig::default()
        .align_stack_images(
            Vec::new(),
            &IngestConfig::default(),
            ProgressCallback::default(),
            CancelToken::never(),
        )
        .unwrap_err();
    assert!(matches!(err, AlignStackError::NoFrames));

    let config = AlignStackConfig {
        detection: StarDetectionConfig {
            extraction: ExtractionConfig {
                sigma_threshold: 0.0,
                ..Default::default()
            },
            ..StarDetectionConfig::default()
        },
        ..AlignStackConfig::default()
    };
    let image = LinearImage::from_pixels(ImageDimensions::new((1, 1), 1), vec![0.0]);
    let error = config
        .align_stack_images(
            vec![image],
            &IngestConfig::default(),
            ProgressCallback::default(),
            CancelToken::never(),
        )
        .unwrap_err();
    let AlignStackError::DetectionConfig(invalid) = error else {
        panic!("expected a detection config error, got {error:?}")
    };
    assert_eq!((invalid.field, invalid.value), ("sigma_threshold", 0.0));

    // Checked at entry, before any frame is worked: a weight count that does not match the
    // lights, a cosmic-ray config the detector cannot run, and a light with a non-finite sample.
    let dims = ImageDimensions::new((4, 4), 1);
    let flat = || LinearImage::from_pixels(dims, vec![0.1; 16]);
    let run = |lights: Vec<LinearImage>, config: &AlignStackConfig| {
        config
            .align_stack_images(
                lights,
                &IngestConfig::default(),
                ProgressCallback::default(),
                CancelToken::never(),
            )
            .unwrap_err()
    };
    let manual = AlignStackConfig {
        stack: StackConfig {
            weighting: Weighting::Manual(vec![1.0]),
            normalization: Normalization::None,
            ..StackConfig::light()
        },
        ..AlignStackConfig::default()
    };
    let error = run(vec![flat(), flat()], &manual);
    assert!(
        matches!(
            error,
            AlignStackError::Stack(StackError::Config(
                StackConfigError::ManualWeightCountMismatch {
                    expected: 2,
                    actual: 1
                }
            ))
        ),
        "{error:?}"
    );

    let mut pixels = vec![0.1; 16];
    pixels[5] = f32::NAN;
    let error = run(
        vec![flat(), LinearImage::from_pixels(dims, pixels)],
        &AlignStackConfig::default(),
    );
    assert!(
        matches!(
            error,
            AlignStackError::Admission(AdmissionError::NonFiniteImageSample {
                index: 1,
                channel: 0,
                pixel: 5,
                ..
            })
        ),
        "{error:?}"
    );
}

/// Persist `image` as a single-channel (`CfaType::Mono`) Lumos CFA FITS light — the cheapest input
/// `AlignStackConfig::calibrate_align_stack_paths` accepts, since the mono demosaic is a
/// passthrough and the frame reaches detection unchanged. `date_obs` is distinct per frame so the
/// stacked master's metadata identifies which frame it was inherited from; the 10 s exposure every
/// frame shares is what the master states.
fn write_mono_cfa_light(directory: &Path, index: usize, image: &LinearImage) -> PathBuf {
    let path = directory.join(format!("light_{index}.fits"));
    let mut cfa = make_cfa(
        Size2us::new(image.width(), image.height()),
        image.channel(0).pixels().to_vec(),
        CfaType::Mono,
    );
    cfa.metadata.date_obs = Some(format!("2026-01-01T00:00:{index:02}"));
    cfa.metadata.exposure_time = Some(10.0);
    save_cfa_fits(&path, &cfa).expect("write synthetic CFA FITS light");
    path
}

/// The RAW front end checks each light as it decodes: a light whose dimensions differ from the
/// first light's header is refused at its own index, an invalid cosmic-ray config before any light
/// decodes, and parametric cosmic-ray noise on a light whose decoder recorded no ADC step — a float
/// FITS — names that light. So does a calibration that refuses one light: a 10 s dark that still
/// holds its bias, against a 20 s second light. A cosmic-ray pass over X-Trans lights is refused
/// from the first light's header, before any light is decoded: the second path names no file, and
/// the error is still the first light's.
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
        config
            .calibrate_align_stack_paths(
                paths,
                &CalibrationMasters::default(),
                &IngestConfig::default(),
                ProgressCallback::default(),
                CancelToken::never(),
            )
            .unwrap_err()
    };

    let error = run(&paths, &AlignStackConfig::default());
    let AlignStackError::Admission(AdmissionError::DimensionMismatch(FrameDimensionMismatch {
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
    let invalid = AlignStackConfig {
        cosmic_ray: Some(CosmicRayConfig {
            niter: 0,
            ..Default::default()
        }),
        ..AlignStackConfig::default()
    };
    let AlignStackError::CosmicRayConfig(invalid) = run(&paths, &invalid) else {
        panic!("expected a cosmic-ray config error")
    };
    assert_eq!(invalid.field, "cosmic-ray niter");

    let mut dark = make_cfa(Size2us::new(32, 32), vec![0.01; 32 * 32], CfaType::Mono);
    dark.metadata.exposure_time = Some(10.0);
    let masters = CalibrationMasters::from_images(
        CalibrationSet {
            dark: Some(dark),
            ..CalibrationSet::default()
        },
        5.0,
        &CancelToken::never(),
    )
    .unwrap();
    let mut longer = make_cfa(Size2us::new(32, 32), vec![0.1; 32 * 32], CfaType::Mono);
    longer.metadata.exposure_time = Some(20.0);
    let longer_path = scratch.path().join("light_longer.fits");
    save_cfa_fits(&longer_path, &longer).unwrap();
    let error = AlignStackConfig::default()
        .calibrate_align_stack_paths(
            &[paths[0].clone(), longer_path.clone()],
            &masters,
            &IngestConfig::default(),
            ProgressCallback::default(),
            CancelToken::never(),
        )
        .unwrap_err();
    assert!(
        matches!(
            &error,
            AlignStackError::Calibration { path, source }
                if *path == longer_path
                    && matches!(**source, CalibrationError::DarkMismatch { .. })
        ),
        "{error:?}"
    );

    let error = run(&paths[..1], &parametric);
    let AlignStackError::CosmicRay { path, source } = error else {
        panic!("expected the light without an ADC step to be named, got {error:?}");
    };
    assert_eq!(
        (path, source),
        (paths[0].clone(), CosmicRayError::UnknownAdcStep)
    );

    let xtrans = scratch.path().join("xtrans_0.fits");
    let cfa = make_cfa(
        Size2us::new(36, 36),
        vec![0.1; 36 * 36],
        CfaType::XTrans(XTRANS_PATTERN),
    );
    save_cfa_fits(&xtrans, &cfa).expect("write synthetic X-Trans light");
    let cosmic = AlignStackConfig {
        cosmic_ray: Some(CosmicRayConfig::default()),
        ..AlignStackConfig::default()
    };
    let error = run(
        &[xtrans.clone(), scratch.path().join("absent.fits")],
        &cosmic,
    );
    let AlignStackError::CosmicRay { path, source } = error else {
        panic!("expected the X-Trans light to be refused, got {error:?}");
    };
    assert_eq!((path, source), (xtrans, CosmicRayError::XTransUnsupported));
}

/// Calibrated lights on disk as RGB float FITS cubes, the configuration that reads them, and the
/// decode context it implies.
#[derive(Debug)]
struct LinearLights {
    base: LinearImage,
    paths: Vec<PathBuf>,
    config: AlignStackConfig,
    ingest: IngestConfig,
    context: LoadContext,
}

impl LinearLights {
    /// Three dithered 128 × 128 views of one star field, its channels at 1, 0.8 and 0.6 of it,
    /// written under `scratch`. Every light registers on this field; on 112 × 112 with 30 stars,
    /// most found no matching pattern.
    fn write(scratch: &TempDir) -> Self {
        let base = star_field(Size2us::new(128, 128), 40, 4242).image;
        let frames = [
            base.clone(),
            shifted(&base, 5.0, -3.0),
            shifted(&base, -4.0, 6.0),
        ];
        let (width, height) = (base.width(), base.height());
        let paths: Vec<PathBuf> = frames
            .iter()
            .enumerate()
            .map(|(index, frame)| {
                let path = scratch.join(format!("light_{index}.fits"));
                let planar: Vec<f32> = [1.0f32, 0.8, 0.6]
                    .iter()
                    .flat_map(|&gain| frame.channel(0).pixels().iter().map(move |&v| v * gain))
                    .collect();
                write_fits(
                    &path,
                    &FitsImage::new([width, height, 3], planar).unwrap(),
                    None,
                );
                path
            })
            .collect();
        let mut config = AlignStackConfig::default();
        config.registration.ransac.seed = 0x5EED_0F5E;
        let mut ingest = IngestConfig::default();
        ingest.fits.cube = FitsCubeInterpretation::Rgb;
        ingest.fits.float_scale = FitsFloatScale::Normalized;
        let context = LoadContext {
            fits: ingest.fits.clone(),
            ..LoadContext::new(CancelToken::never(), u64::MAX)
        };
        Self {
            base,
            paths,
            config,
            ingest,
            context,
        }
    }
}

/// With `keep_cache` every light of a registered stack is kept on the first run, and reads back bit
/// for bit as its source decodes, metadata and all; the kept lights edited on disk move the next
/// run, which they would not if that run decoded the sources.
#[test]
fn kept_lights_are_read_back_by_the_next_run() {
    let scratch = TempDir::new("lumos_linear_kept");
    let LinearLights {
        base,
        paths,
        config,
        mut ingest,
        context,
    } = LinearLights::write(&scratch);
    let width = base.width();
    ingest.keep_cache = true;
    ingest.cache_dir = scratch.join("kept");
    let run = || {
        config
            .align_stack_paths(
                &paths,
                &ingest,
                ProgressCallback::default(),
                CancelToken::never(),
            )
            .unwrap()
    };
    let first = run();
    let decode_cache = DecodeCache::open(&ingest.cache_dir).unwrap();
    for path in &paths {
        let kept_light: LinearImage = decode_cache.decode(path, &context).unwrap();
        let decoded = LinearImage::from_file(path, &context).unwrap();
        for channel in 0..3 {
            assert_eq!(
                bits(kept_light.channel(channel).pixels()),
                bits(decoded.channel(channel).pixels()),
                "{} channel {channel} read back",
                path.display()
            );
        }
        assert_eq!(
            format!("{:?}", kept_light.metadata),
            format!("{:?}", decoded.metadata)
        );
    }
    for path in &paths {
        decode_cache.edit_kept::<LinearImage>(path, &context, |plane| {
            // A gentle gradient, which no affine norm undoes and no rejection clips.
            for (index, sample) in plane.iter_mut().enumerate() {
                *sample += 1e-5 * (index % width) as f32;
            }
        });
    }
    assert_ne!(
        bits(first.product.image.channel(0).pixels()),
        bits(run().product.image.channel(0).pixels()),
        "the edited kept lights are read back"
    );
}

/// Calibrated lights on disk stack through `AlignStackConfig::align_stack_paths` as they do decoded
/// and held, with the found reference. The spilled two-pass run parks every light and reads back
/// every registered frame. A cosmic-ray pass, which runs on mosaics alone, is refused for linear
/// lights by both entries.
#[test]
fn calibrated_lights_on_disk_stack_as_held_ones_do_around_the_found_reference() {
    let scratch = TempDir::new("lumos_linear_paths_found");
    let lights = LinearLights::write(&scratch);
    assert_tiers_stack_as_held(&lights, &scratch, |_| Reference::Auto);

    let LinearLights {
        base,
        paths,
        config,
        ingest,
        ..
    } = lights;
    let cosmic = AlignStackConfig {
        cosmic_ray: Some(CosmicRayConfig::default()),
        ..config
    };
    assert!(matches!(
        cosmic.align_stack_paths(
            &paths,
            &ingest,
            ProgressCallback::default(),
            CancelToken::never()
        ),
        Err(AlignStackError::CosmicRayOnLinearLights)
    ));
    assert!(matches!(
        cosmic.align_stack_images(
            vec![base],
            &ingest,
            ProgressCallback::default(),
            CancelToken::never()
        ),
        Err(AlignStackError::CosmicRayOnLinearLights)
    ));
}

/// Calibrated lights on disk stack through `AlignStackConfig::align_stack_paths` as they do decoded
/// and held, with the held run's reference named. The spilled one-pass run parks no light.
#[test]
fn calibrated_lights_on_disk_stack_as_held_ones_do_around_a_named_reference() {
    let scratch = TempDir::new("lumos_linear_paths_named");
    let lights = LinearLights::write(&scratch);
    assert_tiers_stack_as_held(&lights, &scratch, Reference::Index);
}

/// The held run of `lights`, read as RGB, registers every light, and
/// `AlignStackConfig::align_stack_paths` matches it bit for bit resident and spilled, under the
/// reference `reference` makes of the held run's.
fn assert_tiers_stack_as_held(
    lights: &LinearLights,
    scratch: &TempDir,
    reference: impl Fn(usize) -> Reference,
) {
    let LinearLights {
        paths,
        config,
        ingest,
        context,
        ..
    } = lights;
    let held: Vec<LinearImage> = paths
        .iter()
        .map(|path| LinearImage::from_file(path, context).unwrap())
        .collect();
    assert_eq!(held[0].channels(), 3, "the lights are read as RGB");
    let expected = config
        .align_stack_images(
            held,
            ingest,
            ProgressCallback::default(),
            CancelToken::never(),
        )
        .unwrap();
    let registered = expected.alignment.registered();
    assert_eq!(registered, paths.len(), "every light registers");

    let reference = reference(expected.alignment.reference);
    let parked_when_spilled = match reference {
        Reference::Auto => paths.len() as u64,
        Reference::Index(_) => 0,
    };
    for (memory, cache, parked, spilled) in [
        (u64::MAX, "ram", 0, 0),
        (1, "spill", parked_when_spilled, registered as u64),
    ] {
        let config = AlignStackConfig {
            reference,
            ..config.clone()
        };
        let ingest = IngestConfig {
            memory_override: Some(memory),
            cache_dir: scratch.join(cache),
            ..ingest.clone()
        };
        let result = config
            .align_stack_paths(
                paths,
                &ingest,
                ProgressCallback::default(),
                CancelToken::never(),
            )
            .unwrap();
        let report = &result.product.report;
        assert_eq!(
            (report.parked_lights, report.spilled_frames),
            (parked, spilled),
            "{cache}"
        );
        assert_same_stack(&expected, &result, cache);
    }
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
    config.registration.ransac.seed = 0x5EED_0F5E;

    let (raw_reports, raw_progress) = recorder();
    let paths: Vec<PathBuf> = frames
        .iter()
        .enumerate()
        .map(|(index, frame)| write_mono_cfa_light(scratch.path(), index, frame))
        .collect();
    config
        .calibrate_align_stack_paths(
            &paths,
            &CalibrationMasters::default(),
            &IngestConfig::default(),
            raw_progress,
            CancelToken::never(),
        )
        .expect("raw-path stack");

    let (decoded_reports, decoded_progress) = recorder();
    config
        .align_stack_images(
            frames.to_vec(),
            &IngestConfig::default(),
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
/// [`Tier`] decides only where a frame lives: a resident frame moves out of `PipelineFrame`, a
/// spilled one is read back from its memory map, and the combined result has to be bit-identical
/// either way. A named reference takes each light through one pass, and a found one through two,
/// and they too agree.
///
/// Both runs read the same mono-CFA FITS lights and differ only in `memory_override`, the input
/// `RunPlan::of` keys its tier decision on. RANSAC is seeded, removing the pipeline's only
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
    config.registration.ransac.seed = 0x5EED_0F5E;

    let ram_ingest = IngestConfig {
        memory_override: Some(u64::MAX),
        ..IngestConfig::with_cache_dir(scratch.join("ram_cache"))
    };
    let streaming_ingest = IngestConfig {
        memory_override: Some(1),
        ..IngestConfig::with_cache_dir(scratch.join("streaming_cache"))
    };

    let masters = CalibrationMasters::default();
    let run = |config: &AlignStackConfig, ingest: &IngestConfig| {
        config
            .calibrate_align_stack_paths(
                &paths,
                &masters,
                ingest,
                ProgressCallback::default(),
                CancelToken::never(),
            )
            .expect("stack the lights")
    };
    let ram = run(&config, &ram_ingest);
    let streaming = run(&config, &streaming_ingest);

    // Premise: the two budgets must straddle the tier boundary. The report says which frames went
    // to disk, which is what proves this test exercised two code paths rather than the same one
    // twice: the streaming run parks every light before the reference is known, then reads every
    // registered frame back; the RAM run writes nothing. Neither leaves a file in its cache root.
    let report = |result: &AlignStackResult| {
        (
            result.product.report.parked_lights,
            result.product.report.spilled_frames,
        )
    };
    assert_eq!(report(&ram), (0, 0), "RAM tier spilled to disk");
    assert_eq!(
        report(&streaming),
        (paths.len() as u64, ram.alignment.registered() as u64),
        "streaming tier did not park every light and read back every registered frame"
    );
    for ingest in [&ram_ingest, &streaming_ingest] {
        let root = &ingest.cache_dir;
        assert!(
            !root.exists() || std::fs::read_dir(root).unwrap().next().is_none(),
            "{} kept a file",
            root.display()
        );
    }
    assert_eq!(
        ram.alignment.dropped(),
        vec![2, 5],
        "the two starless frames should drop, in ascending index order"
    );
    assert_eq!(
        ram.alignment.registered(),
        5,
        "every dithered frame should register against the reference"
    );
    // The master inherits the reference frame's metadata. Distinct per-frame observation times make
    // the comparison below non-vacuous; the shared exposure is the stack's own.
    assert!(
        ram.product.image.metadata.date_obs.is_some(),
        "per-frame observation time did not survive the FITS round-trip; \
         the metadata comparison would be vacuous"
    );
    assert_eq!(ram.product.image.metadata.exposure_time, Some(10.0));

    // Naming the reference the automatic choice found takes each light through one pass, on both
    // tiers, and the stack is the same. The spilled one-pass run writes each light once, warped:
    // no calibrated light is parked before its registration, as the two-pass run parks every one.
    let one_pass = AlignStackConfig {
        reference: Reference::Index(ram.alignment.reference),
        ..config
    };
    let ram_one_pass = run(&one_pass, &ram_ingest);
    let streaming_one_pass = run(&one_pass, &streaming_ingest);
    assert_eq!(
        report(&streaming_one_pass),
        (0, ram.alignment.registered() as u64)
    );

    for (other, label) in [
        (&streaming, "streaming"),
        (&ram_one_pass, "RAM one-pass"),
        (&streaming_one_pass, "streaming one-pass"),
    ] {
        assert_same_stack(&ram, other, label);
    }
}

/// Two runs of the default configuration on the same lights give the same stack, bit for bit:
/// the reference choice, every registration and every plane. RANSAC's seed is fixed, and nothing
/// downstream depends on the order threads finish in.
#[test]
fn two_default_runs_stack_bit_for_bit() {
    let base = base_field();
    let frames = || {
        vec![
            shifted(&base, 3.0, -2.0),
            base.clone(),
            shifted(&base, -5.0, 4.0),
            shifted(&base, 1.5, 6.5),
        ]
    };
    let run = || {
        AlignStackConfig::default()
            .align_stack_images(
                frames(),
                &IngestConfig::default(),
                ProgressCallback::default(),
                CancelToken::never(),
            )
            .expect("stack")
    };
    assert_same_stack(&run(), &run(), "second run");
}

/// Two runs stack the same lights against the same reference, bit for bit: every plane, the
/// alignment, the detection funnels and the inherited metadata.
fn assert_same_stack(expected: &AlignStackResult, actual: &AlignStackResult, label: &str) {
    assert_eq!(
        expected.alignment.reference, actual.alignment.reference,
        "{label}"
    );
    // `Debug` prints every float in its shortest round-trip form, so equal text is equal bits.
    assert_eq!(
        format!("{:?}", expected.alignment.frames),
        format!("{:?}", actual.alignment.frames),
        "{label}"
    );
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
        // A plane the combine emits for one run it emits for the other, bit for bit.
        assert_eq!(
            expected
                .weight
                .as_ref()
                .map(|weight| bits(weight.channel(channel))),
            actual
                .weight
                .as_ref()
                .map(|weight| bits(weight.channel(channel))),
            "{label}: weight channel {channel}"
        );
        assert_eq!(
            expected
                .inverse_variance
                .as_ref()
                .map(|variance| bits(variance.channel(channel).pixels())),
            actual
                .inverse_variance
                .as_ref()
                .map(|variance| bits(variance.channel(channel).pixels())),
            "{label}: variance channel {channel}"
        );
    }
    assert_eq!(
        expected
            .coverage
            .as_ref()
            .map(|coverage| bits(coverage.to_plane(0).pixels())),
        actual
            .coverage
            .as_ref()
            .map(|coverage| bits(coverage.to_plane(0).pixels())),
        "{label}: coverage"
    );
    assert_eq!(
        expected.image.metadata.date_obs, actual.image.metadata.date_obs,
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

    let result = AlignStackConfig::default()
        .calibrate_align_stack_paths(
            lights,
            &masters,
            &IngestConfig::default(),
            ProgressCallback::default(),
            CancelToken::never(),
        )
        .expect("calibrate_align_stack_paths");

    // Three lights of one field a few minutes apart all register, and the stack has a light's
    // geometry and only finite samples.
    assert_eq!(result.alignment.dropped(), Vec::<usize>::new());
    assert_eq!(result.alignment.registered(), lights.len());
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
        AlignStackError::from(StackError::Cancelled),
        AlignStackError::Cancelled
    ));
    assert!(matches!(
        AlignStackError::from(StackError::NoFrames),
        AlignStackError::NoFrames
    ));
    assert!(matches!(
        AlignStackError::from(StackError::NoCommonCoverage),
        AlignStackError::Stack(StackError::NoCommonCoverage)
    ));
}
