use std::path::{Path, PathBuf};

use common::CancelToken;
use glam::DVec2;
use imaginarium::Buffer2;

use crate::io::image::ImageDimensions;
use crate::io::image::cfa::CfaType;
use crate::io::image::fits::cfa::save_cfa_fits;
use crate::io::image::linear::LinearImage;
use crate::stacking::calibration_masters::CalibrationMasters;
use crate::stacking::combine::config::{CombineMethod, StackConfig};
use crate::stacking::combine::error::{Error as StackError, StackConfigError};
use crate::stacking::combine::rejection::Rejection;
use crate::stacking::pipeline::align::align_and_stack;
use crate::stacking::pipeline::calibrate::calibrate_align_stack;
use crate::stacking::pipeline::config::{AlignStackConfig, Reference};
use crate::stacking::pipeline::result::Error;
use crate::stacking::registration::config::Config as RegistrationConfig;
use crate::stacking::registration::resample::warp;
use crate::stacking::registration::transform::{Transform, TransformType, WarpTransform};
use crate::stacking::star_detection::config::Config as StarDetectionConfig;
use crate::stacking::star_detection::detector::StarDetector;
use crate::stacking::star_detection::error::StarDetectionConfigError;
use crate::testing::synthetic::fixtures::star_field;
use crate::testing::{ScratchDirectory, make_cfa};

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
    let dims = base.dimensions();
    // A flat frame has no stars → registration fails → it is dropped, not fatal. Two of them, at
    // non-adjacent indices, so `dropped` also pins its documented ascending order — no sort
    // produces that, only rayon's order-preserving indexed `collect`.
    let blank = || LinearImage::from_pixels(dims, vec![0.1; dims.pixel_count()]);
    let frames = vec![
        base.clone(),
        blank(),
        shifted(&base, &reg, 5.0, 3.0),
        blank(),
        shifted(&base, &reg, -4.0, 6.0),
    ];

    let config = AlignStackConfig {
        reference: Reference::Index(0),
        ..Default::default()
    };
    let result = align_and_stack(frames, &config, CancelToken::never()).expect("stack");

    assert_eq!(
        result.alignment.dropped,
        vec![1, 3],
        "both blank frames should be dropped, in ascending index order"
    );
    assert_eq!(
        result.alignment.registered, 3,
        "reference + two aligned frames"
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
fn mismatched_frame_dimensions_are_rejected_before_registration() {
    // `warp` reprojects into the source frame's own grid, so a frame from a different sensor
    // would reach the combine as a differently-sized plane rather than as an error. The guard
    // sits ahead of registration; frame 1 is the first mismatch even though frame 2 also differs.
    let BaseField { image: base, .. } = base_field();
    let odd = LinearImage::from_pixels(ImageDimensions::new((128, 128), 1), vec![0.1; 128 * 128]);
    let odder = LinearImage::from_pixels(ImageDimensions::new((64, 64), 1), vec![0.1; 64 * 64]);

    let error = align_and_stack(
        vec![base, odd, odder],
        &AlignStackConfig::default(),
        CancelToken::never(),
    )
    .unwrap_err();

    let Error::Stack(StackError::DimensionMismatch {
        index,
        expected,
        actual,
    }) = error
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
    let BaseField {
        image: base,
        registration: reg,
    } = base_field();
    let frames = vec![
        base.clone(),
        shifted(&base, &reg, 5.0, 3.0),
        shifted(&base, &reg, -4.0, 6.0),
    ];

    let mut config = AlignStackConfig {
        reference: Reference::Index(0),
        ..Default::default()
    };
    // Homography needs four points, so a three-match floor can never be satisfied.
    config.registration.transform_type = TransformType::Homography;
    config.registration.matching.min_matches = 3;
    assert!(
        config.registration.validate().is_err(),
        "premise: this registration config must be invalid"
    );

    let error = align_and_stack(frames, &config, CancelToken::never()).unwrap_err();
    assert!(
        matches!(error, Error::RegistrationConfig(_)),
        "expected the config to be blamed, got {error:?}"
    );
}

#[test]
fn an_invalid_stack_config_is_caught_before_the_frames_are_worked() {
    // The combine validates its own config, but only after every frame has been decoded,
    // detected, registered and warped — so a run whose frames also fail to register would
    // report `AllFramesDropped` and never mention the config at all. Validating up front means
    // the config is blamed, and nothing upstream is paid for.
    let BaseField { image: base, .. } = base_field();
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

    let error = align_and_stack(frames, &config, CancelToken::never()).unwrap_err();
    assert!(
        matches!(
            error,
            Error::Stack(StackError::Config(StackConfigError::InvalidSigmaLow { .. }))
        ),
        "expected the stack config to be blamed rather than the frames, got {error:?}"
    );
}

#[test]
fn all_non_reference_frames_dropped_errors() {
    // With the reference produced in-place (it survives in `frames`), "nothing aligned" means
    // only the reference remains — guard the changed `frames.len() <= 1` condition.
    let BaseField { image: base, .. } = base_field();
    let dims = base.dimensions();
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
    let dims = base.dimensions();
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

fn bits(buffer: &Buffer2<f32>) -> Vec<u32> {
    buffer
        .pixels()
        .iter()
        .map(|value| value.to_bits())
        .collect()
}

/// Persist `image` as a single-channel (`CfaType::Mono`) Lumos CFA FITS light — the cheapest
/// input `calibrate_align_stack` accepts, since the mono demosaic is a passthrough and the frame
/// reaches detection unchanged. `exposure_time` is distinct per frame so the stacked master's
/// metadata identifies which frame it was inherited from.
fn write_mono_cfa_light(directory: &Path, index: usize, image: &LinearImage) -> PathBuf {
    let path = directory.join(format!("light_{index}.fits"));
    let mut cfa = make_cfa(
        image.width(),
        image.height(),
        image.channel(0).pixels().to_vec(),
        CfaType::Mono,
    );
    cfa.metadata.exposure_time = Some(10.0 + index as f64);
    save_cfa_fits(&path, &cfa).expect("write synthetic CFA FITS light");
    path
}

/// The all-RAM and memory-bounded runs must produce the same stack.
///
/// Both go through one body now ([`super::align::register_warp_and_stack`]), so this no longer
/// guards two transcriptions against drift — it guards the claim that
/// [`super::tier::FrameTier`] only decides *where* a frame lives. A resident frame moves out of
/// `PipelineFrame`, a spilled one is read back from its memory map, and the combined result has
/// to be bit-identical either way.
///
/// The always-run counterpart to `streaming_disk_tier_matches_ram_on_real_lights`, which is
/// gated behind `real-data` *and* `#[ignore]` *and* a dataset on disk, so it never runs in the
/// verification chain.
///
/// Both runs read the same mono-CFA FITS lights and differ only in `available_memory`, the input
/// `plan_memory` keys its tier decision on. RANSAC is seeded, removing the pipeline's only other
/// source of nondeterminism, so any difference the assertions find is a real divergence.
#[test]
fn ram_and_streaming_tiers_produce_identical_stacks() {
    let scratch = ScratchDirectory::new("lumos_tier_equivalence");
    let BaseField {
        image: base,
        registration: reg,
    } = base_field();

    // Five dithered exposures: five clears `StackConfig`'s default `SmallN::median_below(5)`, so
    // the σ-clipped mean actually runs and the combine emits a linear-variance plane — without
    // that the comparison would silently skip one of the four output planes. Two starless frames
    // at non-adjacent indices fail registration on both tiers, so the drop bookkeeping and its
    // ascending order are compared too.
    let dims = base.dimensions();
    let blank = || LinearImage::from_pixels(dims, vec![0.1; dims.pixel_count()]);
    let frames = [
        base.clone(),
        shifted(&base, &reg, 6.0, -4.0),
        blank(),
        shifted(&base, &reg, -5.0, 7.0),
        shifted(&base, &reg, 3.0, 9.0),
        blank(),
        shifted(&base, &reg, -8.0, -2.0),
    ];
    let paths: Vec<PathBuf> = frames
        .iter()
        .enumerate()
        .map(|(index, frame)| write_mono_cfa_light(&scratch, index, frame))
        .collect();

    let mut config = AlignStackConfig::default();
    config.registration.ransac.seed = Some(0x5EED_0F5E);

    let mut ram_config = config.clone();
    ram_config.stack.cache.available_memory = Some(u64::MAX);
    ram_config.stack.cache.cache_dir = scratch.join("ram_cache");

    let mut streaming_config = config;
    streaming_config.stack.cache.available_memory = Some(1);
    streaming_config.stack.cache.cache_dir = scratch.join("streaming_cache");
    // Kept so the premise assertion below can observe that the spill tier really ran; the whole
    // scratch tree goes away when `scratch` drops.
    streaming_config.stack.cache.keep_cache = true;

    let masters = CalibrationMasters::default();
    let ram = calibrate_align_stack(&paths, &masters, &ram_config, CancelToken::never())
        .expect("RAM-tier stack");
    let streaming =
        calibrate_align_stack(&paths, &masters, &streaming_config, CancelToken::never())
            .expect("streaming-tier stack");

    // Premise: the two budgets must straddle the tier boundary. Only the streaming path creates a
    // spill directory, so its presence — and the RAM path's lack of one — is what proves this test
    // exercised two code paths rather than the same one twice.
    assert!(
        streaming_config.stack.cache.cache_dir.is_dir(),
        "streaming tier never spilled; both runs took the RAM path"
    );
    assert!(
        !ram_config.stack.cache.cache_dir.exists(),
        "RAM tier spilled to disk; both runs took the streaming path"
    );

    assert_eq!(ram.alignment.reference, streaming.alignment.reference);
    assert_eq!(ram.alignment.registered, streaming.alignment.registered);
    assert_eq!(ram.alignment.dropped, streaming.alignment.dropped);
    assert_eq!(
        ram.alignment.dropped,
        vec![2, 5],
        "the two starless frames should drop, in ascending index order"
    );
    assert_eq!(
        ram.alignment.registered, 5,
        "every dithered frame should register against the reference"
    );

    assert_eq!(
        ram.product.image.dimensions(),
        streaming.product.image.dimensions()
    );
    let channels = ram.product.image.channels();
    for channel in 0..channels {
        assert_eq!(
            bits(ram.product.image.channel(channel)),
            bits(streaming.product.image.channel(channel)),
            "image channel {channel} differs between the RAM and streaming tiers"
        );
        assert_eq!(
            bits(ram.product.weight.as_ref().unwrap().channel(channel)),
            bits(streaming.product.weight.as_ref().unwrap().channel(channel)),
            "weight channel {channel} differs between the RAM and streaming tiers"
        );
    }
    assert_eq!(
        bits(ram.product.coverage.as_ref().unwrap()),
        bits(streaming.product.coverage.as_ref().unwrap()),
        "coverage differs between the RAM and streaming tiers"
    );

    let ram_variance = ram
        .product
        .linear_variance
        .as_ref()
        .expect("a σ-clipped mean emits a linear-variance plane");
    let streaming_variance = streaming
        .product
        .linear_variance
        .as_ref()
        .expect("a σ-clipped mean emits a linear-variance plane");
    for channel in 0..channels {
        assert_eq!(
            bits(ram_variance.channel(channel)),
            bits(streaming_variance.channel(channel)),
            "linear-variance channel {channel} differs between the RAM and streaming tiers"
        );
    }

    // The master inherits the reference frame's metadata, and the two tiers reach that by
    // different routes — the RAM path overwrites it after combining, the streaming path threads it
    // in. Distinct per-frame exposure times make the comparison non-vacuous.
    let inherited = ram.product.image.metadata.exposure_time;
    assert!(
        inherited.is_some(),
        "per-frame exposure time did not survive the FITS round-trip; \
         the metadata comparison below would be vacuous"
    );
    assert_eq!(
        inherited, streaming.product.image.metadata.exposure_time,
        "master metadata came from a different frame on each tier"
    );
}

#[cfg(feature = "real-data")]
#[test]
#[ignore = "real-data integration test; run explicitly with --ignored"]
fn calibrate_align_stack_runs_end_to_end_on_real_lights() {
    use crate::stacking::calibration_masters::CalibrationMasters;
    use crate::stacking::pipeline::calibrate::calibrate_align_stack;
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
        CancelToken::never(),
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
    use crate::stacking::pipeline::calibrate::calibrate_align_stack;
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
        CancelToken::never(),
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
