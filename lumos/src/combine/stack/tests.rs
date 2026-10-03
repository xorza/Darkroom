use crate::frame_store::frame_facts::FrameFacts;
use crate::internals::prelude::*;
use arrayvec::ArrayVec;

use crate::frame_store::frame_quality::FramePlane;
use crate::frame_store::frame_spill::FrameSpill;

use crate::combine::cache_config::CacheConfig;
use crate::combine::config::{Normalization, SmallN};
use crate::combine::normalization::ChannelNorm;
use crate::combine::rejection::percentile_clip_config::PercentileClipConfig;
use crate::combine::stack::*;
use crate::error::FrameDimensionMismatch;
use crate::frame_store::spill_directory::SpillDirectory;
use crate::internals;
use crate::internals::assertions::bits;
use crate::internals::synthetic::patterns;
use crate::internals::synthetic::sky_field::{Sky, SkyField};
use crate::io::image::cfa::{CfaImage, CfaType};
use crate::io::image::fits::provenance::{
    FitsChecksumProvenance, FitsChecksumState, FitsHduProvenance, FitsTransferProvenance,
};
use crate::io::image::image_provenance::{
    ColorProvenance, DecoderProvenance, DemosaicProvenance, ImageProvenance, RowOrder,
    SourceContainer, TransferProvenance,
};
use crate::io::image::null_mask::NullMask;
use crate::io::image::sample_domain::ScaleOrigin;
use crate::math::statistics::MedianMad;
use crate::registration::config::{self, InterpolationMethod};
use crate::registration::resample;
use crate::registration::transform::{Transform, WarpTransform};
use crate::stack_product::quality_map::QualityMap;
use common::TempDir;
use std::path::PathBuf;

/// The combine, with no progress reported and no cancel.
fn combine(frames: Vec<StackFrame>, config: &StackConfig) -> Result<StackProduct, Error> {
    stack_images(
        frames,
        config,
        ProgressCallback::default(),
        CancelToken::never(),
    )
}

fn stack_frame(image: LinearImage, quality: FrameQuality<Buffer2<f32>>) -> StackFrame {
    let mut frame = StackFrame::from(image);
    frame.quality = quality;
    frame
}

fn make_cfa_stack_cache(
    frame_pixels: Vec<Vec<f32>>,
    source_sigmas: &[f32],
    dimensions: ImageDimensions,
    normalization: Normalization,
) -> FrameCache {
    assert_eq!(frame_pixels.len(), source_sigmas.len());
    let images: Vec<CfaImage> = frame_pixels
        .into_iter()
        .zip(source_sigmas)
        .map(|(pixels, &sigma)| {
            let mut image = internals::cfa::make_cfa(
                Size2us::new(dimensions.width(), dimensions.height()),
                pixels,
                CfaType::Mono,
            );
            image.quantization_sigma = Some(sigma);
            image
        })
        .collect();
    FrameCache::from_images(images, normalization)
}

#[test]
fn cfa_stack_quantization_uses_normalization_and_actual_rejection_survivors() {
    let dimensions = ImageDimensions::new((2, 1), 1);
    // Flat frames, so each frame's measured median *is* its pixel value and its MAD is zero.
    // Multiplicative normalization keys on the medians (0.4 / 0.2 → frame 1 gains 2.0) and the
    // manual weights bypass MAD entirely, so the measured statistics are exactly what this
    // assertion needs.
    let normalized_cache = make_cfa_stack_cache(
        vec![vec![0.4; 2], vec![0.2; 2]],
        &[0.01, 0.02],
        dimensions,
        Normalization::Multiplicative,
    );
    let normalized = run_stacking(
        &normalized_cache,
        &StackConfig {
            method: CombineMethod::Mean(Rejection::None),
            weighting: Weighting::Manual(vec![0.25, 0.75]),
            normalization: Normalization::Multiplicative,
            small_n: SmallN::none(),
            ..Default::default()
        },
    )
    .expect("this cache is never cancelled");
    #[expect(
        clippy::imprecise_flops,
        reason = "the expected value repeats the N-term sum of squares `quantization` computes, so the comparison stays exact"
    )]
    let expected_normalized_sigma =
        ((0.25f32 * 0.01).powi(2) + (0.75f32 * 2.0 * 0.02).powi(2)).sqrt();
    assert_eq!(normalized.image.channel(0).pixels().to_vec(), vec![0.4; 2]);
    assert_eq!(
        normalized.quantization_sigma,
        Some(expected_normalized_sigma),
        "weighted normalized σ must use each frame's own source σ and gain"
    );

    let median_cache = make_cfa_stack_cache(
        vec![vec![0.3; 2], vec![0.4; 2], vec![0.5; 2]],
        &[0.01; 3],
        dimensions,
        Normalization::None,
    );
    let median =
        run_stacking(&median_cache, &StackConfig::median()).expect("this cache is never cancelled");
    let expected_median_sigma = 0.01 * (3.0f32 / 5.0).sqrt();
    assert_eq!(median.image.channel(0).pixels().to_vec(), vec![0.4; 2]);
    assert_eq!(
        median.quantization_sigma,
        Some(expected_median_sigma),
        "an equal-source three-frame median must use the exact uniform order statistic"
    );

    let winsorized_cache = make_cfa_stack_cache(
        vec![vec![0.3; 2], vec![0.5; 2]],
        &[0.01, 0.02],
        dimensions,
        Normalization::None,
    );
    let winsorized = run_stacking(&winsorized_cache, &StackConfig::winsorized(2.5))
        .expect("this cache is never cancelled");
    assert_eq!(
        winsorized.quantization_sigma,
        Some(0.02),
        "nonlinear unequal-source combines must retain the conservative largest σ"
    );

    let rejection_cache = make_cfa_stack_cache(
        (0..8)
            .map(|frame| vec![[1.0, 2.0, 3.0, 4.0, 5.0, 6.0, 7.0, 100.0][frame], 1.0])
            .collect(),
        &[0.01; 8],
        dimensions,
        Normalization::None,
    );
    let rejected = run_stacking(
        &rejection_cache,
        &StackConfig {
            method: CombineMethod::Mean(Rejection::sigma_clip(2.0)),
            small_n: SmallN::none(),
            ..Default::default()
        },
    )
    .expect("this cache is never cancelled");
    assert_eq!(rejected.image.channel(0).pixels().to_vec(), vec![4.0, 1.0]);
    // √(7σ²)/7 is σ/√7: the square, the sum, the root and the quotient round once each, 2ε
    // relative.
    let expected_rejected_sigma = f64::from(0.01f32) / 7.0f64.sqrt();
    assert_close!(
        rejected.quantization_sigma.unwrap(),
        expected_rejected_sigma,
        2.0 * f64::from(f32::EPSILON) * expected_rejected_sigma,
        "the global CFA floor must use the least-reduced pixel's seven survivors"
    );
}

