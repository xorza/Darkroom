use common::CancelToken;
use glam::DVec2;

use crate::io::image::{ImageDimensions, LinearImage};
use crate::stacking::pipeline::align::align_and_stack;
use crate::stacking::pipeline::config::{AlignStackConfig, Reference};
use crate::stacking::pipeline::result::Error;
use crate::stacking::registration::config::Config as RegistrationConfig;
use crate::stacking::registration::resample::warp;
use crate::stacking::registration::transform::{Transform, WarpTransform};
use crate::stacking::star_detection::config::Config as StarDetectionConfig;
use crate::stacking::star_detection::detector::StarDetector;
use crate::stacking::star_detection::error::StarDetectionConfigError;
use crate::testing::synthetic::fixtures::star_field;

#[derive(Debug)]
struct BaseField {
    image: LinearImage,
    registration: RegistrationConfig,
}

fn base_field() -> BaseField {
    BaseField {
        image: star_field(256, 256, 40, 66666).image,
        registration: RegistrationConfig::default(),
    }
}

/// Warp `base` by a pure translation to fake a dithered exposure.
fn shifted(base: &LinearImage, reg: &RegistrationConfig, dx: f64, dy: f64) -> LinearImage {
    let t = Transform::translation(DVec2::new(dx, dy));
    warp(base, &WarpTransform::new(t), &reg.warp).image
}

#[test]
fn aligns_shifted_frames_into_a_sharp_stack() {
    let BaseField {
        image: base,
        registration: reg,
    } = base_field();
    let frames = vec![
        base.clone(),
        shifted(&base, &reg, 8.0, -5.0),
        shifted(&base, &reg, -6.0, 7.0),
    ];

    let config = AlignStackConfig {
        reference: Reference::Index(0),
        ..Default::default()
    };
    let result = align_and_stack(frames, &config, CancelToken::never()).expect("stack");

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
    let BaseField {
        image: base,
        registration: reg,
    } = base_field();
    let dims = base.dimensions;
    // A flat frame has no stars → registration fails → it is dropped, not fatal.
    let blank = LinearImage::from_pixels(dims, vec![0.1; dims.pixel_count()]);
    let frames = vec![base.clone(), shifted(&base, &reg, 5.0, 3.0), blank];

    let config = AlignStackConfig {
        reference: Reference::Index(0),
        ..Default::default()
    };
    let result = align_and_stack(frames, &config, CancelToken::never()).expect("stack");

    assert_eq!(
        result.alignment.dropped,
        vec![2],
        "blank frame should be dropped"
    );
    assert_eq!(
        result.alignment.registered, 2,
        "reference + one aligned frame"
    );
}

#[test]
fn stacked_master_inherits_reference_frame_metadata() {
    // The master's metadata comes from the reference frame (the alignment anchor), not frame 0,
    // so the RAM and streaming tiers agree. With reference = index 1, frame 0 is a (warped)
    // non-reference frame whose metadata must NOT win.
    let BaseField {
        image: base,
        registration: reg,
    } = base_field();
    let mut f0 = shifted(&base, &reg, 5.0, 3.0);
    let mut f1 = base.clone(); // the reference (index 1)
    let mut f2 = shifted(&base, &reg, -4.0, 6.0);
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
    let result = align_and_stack(vec![f0, f1, f2], &config, CancelToken::never()).expect("stack");

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
fn all_non_reference_frames_dropped_errors() {
    // With the reference produced in-place (it survives in `frames`), "nothing aligned" means
    // only the reference remains — guard the changed `frames.len() <= 1` condition.
    let BaseField { image: base, .. } = base_field();
    let dims = base.dimensions;
    let blank = || LinearImage::from_pixels(dims, vec![0.1; dims.pixel_count()]);
    // Reference has stars; both others are blank → both fail to register → nothing aligns.
    let frames = vec![base, blank(), blank()];

    let config = AlignStackConfig {
        reference: Reference::Index(0),
        ..Default::default()
    };
    let err = align_and_stack(frames, &config, CancelToken::never()).unwrap_err();
    assert!(
        matches!(err, Error::AllFramesDropped { count: 2 }),
        "all non-reference frames dropped → AllFramesDropped {{ count: 2 }}, got {err:?}"
    );
}

#[test]
fn auto_reference_picks_the_richest_frame() {
    let BaseField {
        image: base,
        registration: reg,
    } = base_field();
    // Frame 1 (full field) has far more stars than frame 0 (a near-blank), so Auto must
    // anchor on frame 1.
    let dims = base.dimensions;
    let sparse = LinearImage::from_pixels(dims, vec![0.1; dims.pixel_count()]);
    let frames = vec![sparse, base.clone(), shifted(&base, &reg, 4.0, -3.0)];

    let result =
        align_and_stack(frames, &AlignStackConfig::default(), CancelToken::never()).expect("stack");
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
        CancelToken::never(),
    )
    .unwrap_err();
    assert!(matches!(err, Error::NoFrames));

    let config = AlignStackConfig {
        detection: StarDetectionConfig {
            detection: crate::stacking::star_detection::config::DetectionConfig {
                sigma_threshold: 0.0,
                ..Default::default()
            },
            ..StarDetectionConfig::default()
        },
        ..AlignStackConfig::default()
    };
    let image = LinearImage::from_pixels(ImageDimensions::new((1, 1), 1), vec![0.0]);
    let error = align_and_stack(vec![image], &config, CancelToken::never()).unwrap_err();
    assert!(matches!(
        error,
        Error::DetectionConfig(StarDetectionConfigError::InvalidSigmaThreshold { value: 0.0 })
    ));
}