/// The load-bearing guarantee for memory-aware stacking: spilling frames to disk (mmap) and
/// combining must be **bit-identical** to the all-RAM combine — same frames, same math, only the
/// plane storage differs. Exercises σ-clip rejection + noise weighting + global norm + a partial
/// coverage map (so the coverage spill round-trips too).
#[test]
fn disk_tier_output_is_bit_identical_to_memory_tier() {
    let (w, h, n) = (40usize, 30usize, 12usize);
    let dims = ImageDimensions::new((w, h), 1);
    let make_frame = |f: usize| -> StackFrame {
        let mut rng = TestRng::new(f as u64);
        let mut px: Vec<f32> = (0..w * h)
            .map(|_| 0.2 + (f as f32) * 0.01 + (rng.next_f32() - 0.5) * 0.02)
            .collect();
        px[(f * 7) % (w * h)] = 0.95; // an outlier so rejection actually fires
        let image = LinearImage::from_planar_channels(dims, [px]);
        // Every other frame gets a partial coverage map (warped-border emulation).
        let quality = if f.is_multiple_of(2) {
            let mut coverage = Buffer2::new_filled(w, h, 1.0f32);
            coverage[0] = 0.0;
            FrameQuality::from_coverage(coverage)
        } else {
            FrameQuality::None
        };
        stack_frame(image, quality)
    };
    let config = StackConfig::light();

    // `make_frame` is deterministic, so building each tier's frames from it is what makes the two
    // sets identical — the alternative, cloning one set, has to restate that the stats came along.
    let ram = combine((0..n).map(make_frame).collect(), &config.clone()).unwrap();
    let frames: Vec<StackFrame> = (0..n).map(make_frame).collect();

    let scratch = TempDir::new("lumos_tier_test");
    let spill_directory = SpillDirectory::create(&scratch.join("cache"), false).unwrap();
    let metadata = frames[0].image.metadata.clone();
    let stored = frames
        .into_iter()
        .enumerate()
        .map(|(i, f)| {
            StoredFrame::spill(
                &FrameSpill::new(spill_directory.path(), &format!("f{i}")),
                &f.image,
                &f.quality,
                f.source_stats,
            )
            .unwrap()
        })
        .collect();
    let disk = stack_stored_frames(
        stored,
        CacheTier::of(Some(spill_directory), RunMemory::new(1 << 30, None)),
        dims,
        metadata,
        &config,
        ProgressCallback::default(),
        CancelToken::never(),
    )
    .unwrap();

    let bits = |plane: &Buffer2<f32>| bits(plane.pixels());
    assert_eq!(
        bits(ram.image.channel(0)),
        bits(disk.image.channel(0)),
        "stacked image differs between RAM and disk tiers"
    );
    assert_eq!(
        bits(&ram.coverage.as_ref().unwrap().to_plane()),
        bits(&disk.coverage.as_ref().unwrap().to_plane()),
        "coverage differs"
    );
    let ram_linear_variance = ram.linear_variance.as_ref().unwrap();
    let disk_linear_variance = disk.linear_variance.as_ref().unwrap();
    for channel in 0..ram.image.channels() {
        assert_eq!(
            bits(ram.weight.as_ref().unwrap().channel(channel)),
            bits(disk.weight.as_ref().unwrap().channel(channel)),
            "weight channel {channel} differs"
        );
        assert_eq!(
            bits(ram_linear_variance.channel(channel)),
            bits(disk_linear_variance.channel(channel)),
            "variance channel {channel} differs"
        );
    }
}

fn source_stats(cache: &FrameCache) -> impl Iterator<Item = &FrameStats> {
    cache.frames.iter().map(|frame| &frame.source_stats)
}

#[test]
fn stack_empty_paths() {
    let paths: Vec<PathBuf> = vec![];
    let result = stack(
        &paths,
        &StackConfig::default(),
        ProgressCallback::default(),
        CancelToken::never(),
    );
    assert!(matches!(result.unwrap_err(), Error::NoFrames));
}

#[test]
fn stack_images_empty() {
    let result = combine(Vec::new(), &StackConfig::default());
    assert!(matches!(result.unwrap_err(), Error::NoFrames));
}

#[test]
fn stack_nonexistent_file() {
    let paths = vec![PathBuf::from("/nonexistent/image.fits")];
    let result = stack(
        &paths,
        &StackConfig::default(),
        ProgressCallback::default(),
        CancelToken::never(),
    );
    assert!(matches!(result.unwrap_err(), Error::ImageLoad(_)));
}

#[test]
fn stack_rejects_invalid_config_before_loading() {
    let paths = vec![
        PathBuf::from("/a.fits"),
        PathBuf::from("/b.fits"),
        PathBuf::from("/c.fits"),
    ];
    let error = stack(
        &paths,
        &StackConfig::weighted(vec![1.0, 2.0]),
        ProgressCallback::default(),
        CancelToken::never(),
    )
    .unwrap_err();
    assert!(matches!(
        error,
        Error::Config(StackConfigError::ManualWeightCountMismatch {
            expected: 3,
            actual: 2,
        })
    ));

    let error = stack(
        &paths,
        &StackConfig::sigma_clipped(-1.0),
        ProgressCallback::default(),
        CancelToken::never(),
    )
    .unwrap_err();
    assert!(matches!(
        error,
        Error::Config(StackConfigError::Field(invalid))
            if invalid.field == "sigma_low" && invalid.value == -1.0
    ));
}

#[test]
fn stack_images_in_memory_mean() {
    // In-memory stacking must match the documented mean: (10 + 20 + 30)/3 = 20.
    let dims = ImageDimensions::new((4, 4), 1);
    let images = vec![
        LinearImage::from_pixels(dims, vec![10.0; 16]),
        LinearImage::from_pixels(dims, vec![20.0; 16]),
        LinearImage::from_pixels(dims, vec![30.0; 16]),
    ];
    let config = StackConfig {
        method: CombineMethod::Mean(Rejection::None),
        ..Default::default()
    };
    let frames = images.into_iter().map(StackFrame::from).collect();
    let result = combine(frames, &config).unwrap().image;
    assert_eq!(result.channels(), 1);
    assert_eq!(result.channel(0).pixels(), &[20.0; 16]);
}

#[test]
fn a_frames_null_pixels_are_excluded_from_the_stack_at_those_pixels_alone() {
    // Three 2x2 frames holding 1, 2 and 3 everywhere. Frame 0 declares pixel 1 null, so the mean
    // there is over frames 1 and 2 alone — (2 + 3) / 2 = 2.5 — while every other pixel averages all
    // three to (1 + 2 + 3) / 3 = 2. Normalization is off so the two figures are the plain means.
    let dims = ImageDimensions::new((2, 2), 1);
    let frame = |value: f32, null_at: Option<usize>| {
        let mut image = LinearImage::from_pixels(dims, vec![value; 4]);
        if let Some(index) = null_at {
            let mut samples = vec![0.0f32; 4];
            samples[index] = f32::NAN;
            image.nulls = NullMask::of_non_finite(dims.size(), &[&samples]);
        }
        StackFrame::from(image)
    };
    let config = StackConfig {
        method: CombineMethod::Mean(Rejection::None),
        normalization: Normalization::None,
        ..Default::default()
    };

    let stacked = combine(
        vec![frame(1.0, Some(1)), frame(2.0, None), frame(3.0, None)],
        &config.clone(),
    )
    .unwrap();

    assert_eq!(stacked.image.channel(0).pixels(), &[2.0, 2.5, 2.0, 2.0]);
    // The reported coverage counts the same contributors the value was built from — two of three
    // frames at the masked pixel, all three elsewhere. It is per-pixel because a frame
    // declaring nulls is a frame that carries support, which is what makes the plane exist at all.
    let coverage = stacked.coverage.as_ref().unwrap().per_pixel().unwrap();
    assert_eq!(coverage.pixels(), &[1.0, 2.0 / 3.0, 1.0, 1.0]);

    // A frame null everywhere contributes nowhere, so the stack is the other two throughout —
    // (2 + 3) / 2 = 2.5 — rather than a division by a zero contributor count.
    let mut all_null = LinearImage::from_pixels(dims, vec![1.0; 4]);
    all_null.nulls = NullMask::of_non_finite(dims.size(), &[&[f32::NAN; 4]]);
    let stacked = combine(
        vec![
            StackFrame::from(all_null),
            frame(2.0, None),
            frame(3.0, None),
        ],
        &config,
    )
    .unwrap();
    assert_eq!(stacked.image.channel(0).pixels(), &[2.5; 4]);
}

#[test]
fn normalization_fits_a_masked_set_over_the_pixels_they_all_reached() {
    // A masked frame is partially covering, which sends normalization down the path that
    // re-measures over the pixels every frame *shares* instead of trusting what each measured over
    // its own. Those differ as soon as two frames are masked in different places: each frame's own
    // valid pixels then reach beyond the intersection, so a gradient makes the two answers diverge.
    //
    // 1x6, holes at opposite ends, and the second frame ten times the first:
    //
    //   frame 0 = [1, 2, 3, 4, ·, ·]   valid median 2.5, MAD 1
    //   frame 1 = [·, ·, 30, 40, 50, 60]   valid median 45, MAD 10
    //
    // Frame 0 is the reference, being the quieter of the two. Multiplicative scales frame 1 by
    // `reference_median / frame_median`:
    //
    //   over the shared pixels 2 and 3, that is 3.5 / 35 = 0.1, which puts frame 1's 30..60 onto
    //   frame 0's 3..6 and reconstructs the ramp;
    //   over each frame's own valid pixels it is 2.5 / 45 ≈ 0.056, which would land pixel 2 at
    //   about 2.3 instead of 3.
    //
    // So an exact `[1, 2, 3, 4, 5, 6]` is the fit measured over the shared pixels, and nothing else
    // produces it: 3.5/35 rounds to the f32 0.1, and 30 … 60 times that round back to 3 … 6.
    let dims = ImageDimensions::new((6, 1), 1);
    let mut low = LinearImage::from_pixels(dims, vec![1.0, 2.0, 3.0, 4.0, 900.0, 900.0]);
    low.nulls = NullMask::of_non_finite(dims.size(), &[&[0.0, 0.0, 0.0, 0.0, f32::NAN, f32::NAN]]);
    let mut high = LinearImage::from_pixels(dims, vec![900.0, 900.0, 30.0, 40.0, 50.0, 60.0]);
    high.nulls = NullMask::of_non_finite(dims.size(), &[&[f32::NAN, f32::NAN, 0.0, 0.0, 0.0, 0.0]]);
    let config = StackConfig {
        method: CombineMethod::Mean(Rejection::None),
        normalization: Normalization::Multiplicative,
        ..Default::default()
    };

    let stacked = combine(
        vec![StackFrame::from(low), StackFrame::from(high)],
        &config.clone(),
    )
    .unwrap();
    assert_eq!(
        stacked.image.channel(0).pixels(),
        &[1.0, 2.0, 3.0, 4.0, 5.0, 6.0]
    );

    // With no pixel that every frame reached there is nothing to fit on, which is named rather
    // than divided by: one frame null everywhere leaves the intersection empty.
    let mut all_null = LinearImage::from_pixels(dims, vec![10.0; 6]);
    all_null.nulls = NullMask::of_non_finite(dims.size(), &[&[f32::NAN; 6]]);
    assert!(matches!(
        combine(
            vec![
                StackFrame::from(all_null),
                StackFrame::from(LinearImage::from_pixels(dims, vec![20.0; 6])),
            ],
            &config
        )
        .unwrap_err(),
        Error::NoCommonCoverage
    ));
}

#[test]
fn stack_images_rejects_frames_whose_rows_run_from_opposite_ends() {
    // Rows are decoded in the order the file stored them, so a BOTTOM-UP frame and a TOP-DOWN one
    // of the same field are upside-down relative to each other. Averaging them is meaningless, and
    // registration cannot reconcile a mirrored field either — this names the cause where it can be
    // seen, instead of leaving it to surface as a registration failure with no stated reason.
    let dims = ImageDimensions::new((2, 2), 1);
    let frame = |row_order: Option<RowOrder>| {
        let mut image = LinearImage::from_pixels(dims, vec![1.0; 4]);
        image.metadata.provenance = row_order.map(|row_order| ImageProvenance {
            container: SourceContainer::Tiff,
            decoder: DecoderProvenance::Imaginarium,
            transfer: TransferProvenance::FloatRaster,
            color: ColorProvenance::Monochrome,
            clipped: false,
            demosaic: DemosaicProvenance::None,
            row_order,
        });
        StackFrame::from(image)
    };
    let stack = |orders: [Option<RowOrder>; 2]| {
        combine(
            orders.map(frame).into_iter().collect(),
            &StackConfig::default(),
        )
    };

    assert!(
        matches!(
            stack([Some(RowOrder::TopDown), Some(RowOrder::BottomUp)]).unwrap_err(),
            Error::RowOrderMismatch {
                index: 1,
                reference_index: 0,
                ..
            }
        ),
        "a mirrored frame must be named, not averaged in"
    );

    // Agreeing on either order stacks, and so does a set where a frame declares none — an
    // undeclared order is "cannot tell", which must not reject an in-memory frame.
    for orders in [
        [Some(RowOrder::TopDown), Some(RowOrder::TopDown)],
        [Some(RowOrder::BottomUp), Some(RowOrder::BottomUp)],
        [Some(RowOrder::BottomUp), None],
        [None, None],
    ] {
        assert!(stack(orders).is_ok(), "orders {orders:?} must stack");
    }
}

#[test]
fn stack_images_rejects_frames_decoded_into_different_sample_domains() {
    type Declared<'a> = Option<(f32, ScaleOrigin, Option<&'a str>)>;

    // The case decode-time normalization introduced: a `uint16` FITS is divided by 65535, a
    // `float32` one holding the same ADU is taken as already normalized and divided by 1. Both
    // present as `FitsNormalized` and agree on every other axis, and `Normalization::Global` would
    // absorb the 65535× into its fitted gain and hand back a plausible-looking stack.
    let domain =
        |physical_scale: f32, scale_origin: ScaleOrigin, unit: Option<&str>| ImageProvenance {
            container: SourceContainer::Fits,
            decoder: DecoderProvenance::FitsWell,
            transfer: TransferProvenance::FitsNormalized(FitsTransferProvenance {
                bscale: 1.0,
                bzero: 0.0,
                physical_scale,
                scale_origin,
                unit: unit.map(str::to_owned),
                hdu: FitsHduProvenance {
                    index: 0,
                    extname: None,
                    extver: None,
                },
                checksum: FitsChecksumProvenance {
                    datasum: FitsChecksumState::NotChecked,
                    checksum: FitsChecksumState::NotChecked,
                },
            }),
            color: ColorProvenance::Monochrome,
            clipped: false,
            demosaic: DemosaicProvenance::None,
            row_order: RowOrder::TopDown,
        };
    let frame = |declared: Declared<'_>| {
        let mut image = LinearImage::from_pixels(ImageDimensions::new((2, 2), 1), vec![1.0; 4]);
        image.metadata.provenance =
            declared.map(|(scale, origin, unit)| domain(scale, origin, unit));
        image
    };
    let stack = |frames: [Declared<'_>; 2]| {
        combine(
            frames
                .map(|declared| frame(declared).into())
                .into_iter()
                .collect(),
            &StackConfig::default(),
        )
    };

    assert!(
        matches!(
            stack([
                Some((65_535.0, ScaleOrigin::Declared, None)),
                Some((1.0, ScaleOrigin::Assumed, None)),
            ])
            .unwrap_err(),
            Error::SampleDomainMismatch {
                index: 1,
                reference_index: 0,
                ..
            }
        ),
        "a declared span against an assumed one must be named, not absorbed into the fitted gain"
    );

    // Two declared spans in one unit convert exactly: frame 1 reads 1.0 on twice frame 0's span,
    // so it is worth 2.0 of frame 0's units and the mean is 1.5.
    let converted = stack([
        Some((1.0, ScaleOrigin::Declared, None)),
        Some((2.0, ScaleOrigin::Declared, None)),
    ])
    .unwrap();
    assert_eq!(converted.image.channel(0).pixels(), &[1.5; 4]);

    // The same rejection with no span to give it away: one span, two quantities. Without BUNIT
    // these two frames are indistinguishable, and a surface brightness would be averaged with a
    // count rate.
    assert!(
        matches!(
            stack([
                Some((1.0, ScaleOrigin::Declared, Some("Jy/beam"))),
                Some((1.0, ScaleOrigin::Declared, Some("count/s"))),
            ])
            .unwrap_err(),
            Error::SampleDomainMismatch {
                index: 1,
                reference_index: 0,
                ..
            }
        ),
        "two units on one span must be named"
    );

    // A frame that states no unit cannot vouch for the ones that do, so the unit reference is the
    // first frame to state one — frame 1 here, which is what the error must name. Comparing every
    // frame against frame 0 alone would let this set through on frame order.
    assert!(
        matches!(
            combine(
                [
                    Some((1.0, ScaleOrigin::Declared, None)),
                    Some((1.0, ScaleOrigin::Declared, Some("Jy/beam"))),
                    Some((1.0, ScaleOrigin::Declared, Some("count/s"))),
                ]
                .map(|declared| frame(declared).into())
                .into_iter()
                .collect(),
                &StackConfig::default()
            )
            .unwrap_err(),
            Error::SampleDomainMismatch {
                index: 2,
                reference_index: 1,
                ..
            }
        ),
        "a unitless first frame must not vouch for two frames that disagree with each other"
    );

    // Matching domains stack, and so does a set where a frame declares none — an undeclared span
    // or unit is "cannot tell", which must not reject an in-memory frame.
    let declared = ScaleOrigin::Declared;
    let assumed = ScaleOrigin::Assumed;
    for domains in [
        [
            Some((65_535.0, declared, None)),
            Some((65_535.0, declared, None)),
        ],
        [Some((65_535.0, declared, None)), None],
        [None, None],
        [
            Some((1.0, declared, Some("ADU"))),
            Some((1.0, declared, Some("ADU"))),
        ],
        [
            Some((1.0, declared, Some("ADU"))),
            Some((1.0, declared, None)),
        ],
        [Some((1.0, assumed, None)), Some((1.0, assumed, None))],
    ] {
        assert!(stack(domains).is_ok(), "domains {domains:?} must stack");
    }
}