#[cfg(feature = "real-data")]
#[test]
#[ignore = "real-data integration test; run explicitly with --ignored"]
fn calibrate_align_stack_runs_end_to_end_on_real_lights() {
    use crate::stacking::calibration_masters::CalibrationMasters;
    use crate::stacking::pipeline::streaming::calibrate_align_stack;
    use crate::testing::calibration_image_paths;
    use crate::{CalibrationSet, DEFAULT_SIGMA_THRESHOLD};

    let dark_paths = calibration_image_paths("Darks").unwrap_or_default();
    let bias_paths = calibration_image_paths("Bias").unwrap_or_default();
    let flat_paths = calibration_image_paths("Flats").unwrap_or_default();
    let empty: Vec<std::path::PathBuf> = Vec::new();
    let masters = CalibrationMasters::from_files(
        CalibrationSet {
            dark: &dark_paths,
            flat: &flat_paths,
            bias: &bias_paths,
            flat_dark: &empty,
        },
        DEFAULT_SIGMA_THRESHOLD,
    )
    .expect("build calibration masters");

    let all = calibration_image_paths("Lights").expect("Lights subdirectory");
    let lights = &all[..all.len().min(3)];
    assert!(lights.len() >= 2, "need ≥2 lights to exercise registration");

    let result = calibrate_align_stack(
        lights,
        &masters,
        &AlignStackConfig::default(),
        CancelToken::never(),
    )
    .expect("calibrate_align_stack");

    // A real stacked image came out, and every input frame is accounted for.
    assert!(result.product.image.width() > 0 && result.product.image.height() > 0);
    assert_eq!(
        result.alignment.registered + result.alignment.dropped.len(),
        lights.len()
    );
    assert!(
        result.alignment.registered >= 1,
        "at least the reference is stacked"
    );
}

#[cfg(feature = "real-data")]
#[test]
#[ignore = "real-data integration test; run explicitly with --ignored"]
fn streaming_disk_tier_matches_ram_on_real_lights() {
    use crate::stacking::calibration_masters::CalibrationMasters;
    use crate::stacking::pipeline::streaming::calibrate_align_stack;
    use crate::testing::calibration_image_paths;
    use crate::{CalibrationSet, DEFAULT_SIGMA_THRESHOLD};

    let dark_paths = calibration_image_paths("Darks").unwrap_or_default();
    let bias_paths = calibration_image_paths("Bias").unwrap_or_default();
    let flat_paths = calibration_image_paths("Flats").unwrap_or_default();
    let empty: Vec<std::path::PathBuf> = Vec::new();
    let masters = CalibrationMasters::from_files(
        CalibrationSet {
            dark: &dark_paths,
            flat: &flat_paths,
            bias: &bias_paths,
            flat_dark: &empty,
        },
        DEFAULT_SIGMA_THRESHOLD,
    )
    .expect("build calibration masters");

    let all = calibration_image_paths("Lights").expect("Lights subdirectory");
    let lights = &all[..all.len().min(3)];
    assert!(lights.len() >= 2, "need ≥2 lights to exercise registration");

    // Seed RANSAC so both tiers are bit-comparable (registration is the only nondeterminism).
    let mut config = AlignStackConfig::default();
    config.registration.ransac.seed = Some(0x00C0_FFEE);

    // RAM tier: huge memory budget → the all-in-memory path.
    let mut ram_cfg = config.clone();
    ram_cfg.stack.cache.available_memory = Some(u64::MAX);
    let ram = calibrate_align_stack(lights, &masters, &ram_cfg, CancelToken::never())
        .expect("RAM-tier stack");

    // Disk tier: a 1-byte budget forces the streaming disk path; clean its cache on drop.
    let mut disk_cfg = config;
    disk_cfg.stack.cache.available_memory = Some(1);
    disk_cfg.stack.cache.keep_cache = false;
    let disk = calibrate_align_stack(lights, &masters, &disk_cfg, CancelToken::never())
        .expect("disk-tier (streaming) stack");

    assert_eq!(
        ram.alignment.registered, disk.alignment.registered,
        "same frames stacked"
    );
    assert_eq!(
        ram.alignment.dropped, disk.alignment.dropped,
        "same frames dropped"
    );
    assert_eq!(
        ram.alignment.reference, disk.alignment.reference,
        "same reference"
    );
    assert_eq!(
        ram.product.image.dimensions(),
        disk.product.image.dimensions()
    );
    // Bit-identical: same frames, same (seeded) registration, same combine — only the frame
    // storage (RAM vs mmap) differs.
    for c in 0..ram.product.image.channels() {
        let a: Vec<u32> = ram
            .product
            .image
            .channel(c)
            .pixels()
            .iter()
            .map(|x| x.to_bits())
            .collect();
        let b: Vec<u32> = disk
            .product
            .image
            .channel(c)
            .pixels()
            .iter()
            .map(|x| x.to_bits())
            .collect();
        assert_eq!(a, b, "channel {c} differs between the RAM and disk tiers");
    }
}