#[test]
fn stack_images_dimension_errors() {
    let a = LinearImage::from_pixels(ImageDimensions::new((4, 4), 1), vec![1.0; 16]);
    let b = LinearImage::from_pixels(ImageDimensions::new((2, 2), 1), vec![1.0; 4]);
    let result = combine(vec![a.into(), b.into()], &StackConfig::default());
    assert!(matches!(
        result.unwrap_err(),
        Error::DimensionMismatch(FrameDimensionMismatch { index: 1, .. })
    ));

    // Either plane of the pair is named for itself; the wrong-shaped one is the one reported.
    for (expected_plane, coverage, confidence) in [
        (
            FramePlane::Coverage,
            Buffer2::new_filled(2, 2, 1.0),
            Buffer2::new_filled(4, 4, 1.0),
        ),
        (
            FramePlane::Confidence,
            Buffer2::new_filled(4, 4, 1.0),
            Buffer2::new_filled(2, 2, 1.0),
        ),
    ] {
        let frame = stack_frame(
            LinearImage::from_pixels(ImageDimensions::new((4, 4), 1), vec![1.0; 16]),
            FrameQuality::Planes {
                coverage,
                confidence,
            },
        );
        let error = combine(vec![frame], &StackConfig::default()).unwrap_err();
        assert!(
            matches!(
                error,
                Error::WarpPlaneDimensionMismatch {
                    index: 0,
                    plane,
                    expected_width: 4,
                    expected_height: 4,
                    actual_width: 2,
                    actual_height: 2,
                } if plane == expected_plane
            ),
            "expected a {expected_plane} dimension error, got {error:?}"
        );
    }
}

#[test]
fn stack_images_rejects_invalid_warp_quality_values() {
    let dims = ImageDimensions::new((2, 1), 1);
    // Each plane carries its own range — coverage a fraction, confidence any non-negative weight —
    // so the out-of-range value is paired with an in-range partner to isolate it.
    for (coverage, confidence, expected_plane, expected_value) in [
        (
            Buffer2::new(2, 1, vec![1.0, 1.1]),
            Buffer2::new(2, 1, vec![1.0, 1.0]),
            FramePlane::Coverage,
            1.1,
        ),
        (
            Buffer2::new(2, 1, vec![1.0, 1.0]),
            Buffer2::new(2, 1, vec![1.0, -0.1]),
            FramePlane::Confidence,
            -0.1,
        ),
    ] {
        let error = combine(
            vec![stack_frame(
                LinearImage::from_pixels(dims, vec![1.0; 2]),
                FrameQuality::Planes {
                    coverage,
                    confidence,
                },
            )],
            &StackConfig::default(),
        )
        .unwrap_err();
        assert!(
            matches!(
                error,
                Error::InvalidWarpPlaneValue {
                    index: 0,
                    plane,
                    pixel: 1,
                    value,
                } if plane == expected_plane && value == expected_value
            ),
            "expected an out-of-range {expected_plane} value, got {error:?}"
        );
    }
}

/// Both halves of the pairing the combine leans on: a covered pixel at zero confidence would enter
/// the statistics weightless, and a confident pixel at zero coverage would be dropped despite
/// having data. Neither is a shape a warp produces, so both are rejected rather than combined.
#[test]
fn stack_images_rejects_warp_quality_planes_that_disagree_about_support() {
    let dims = ImageDimensions::new((2, 1), 1);
    for (coverage, confidence) in [
        (vec![1.0, 1.0], vec![1.0, 0.0]),
        (vec![1.0, 0.0], vec![1.0, 0.5]),
    ] {
        let expected_coverage = coverage[1];
        let expected_confidence = confidence[1];
        let error = combine(
            vec![stack_frame(
                LinearImage::from_pixels(dims, vec![1.0; 2]),
                FrameQuality::Planes {
                    coverage: Buffer2::new(2, 1, coverage),
                    confidence: Buffer2::new(2, 1, confidence),
                },
            )],
            &StackConfig::default(),
        )
        .unwrap_err();
        assert!(
            matches!(
                error,
                Error::FrameQualityPairMismatch {
                    index: 0,
                    pixel: 1,
                    coverage,
                    confidence,
                } if coverage == expected_coverage && confidence == expected_confidence
            ),
            "expected a pair mismatch at pixel 1, got {error:?}"
        );
    }

    // The matched pair the same planes describe still combines.
    let product = combine(
        vec![stack_frame(
            LinearImage::from_pixels(dims, vec![1.0; 2]),
            FrameQuality::Planes {
                coverage: Buffer2::new(2, 1, vec![1.0, 0.0]),
                confidence: Buffer2::new(2, 1, vec![1.0, 0.0]),
            },
        )],
        &StackConfig {
            method: CombineMethod::Mean(Rejection::None),
            normalization: Normalization::None,
            ..Default::default()
        },
    )
    .unwrap();
    assert_eq!(product.image.channel(0).pixels(), &[1.0, 0.0]);
}

#[test]
fn stack_images_rejects_each_nonfinite_sample_class_with_location() {
    let dimensions = ImageDimensions::new((2, 2), 3);
    let finite =
        LinearImage::from_planar_channels(dimensions, [vec![1.0; 4], vec![1.0; 4], vec![1.0; 4]]);

    for invalid_value in [f32::NAN, f32::INFINITY, f32::NEG_INFINITY] {
        let invalid = LinearImage::from_planar_channels(
            dimensions,
            [
                vec![1.0; 4],
                vec![2.0, 2.0, invalid_value, 2.0],
                vec![3.0; 4],
            ],
        );
        let error = combine(
            vec![finite.clone().into(), invalid.into()],
            &StackConfig::mean(),
        )
        .unwrap_err();

        let Error::NonFiniteImageSample {
            index,
            channel,
            pixel,
            value,
        } = error
        else {
            panic!("expected a non-finite image sample error, got {error:?}");
        };
        assert_eq!(index, 1);
        assert_eq!(channel, 1);
        assert_eq!(pixel, 2);
        assert_eq!(value.to_bits(), invalid_value.to_bits());
    }
}

#[test]
fn cancelled_combine_reports_cancellation_from_either_exit() {
    // The chunk walk abandons the output between chunks rather than unwinding, so a cancelled
    // run still assembles a `StackProduct` — one holding zeros wherever it stopped. Both of
    // `run_stacking`'s exits must report that as an error rather than hand it back: the
    // early one taken when no frame carries a quantization sigma, and the normal one.
    // The token is swapped in after the cache is built, so this exercises the combine itself
    // rather than the loader's own cancellation check.
    let dimensions = ImageDimensions::new((2, 1), 1);
    let config = StackConfig {
        method: CombineMethod::Mean(Rejection::None),
        normalization: Normalization::None,
        small_n: SmallN::none(),
        ..Default::default()
    };

    let without_sigmas = FrameCache::from_images(
        vec![
            LinearImage::from_pixels(dimensions, vec![1.0; 2]),
            LinearImage::from_pixels(dimensions, vec![3.0; 2]),
        ],
        Normalization::None,
    );
    let with_sigmas = make_cfa_stack_cache(
        vec![vec![0.4; 2], vec![0.2; 2]],
        &[0.01, 0.02],
        dimensions,
        Normalization::None,
    );

    for mut cache in [without_sigmas, with_sigmas] {
        // Uncancelled, the same cache and config produce a product — so the error below is
        // the token talking, not a combine that could never have succeeded.
        run_stacking(&cache, &config).expect("an uncancelled combine produces a product");

        let cancel = CancelToken::new();
        cancel.cancel();
        cache.core.cancel = cancel;
        assert!(matches!(
            run_stacking(&cache, &config).unwrap_err(),
            Error::Cancelled
        ));
    }
}

#[test]
fn cancelled_stack_returns_cancelled_error() {
    let a = LinearImage::from_pixels(ImageDimensions::new((4, 4), 1), vec![1.0; 16]);
    let mut invalid_pixels = vec![2.0; 16];
    invalid_pixels[0] = f32::NAN;
    let b = LinearImage::from_pixels(ImageDimensions::new((4, 4), 1), invalid_pixels);
    let cancel = CancelToken::new();
    cancel.cancel();
    let result = stack_images(
        vec![a.into(), b.into()],
        &StackConfig::default(),
        ProgressCallback::default(),
        cancel,
    );
    assert!(matches!(result.unwrap_err(), Error::Cancelled));
}

/// Coverage decides which frames reach each pixel, and the product's planes count them. Frame A
/// holds 10 and covers both pixels, frame B holds 40 and covers pixel 0 alone, and frame C holds
/// 10 with no quality planes, which is full support. Unit weights, mean combine:
///   px0: A, B, C → 60/3 = 20, coverage 3/3, weight 3, variance 3/3² = 1/3
///   px1: A, C    → 20/2 = 10, coverage 2/3, weight 2, variance 2/2² = 1/2
/// B taken in at px1 would read 20 there. A pixel no frame covers is the warp border's 0.
#[test]
fn coverage_decides_which_frames_reach_each_pixel() {
    let dims = ImageDimensions::new((2, 1), 1);
    let config = StackConfig {
        method: CombineMethod::Mean(Rejection::None),
        normalization: Normalization::None,
        ..Default::default()
    };
    let covered = |value: f32, coverage: [f32; 2]| {
        stack_frame(
            LinearImage::from_pixels(dims, vec![value; 2]),
            FrameQuality::from_coverage(Buffer2::new(2, 1, coverage.to_vec())),
        )
    };
    let product = combine(
        vec![
            covered(10.0, [1.0, 1.0]),
            covered(40.0, [1.0, 0.0]),
            StackFrame::from(LinearImage::from_pixels(dims, vec![10.0; 2])),
        ],
        &config.clone(),
    )
    .unwrap();
    assert_eq!(product.image.channel(0).pixels(), &[20.0, 10.0]);
    assert_eq!(
        product.coverage.as_ref().unwrap().to_plane().pixels(),
        &[1.0, 2.0 / 3.0]
    );
    assert_eq!(
        product.weight.as_ref().unwrap().channel(0).pixels(),
        &[3.0, 2.0]
    );
    assert_eq!(
        product
            .linear_variance
            .as_ref()
            .unwrap()
            .channel(0)
            .pixels(),
        &[1.0 / 3.0, 0.5]
    );

    let alone = combine(vec![covered(10.0, [1.0, 0.0])], &config).unwrap();
    assert_eq!(alone.image.channel(0).pixels(), &[10.0, 0.0]);
    assert_eq!(
        alone.coverage.as_ref().unwrap().to_plane().pixels(),
        &[1.0, 0.0]
    );
}

/// Three frames covering pixels 2 and 3 alone. Their stated medians 12, 24, 31 and MADs 2, 4, 1
/// make frame 2 the reference and the multiplicative gains 31/12, 31/24 and 1. As combined, each
/// frame's σ is its gain times 1.4826 × MAD — 31/6, 31/6 and 1 in units of 1.4826 — so the
/// inverse-variance weights stand 36 : 36 : 961, out of 1033. Pixel 2 is
/// (36·(10·31/12 + 20·31/24) + 961·30)/1033 = 30 690/1033, pixel 3 likewise 33 356/1033, and the
/// uncovered pixels are 0. What those pixels hold — zeros, or ±1e20 — changes none of it.
#[test]
fn common_coverage_makes_reference_norms_and_noise_weights_fill_invariant() {
    let dims = ImageDimensions::new((6, 1), 1);
    let coverage = Buffer2::new(6, 1, vec![0.0, 0.0, 1.0, 1.0, 0.0, 0.0]);
    let make_cache = |fill: [f32; 4]| {
        let frames = [
            [fill[0], fill[1], 10.0, 14.0, fill[2], fill[3]],
            [fill[3], fill[2], 20.0, 28.0, fill[1], fill[0]],
            [fill[1], fill[3], 30.0, 32.0, fill[0], fill[2]],
        ]
        .into_iter()
        .zip([(12.0, 2.0), (24.0, 4.0), (31.0, 1.0)])
        .map(|(pixels, (median, mad))| {
            let mut frame = stack_frame(
                LinearImage::from_pixels(dims, pixels.to_vec()),
                FrameQuality::from_coverage(coverage.clone()),
            );
            frame.source_stats = FrameStats {
                channels: [MedianMad { median, mad }].into_iter().collect(),
                quantization_sigma: None,
                facts: FrameFacts {
                    domain: None,
                    row_order: None,
                    cfa_type: None,
                },
            };
            frame
        })
        .collect();
        FrameCache::from_stack_frames(
            frames,
            Normalization::Multiplicative,
            ProgressCallback::default(),
            CancelToken::never(),
        )
        .unwrap()
    };

    let caches = [
        make_cache([0.0, 0.0, 0.0, 0.0]),
        make_cache([-1e20, 1e20, -123_456.0, 789_012.0]),
    ];
    for cache in &caches {
        let norms = cache.frame_norms.as_ref().unwrap();
        assert_eq!(norms[0].channels[0].gain, 31.0 / 12.0);
        assert_eq!(norms[1].channels[0].gain, 31.0 / 24.0);
        assert_eq!(norms[2].channels[0].gain, 1.0);
        assert!(norms.iter().all(|norm| norm.channels[0].offset == 0.0));

        // σ, its gain, square, inverse, the sum and the quotient: 7 f32 roundings, 4ε relative.
        let weights = resolve_weights(&Weighting::Noise, source_stats(cache), Some(norms)).unwrap();
        for (weight, expected) in weights.iter().zip([36.0, 36.0, 961.0]) {
            let expected = expected / 1033.0;
            assert_close!(*weight, expected, 4.0 * f64::from(f32::EPSILON) * expected);
        }
    }

    let config = StackConfig {
        method: CombineMethod::Mean(Rejection::None),
        weighting: Weighting::Noise,
        normalization: Normalization::Multiplicative,
        ..Default::default()
    };
    let first = run_stacking(&caches[0], &config).expect("this cache is never cancelled");
    let second = run_stacking(&caches[1], &config).expect("this cache is never cancelled");
    assert_eq!(
        first.image.channel(0).pixels(),
        second.image.channel(0).pixels()
    );
    assert_eq!(first.image.channel(0).pixels()[0..2], [0.0, 0.0]);
    // The weights' 4ε, the normalized samples' rounding and the final one: 6ε relative.
    for (pixel, expected) in [(2, 30_690.0 / 1033.0), (3, 33_356.0 / 1033.0)] {
        assert_close!(
            first.image.channel(0).pixels()[pixel],
            expected,
            6.0 * f64::from(f32::EPSILON) * expected
        );
    }
    assert_eq!(first.image.channel(0).pixels()[4..6], [0.0, 0.0]);
}

#[test]
fn only_normalization_requires_common_coverage() {
    let dims = ImageDimensions::new((2, 1), 1);
    let frames = || {
        vec![
            stack_frame(
                LinearImage::from_pixels(dims, vec![1.0, 2.0]),
                FrameQuality::from_coverage(Buffer2::new(2, 1, vec![1.0, 0.0])),
            ),
            stack_frame(
                LinearImage::from_pixels(dims, vec![3.0, 4.0]),
                FrameQuality::from_coverage(Buffer2::new(2, 1, vec![0.0, 1.0])),
            ),
        ]
    };
    let error = combine(
        frames(),
        &StackConfig {
            normalization: Normalization::Global,
            ..Default::default()
        },
    )
    .unwrap_err();
    assert!(matches!(error, Error::NoCommonCoverage));

    let product = combine(
        frames(),
        &StackConfig {
            normalization: Normalization::None,
            ..Default::default()
        },
    )
    .unwrap();
    assert_eq!(product.image.channel(0).pixels(), &[1.0, 4.0]);
}

#[test]
fn confidence_scales_a_contribution_rather_than_gating_it() {
    // px0: A (q 1, val 10) + B (q .5, val 20) → 20/1.5 = 40/3, so B's half confidence halves its
    // pull without excluding it — both frames still count as covering the pixel, with weight 1.5
    // and variance (1 + 0.25)/1.5² = 5/9. At px1 B has no support at all, which is what does
    // exclude a frame: A alone, and coverage 1/2. Every sum is exact; each figure rounds once.
    let dims = ImageDimensions::new((2, 1), 1);
    let a = LinearImage::from_pixels(dims, vec![10.0, 10.0]);
    let b = LinearImage::from_pixels(dims, vec![20.0, 20.0]);
    let config = StackConfig {
        method: CombineMethod::Mean(Rejection::None),
        normalization: Normalization::None,
        ..Default::default()
    };
    let frames = vec![
        stack_frame(
            a,
            FrameQuality::Planes {
                coverage: Buffer2::new(2, 1, vec![1.0, 1.0]),
                confidence: Buffer2::new(2, 1, vec![1.0, 1.0]),
            },
        ),
        stack_frame(
            b,
            FrameQuality::Planes {
                coverage: Buffer2::new(2, 1, vec![1.0, 0.0]),
                confidence: Buffer2::new(2, 1, vec![0.5, 0.0]),
            },
        ),
    ];
    let product = combine(frames, &config).unwrap();
    assert_eq!(product.image.channel(0).pixels(), &[40.0 / 3.0, 10.0]);
    assert_eq!(
        product.coverage.as_ref().unwrap().to_plane().pixels(),
        &[1.0, 0.5]
    );
    assert_eq!(
        product.weight.as_ref().unwrap().channel(0).pixels(),
        &[1.5, 1.0]
    );
    assert_eq!(
        product
            .linear_variance
            .as_ref()
            .unwrap()
            .channel(0)
            .pixels(),
        &[5.0 / 9.0, 1.0]
    );
}

/// A uniform −0.7 survives the warp and the weighted combine at every interpolation method. The
/// warp sums up to 64 weighted taps in f32, each addition rounding by at most half an ulp of 0.7
/// (3e-8): under 2e-6 in the warped frame, half that in its mean with the unwarped one.
#[test]
fn signed_uniform_warp_and_weighted_combine_preserve_dc() {
    let dims = ImageDimensions::new((24, 20), 1);
    let expected = -0.7;
    let source = LinearImage::from_pixels(dims, vec![expected; dims.pixel_count()]);
    let transform = WarpTransform::new(Transform::translation(DVec2::new(-2.37, 1.43)));
    let config = StackConfig {
        method: CombineMethod::Mean(Rejection::None),
        normalization: Normalization::None,
        ..Default::default()
    };

    for method in InterpolationMethod::ALL {
        let warped = resample::warp(&source, &transform, config::internals::warp_params(method));
        let frames = vec![
            StackFrame::from(source.clone()),
            StackFrame::registered(&source, warped),
        ];
        let product = combine(frames, &config.clone()).unwrap();
        for (pixel, &actual) in product.image.channel(0).pixels().iter().enumerate() {
            assert_close!(actual, expected, 1e-6, "{method:?} pixel {pixel}");
        }
    }
}

/// Registered frames normalize on their paired pixels. One star field seen twice: frame A as it is
/// with sky noise 0.002, frame B at 0.8 of the signal with three times the noise, rendered a pixel
/// to the right and warped back onto A's grid by a whole-pixel translation — a copy, not an
/// interpolation, whose coverage plane puts the fit on the pixels both frames reach. A is the
/// quieter and so the reference, and the gain carrying B onto it is 1/0.8 = 1.25; the ratio of sky
/// spreads would read 1/3. The Deming slope's standard error is
/// `√((σ²_A + g²σ²_B)/S_xx)`, with `S_xx` B's spread over the frame — a column more than the paired
/// pixels, which moves it by under 1% — and the fit lands within 5 of them.
#[test]
fn registered_global_normalization_uses_paired_signal_samples() {
    let size = Size2us::new(128, 128);
    let dims = ImageDimensions::new((size.width, size.height), 1);
    let mut rng = TestRng::new(0x5ca1e);
    let stars: Vec<(Vec2, f32)> = (0..60)
        .map(|_| {
            let center = Vec2::new(
                8.0 + rng.next_f32() * (size.width - 16) as f32,
                8.0 + rng.next_f32() * (size.height - 16) as f32,
            );
            (center, 0.05 + rng.next_f32() * 0.7)
        })
        .collect();
    let sky = Sky {
        level: 0.1,
        noise: 0.0,
        clamp: false,
    };
    let field = |shift: f32, gain: f32, noise: f32, seed: u64| {
        let shifted: Vec<(Vec2, f32)> = stars
            .iter()
            .map(|&(center, peak)| (center + Vec2::new(shift, 0.0), peak))
            .collect();
        let mut pixels: Vec<f32> = SkyField::render(size, sky, 1.5, &shifted, 0)
            .pixels
            .pixels()
            .iter()
            .map(|&value| value * gain)
            .collect();
        patterns::add_gaussian_noise(&mut pixels, noise, seed);
        LinearImage::from_pixels(dims, pixels)
    };
    let a = field(0.0, 1.0, 0.002, 1);
    let b = field(1.0, 0.8, 0.006, 2);
    let params = config::internals::warp_params(InterpolationMethod::Bilinear);
    let warped_b = resample::warp(
        &b,
        &WarpTransform::new(Transform::translation(DVec2::new(1.0, 0.0))),
        params,
    );
    let cache = FrameCache::from_stack_frames(
        vec![StackFrame::from(a), StackFrame::registered(&b, warped_b)],
        Normalization::Global,
        ProgressCallback::default(),
        CancelToken::never(),
    )
    .unwrap();
    let norms = cache.frame_norms.as_ref().unwrap();
    let gain = f64::from(norms[1].channels[0].gain);

    let truth = SkyField::render(size, sky, 1.5, &stars, 0).pixels;
    let mean = truth.pixels().iter().map(|&t| f64::from(t)).sum::<f64>() / truth.len() as f64;
    let spread: f64 = truth
        .pixels()
        .iter()
        .map(|&t| (0.8 * (f64::from(t) - mean)).powi(2))
        .sum();
    let standard_error = ((0.002f64.powi(2) + 1.25f64.powi(2) * 0.006f64.powi(2)) / spread).sqrt();
    assert_eq!(norms[0].channels[0].gain, 1.0, "frame A is the reference");
    assert_close!(gain, 1.25, 5.0 * standard_error);
    let mads = source_stats(&cache)
        .map(|stats| f64::from(stats.channels[0].mad))
        .collect::<Vec<_>>();
    assert!(
        (mads[0] / mads[1] - 1.25).abs() > 100.0 * standard_error,
        "premise: the sky spreads alone must miss the gain"
    );
}

/// A half-pixel bilinear warp averages four source pixels equally, so its confidence `1/Σw²` is 4.
/// The two frames share one source and so one noise level: weights 1/2 each, and the warped
/// frame's effective weight is 4 times the unwarped one's — the confidence applied once, where
/// twice would give 16. The product's weight is `Σ w·c` = 0.5 + 2 = 2.5.
#[test]
fn registered_noise_weight_applies_half_pixel_confidence_once() {
    let dims = ImageDimensions::new((64, 48), 1);
    let mut rng = TestRng::new(0x1234_5678);
    let pixels = (0..dims.pixel_count())
        .map(|_| rng.next_f32() - 0.5)
        .collect();
    let source = LinearImage::from_pixels(dims, pixels);
    let params = config::internals::warp_params(InterpolationMethod::Bilinear);
    let frames = vec![
        StackFrame::registered(
            &source,
            resample::warp(&source, &WarpTransform::new(Transform::identity()), params),
        ),
        StackFrame::registered(
            &source,
            resample::warp(
                &source,
                &WarpTransform::new(Transform::translation(DVec2::splat(0.5))),
                params,
            ),
        ),
    ];
    let cache = FrameCache::from_stack_frames(
        frames,
        Normalization::None,
        ProgressCallback::default(),
        CancelToken::never(),
    )
    .unwrap();
    let base_weights = resolve_weights(&Weighting::Noise, source_stats(&cache), None).unwrap();
    assert_eq!(base_weights, [0.5, 0.5]);

    let pixel = 12 * dims.width() + 12;
    let identity_confidence = cache.frames[0]
        .quality
        .confidence()
        .unwrap()
        .chunk(pixel, pixel + 1)[0];
    let half_pixel_confidence = cache.frames[1]
        .quality
        .confidence()
        .unwrap()
        .chunk(pixel, pixel + 1)[0];
    let effective_ratio =
        base_weights[1] * half_pixel_confidence / (base_weights[0] * identity_confidence);
    assert_eq!(effective_ratio, 4.0);

    let product = run_stacking(
        &cache,
        &StackConfig {
            method: CombineMethod::Mean(Rejection::None),
            weighting: Weighting::Noise,
            normalization: Normalization::None,
            small_n: SmallN::none(),
            ..Default::default()
        },
    )
    .expect("this cache is never cancelled");
    assert_eq!(product.weight.as_ref().unwrap().channel(0)[pixel], 2.5);
}

#[test]
fn requested_planes_decide_what_the_combine_allocates() {
    // Every ancillary plane is a full image-sized allocation the reducer writes per pixel, so
    // "not requested" has to mean "never built" rather than "built and dropped".
    let dims = ImageDimensions::new((2, 1), 1);
    let frames = || {
        (0..6)
            .map(|i| StackFrame::from(LinearImage::from_pixels(dims, vec![i as f32, i as f32])))
            .collect::<Vec<_>>()
    };
    let stack = |config: StackConfig| combine(frames(), &config).unwrap();

    // A mean asked for everything gets everything.
    let all = stack(StackConfig {
        method: CombineMethod::Mean(Rejection::None),
        quality: QualityPlanes::ALL,
        ..Default::default()
    });
    assert!(all.coverage.is_some());
    assert!(all.weight.is_some());
    assert!(all.linear_variance.is_some());

    // A median is not a linear combination, so its variance plane is absent even though the
    // request asked for it — and is never allocated, not allocated and cleared.
    let median = stack(StackConfig {
        method: CombineMethod::Median,
        small_n: SmallN::none(),
        quality: QualityPlanes::ALL,
        ..Default::default()
    });
    assert!(median.coverage.is_some());
    assert!(median.weight.is_some(), "a median still reports weight");
    assert!(
        median.linear_variance.is_none(),
        "a median has no linear-combine variance factor"
    );

    // Image only: no ancillary plane survives, whatever the method would support.
    let bare = stack(StackConfig {
        method: CombineMethod::Mean(Rejection::None),
        quality: QualityPlanes::IMAGE_ONLY,
        ..Default::default()
    });
    assert!(bare.coverage.is_none());
    assert!(bare.weight.is_none());
    assert!(bare.linear_variance.is_none());
    // The combined pixels are unaffected by which planes were asked for.
    assert_eq!(
        bare.image.channel(0).pixels(),
        all.image.channel(0).pixels()
    );
}

#[test]
fn coverage_keeps_real_values_from_sigma_rejection_at_sparse_edges() {
    // An edge covered by 2/5 frames. Without coverage the 3 zero border-fills dominate SigmaClip
    // (median 0) and reject the real 0.125 values → a dark edge. Coverage excludes the fills, so
    // only the 2 real frames combine: 0.125. Without coverage maps a plain mean takes the fills in:
    // 0.25/5.
    let dims = ImageDimensions::new((1, 1), 1);
    let frames: Vec<StackFrame> = [0.125, 0.125, 0.0, 0.0, 0.0]
        .iter()
        .zip([1.0, 1.0, 0.0, 0.0, 0.0])
        .map(|(&v, c)| {
            stack_frame(
                LinearImage::from_pixels(dims, vec![v]),
                FrameQuality::from_coverage(Buffer2::new(1, 1, vec![c])),
            )
        })
        .collect();
    let config = StackConfig {
        method: CombineMethod::Mean(Rejection::sigma_clip(2.5)),
        normalization: Normalization::None,
        ..Default::default()
    };
    let edge = combine(frames, &config).unwrap();
    assert_eq!(edge.image.channel(0).pixels(), &[0.125]);

    let uncovered: Vec<StackFrame> = [0.125, 0.125, 0.0, 0.0, 0.0]
        .iter()
        .map(|&v| LinearImage::from_pixels(dims, vec![v]).into())
        .collect();
    let mean = StackConfig {
        method: CombineMethod::Mean(Rejection::None),
        normalization: Normalization::None,
        ..Default::default()
    };
    let dark = combine(uncovered, &mean).unwrap();
    assert_eq!(dark.image.channel(0).pixels(), &[0.05]);
}

#[test]
fn rejection_emits_channel_shaped_survivor_weight_and_linear_variance() {
    // Percentile clipping removes each channel's high value, hence a different source frame:
    // R keeps f0/f1, G keeps f1/f2, B keeps f0/f2. Manual weights [1,2,3] normalize to
    // [1/6,2/6,3/6].
    let dims = ImageDimensions::new((1, 1), 3);
    let frames = vec![
        LinearImage::from_pixels(dims, vec![1.0, 100.0, 1.0]).into(),
        LinearImage::from_pixels(dims, vec![2.0, 2.0, 100.0]).into(),
        LinearImage::from_pixels(dims, vec![100.0, 3.0, 3.0]).into(),
    ];
    let config = StackConfig {
        method: CombineMethod::Mean(Rejection::Percentile(PercentileClipConfig::new(0.0, 34.0))),
        weighting: Weighting::Manual(vec![1.0, 2.0, 3.0]),
        normalization: Normalization::None,
        small_n: SmallN::none(),
        ..Default::default()
    };

    let result = combine(frames, &config).unwrap();

    assert_eq!(result.coverage.as_ref().unwrap()[0], 1.0);
    let expected_values: [f64; 3] = [5.0 / 3.0, 13.0 / 5.0, 5.0 / 2.0];
    let expected_weights: [f64; 3] = [3.0 / 6.0, 5.0 / 6.0, 4.0 / 6.0];
    let expected_linear_variances: [f64; 3] = [5.0 / 9.0, 13.0 / 25.0, 10.0 / 16.0];
    let linear_variance = result.linear_variance.as_ref().unwrap();
    assert!(matches!(&result.weight, Some(QualityMap::PerChannel(_))));
    assert!(matches!(linear_variance, QualityMap::PerChannel(_)));
    // The normalized weights 1/6, 2/6, 3/6 each round once in f32; a sum, product or ratio of two
    // of them, rounded again, sits within 3 half-ulps — 2ε relative — of the exact fraction.
    for channel in 0..3 {
        let close = |actual: f32, expected: f64, what: &str| {
            assert_close!(
                actual,
                expected,
                2.0 * f64::from(f32::EPSILON) * expected,
                "channel {channel} {what}"
            );
        };
        close(
            result.image.channel(channel)[0],
            expected_values[channel],
            "value",
        );
        close(
            result.weight.as_ref().unwrap().channel(channel)[0],
            expected_weights[channel],
            "weight",
        );
        close(
            linear_variance.channel(channel)[0],
            expected_linear_variances[channel],
            "variance",
        );
    }
    assert_ne!(
        result.weight.as_ref().unwrap().channel(0)[0],
        result.weight.as_ref().unwrap().channel(1)[0]
    );
    assert_ne!(linear_variance.channel(1)[0], linear_variance.channel(2)[0]);
}

#[test]
fn median_quality_uses_equal_weights_and_has_no_linear_variance() {
    let dims = ImageDimensions::new((8, 1), 1);
    let mk = |base: f32, spread: f32| -> Vec<f32> {
        (0..8).map(|i| base + i as f32 * spread / 7.0).collect()
    };
    let frames = || -> Vec<StackFrame> {
        vec![
            LinearImage::from_pixels(dims, mk(100.0, 1.0)).into(),
            LinearImage::from_pixels(dims, mk(100.0, 20.0)).into(),
            LinearImage::from_pixels(dims, mk(100.0, 2.0)).into(),
        ]
    };
    let stack = |config: &StackConfig| combine(frames(), config).unwrap();

    let explicit = stack(&StackConfig {
        method: CombineMethod::Median,
        weighting: Weighting::Noise,
        normalization: Normalization::None,
        ..Default::default()
    });
    assert!(explicit.linear_variance.is_none());
    // The middle frame is the median at every pixel, sample for sample.
    assert_eq!(explicit.image.channel(0).pixels(), mk(100.0, 2.0));
    assert_eq!(
        explicit.coverage.as_ref().unwrap().to_plane().pixels(),
        &[1.0; 8]
    );
    assert_eq!(
        explicit.weight.as_ref().unwrap().channel(0).pixels(),
        &[3.0; 8],
        "median quality must count unit-confidence contributors"
    );

    for (name, config) in [
        ("default", StackConfig::default()),
        ("sigma", StackConfig::sigma_clipped(2.5)),
        ("linear fit", StackConfig::linear_fit(3.0)),
        ("GESD", StackConfig::gesd()),
        ("flat", StackConfig::flat()),
        ("light", StackConfig::light()),
        (
            "manual weighting",
            StackConfig::weighted(vec![1.0, 2.0, 3.0]),
        ),
    ] {
        let downgraded = stack(&config);
        assert!(
            downgraded.linear_variance.is_none(),
            "{name} must expose no linear variance after its small-N median downgrade"
        );
        assert_eq!(
            downgraded.weight.as_ref().unwrap().channel(0).pixels(),
            &[3.0; 8],
            "{name} median fallback must count unit-confidence contributors"
        );
    }

    let linear_fallback = stack(&StackConfig {
        method: CombineMethod::Mean(Rejection::sigma_clip(2.5)),
        small_n: SmallN {
            min_frames: 4,
            fallback: CombineMethod::Mean(Rejection::None),
        },
        ..Default::default()
    });
    assert_eq!(
        linear_fallback.linear_variance.unwrap().channel(0).pixels(),
        &[1.0 / 3.0; 8]
    );
}

#[test]
fn disk_backed_stack_combines_via_mmap() {
    // Force the disk tier (1-byte memory budget) so the full chunked combine reads
    // memory-mapped `Plane`s: mean(10, 20, 30) = 20 at every pixel.
    let temp_dir = TempDir::new("lumos_disk_stack_combine_test");

    let dims = ImageDimensions::new((4, 4), 1);
    let mut paths = Vec::new();
    for (i, &v) in [10.0f32, 20.0, 30.0].iter().enumerate() {
        let image = LinearImage::from_pixels(dims, vec![v; 16]);
        let path = temp_dir.join(format!("frame{i}.tiff"));
        image.save(&path).unwrap();
        paths.push(path);
    }

    let config = StackConfig {
        method: CombineMethod::Mean(Rejection::None),
        normalization: Normalization::None,
        cache: CacheConfig {
            memory_override: Some(1), // forces disk-backed (mmap) storage
            ..CacheConfig::with_cache_dir(temp_dir.join("cache"))
        },
        ..Default::default()
    };
    let result = stack(
        &paths,
        &config,
        ProgressCallback::default(),
        CancelToken::never(),
    )
    .unwrap()
    .image;
    assert_eq!(result.channel(0).pixels(), &[20.0; 16]);
}

/// Noise weights are inverse variances, `1/σ²` with σ = 1.4826 × each frame's MAD. Ramps of 100
/// samples spaced `d` have a MAD of 25d, so a spacing of 1/2 against 20 is a σ ratio of 40: weights
/// 1600/1601 and 1/1601. Each σ, square, inverse, sum and quotient rounds once in f32, so each
/// weight is within 5 half-ulps, 3ε relative. Three frames of one spacing, 1/8, have one MAD
/// exactly, so their weights are equal: x/(3x), within one rounding of the sum and one of the
/// quotient of 1/3. Frames with no spread at all have no noise to weigh by, and fall back to
/// equal weighting.
#[test]
fn noise_weights_are_inverse_variances() {
    let ramp = |start: f32, spacing: f32| {
        LinearImage::from_pixels(
            ImageDimensions::new((100, 1), 1),
            (0..100).map(|i| start + i as f32 * spacing).collect(),
        )
    };
    let weights = |images: Vec<LinearImage>| {
        resolve_weights(
            &Weighting::Noise,
            source_stats(&FrameCache::from_images(images, Normalization::None)),
            None,
        )
    };

    let clean_and_noisy = weights(vec![ramp(100.0, 0.5), ramp(190.0, 20.0)]).unwrap();
    for (weight, expected) in clean_and_noisy.iter().zip([1600.0 / 1601.0, 1.0 / 1601.0]) {
        assert_close!(*weight, expected, 3.0 * f64::from(f32::EPSILON) * expected);
    }

    let equal = weights(vec![
        ramp(100.0, 0.125),
        ramp(200.0, 0.125),
        ramp(300.0, 0.125),
    ])
    .unwrap();
    assert_eq!((equal[0], equal[1]), (equal[2], equal[2]));
    assert_close!(equal[0], 1.0 / 3.0, f64::from(f32::EPSILON) / 3.0);

    assert_eq!(weights(vec![ramp(50.0, 0.0); 3]), None);
}

/// Noise weights survive the rejection: pixel 0 holds 100, 90 and 999 across three frames, the
/// first two with MADs of 1/16 and 4. Sigma clipping at 2σ — median 100, MAD 10, so a band of ±29.7
/// — drops the 999, and the two left average under weights in the ratio (4·16)² = 4096 : 1:
/// (4096·100 + 90)/4097. The median of the three, 100, is what the small-stack fallback would give,
/// so `SmallN::none()` is what lets the clip run. The weights' roundings move the mean by under
/// 1e-9; the result rounds once, half an ulp of 100.
#[test]
fn noise_weights_survive_rejection() {
    let ramp = |start: f32, spacing: f32| -> Vec<f32> {
        (0..16).map(|i| start + i as f32 * spacing).collect()
    };
    let mut hit = ramp(100.0, 1.0 / 32.0);
    hit[0] = 999.0;
    let dims = ImageDimensions::new((16, 1), 1);
    let cache = FrameCache::from_images(
        [ramp(100.0, 1.0 / 64.0), ramp(90.0, 1.0), hit]
            .into_iter()
            .map(|pixels| LinearImage::from_pixels(dims, pixels))
            .collect(),
        Normalization::None,
    );
    let config = StackConfig {
        method: CombineMethod::Mean(Rejection::sigma_clip(2.0)),
        weighting: Weighting::Noise,
        small_n: SmallN::none(),
        ..Default::default()
    };
    let result = run_stacking(&cache, &config).expect("this cache is never cancelled");
    assert_close!(
        result.image.channel(0)[0],
        (4096.0 * 100.0 + 90.0) / 4097.0,
        f64::from(f32::EPSILON) * 100.0 / 2.0
    );
}

#[test]
fn noise_weighting_folds_normalization_gain() {
    // Two frames with identical MAD (σ_A = σ_B). Frame B's normalization gain is 2, so its
    // combined noise is 2σ: w_A ∝ 1/σ², w_B ∝ 1/(2σ)² = w_A/4 → normalized 0.8 / 0.2.
    // Without the pscale² term both weights would come out 0.5.
    let frame_stats = |mad: f32| {
        let mut channels = ArrayVec::new();
        channels.push(MedianMad { median: 0.5, mad });
        FrameStats {
            channels,
            quantization_sigma: None,
            facts: FrameFacts {
                domain: None,
                row_order: None,
                cfa_type: None,
            },
        }
    };
    let frame_norm = |gain: f32| {
        let mut channels = ArrayVec::new();
        channels.push(ChannelNorm { gain, offset: 0.0 });
        FrameNorm { channels }
    };
    let stats = vec![frame_stats(0.01), frame_stats(0.01)];
    let norms = vec![frame_norm(1.0), frame_norm(2.0)];

    // The gain-2 frame's inverse variance is the other's over 4 exactly, so the sum rounds once
    // and each quotient once: within 2 half-ulps of 0.8 and 0.2.
    let weights = resolve_weights(&Weighting::Noise, &stats, Some(&norms)).unwrap();
    for (weight, expected) in weights.iter().zip([0.8, 0.2]) {
        assert_close!(*weight, expected, f64::from(f32::EPSILON) * expected);
    }

    // Identity norms reproduce the unscaled weighting: equal σ → equal weights, x/(2x) exactly.
    let identity = vec![frame_norm(1.0), frame_norm(1.0)];
    let equal = resolve_weights(&Weighting::Noise, &stats, Some(&identity)).unwrap();
    assert_eq!(equal, [0.5, 0.5]);
}

#[test]
fn manual_weighting_is_scale_invariant() {
    let weights = resolve_weights(&Weighting::Manual(vec![1.0, 2.0, 3.0]), &[], None).unwrap();
    assert_eq!(weights, [1.0_f32 / 6.0, 2.0 / 6.0, 3.0 / 6.0]);

    let scale = f32::MIN_POSITIVE;
    let tiny = resolve_weights(
        &Weighting::Manual(vec![scale, 2.0 * scale, 3.0 * scale]),
        &[],
        None,
    )
    .unwrap();
    assert_eq!(tiny, weights);
}

#[test]
fn equal_weighting_returns_none() {
    let weights = resolve_weights(&Weighting::Equal, &[], None);
    assert!(weights.is_none());
}
