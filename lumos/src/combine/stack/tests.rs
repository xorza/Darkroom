use crate::combine::cache::slots::Slots;
use crate::combine::config::{Combine, Normalization, SmallN};
use crate::frame_store::capture_conditions::CaptureConditions;
use crate::frame_store::frame_facts::FrameFacts;
use crate::internals::prelude::*;
use crate::io::image::unverified_conditions::UnverifiedConditions;
use crate::memory::run_memory::RunMemory;

use crate::frame_store::frame_quality::FramePlane;

use crate::combine::rejection::Rejection;
use crate::combine::rejection::trim_config::TrimConfig;
use crate::combine::stack::*;
use crate::error::FrameDimensionMismatch;
use crate::frame_store::run_scratch::RunScratch;
use crate::internals;
use crate::internals::assertions::bits;
use crate::internals::synthetic::patterns;
use crate::internals::synthetic::sky_field::{Sky, SkyField};
use crate::io::image::cfa::{CfaImage, CfaType};
use crate::io::image::flat_gain::FlatGain;
use crate::io::image::image_provenance::{
    ColorProvenance, DecoderProvenance, DemosaicProvenance, ImageProvenance, RowOrder,
    SourceContainer, TransferProvenance,
};
use crate::io::image::mosaic_noise::MosaicNoise;
use crate::io::image::pixel_flags::{PixelFlags, QualityFlags};
use crate::io::image::sample_domain::{Pedestal, SampleDomain, ScaleOrigin};
use crate::io::raw::demosaic::bayer::CfaPattern;
use crate::math::statistics::{MedianMad, mad_to_sigma};
use crate::registration::registration_config::{self, InterpolationMethod};
use crate::registration::resample;
use crate::registration::transform::{Transform, WarpTransform};
use crate::stack_product::quality_map::QualityMap;
use common::TempDir;
use std::path::PathBuf;
use std::sync::Arc;

/// The combine, with no progress reported and no cancel.
fn combine(frames: Vec<StackFrame>, config: &StackConfig) -> Result<StackProduct, StackError> {
    stack_images(
        frames,
        config,
        ProgressCallback::default(),
        CancelToken::never(),
    )
}

/// `frame` measured to have white noise `sigma` in every slot, so each of its samples has variance
/// `sigma²` over its confidence.
fn with_noise(mut frame: StackFrame, sigma: f32) -> StackFrame {
    for noise in &mut frame.source_stats.noise {
        *noise = sigma;
    }
    frame
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
            image.metadata.quantization_sigma = Some(sigma);
            image
        })
        .collect();
    FrameCache::from_images(images, normalization)
}

/// A master states the largest step of its inputs once normalization scaled it, whatever the
/// reduction and its survivors: every figure is one of the inputs' σ times its gain, so exact.
/// - Multiplicative normalization of flat frames at 0.4 and 0.2 gives frame 1 a gain of 2: its σ
///   0.02 becomes 0.04, over frame 0's 0.01.
/// - A median of three frames of σ 0.01 states 0.01.
/// - A winsorized clip of σ 0.01 and 0.02 states 0.02.
/// - A σ-clip that rejects the 100 among eight frames of σ 0.01 still states 0.01.
#[test]
fn a_master_states_the_largest_step_of_its_inputs() {
    let dimensions = ImageDimensions::new((2, 1), 1);
    let stated = |pixels: Vec<Vec<f32>>, sigmas: &[f32], config: StackConfig| {
        let cache = make_cfa_stack_cache(pixels, sigmas, dimensions, config.normalization);
        let product = run_stacking(&cache, &config).expect("this cache is never cancelled");
        product.image.metadata.quantization_sigma
    };
    let mean = |rejection| Combine {
        method: CombineMethod::Mean(rejection),
        small_n: SmallN::none(),
    };
    let normalized = stated(
        vec![vec![0.4; 2], vec![0.2; 2]],
        &[0.01, 0.02],
        StackConfig {
            combine: mean(Rejection::None),
            weighting: Weighting::Manual(vec![0.25, 0.75]),
            normalization: Normalization::Multiplicative,
            ..StackConfig::light()
        },
    );
    assert_eq!(normalized, Some(0.04));
    let equal = |combine| StackConfig {
        combine,
        weighting: Weighting::Equal,
        normalization: Normalization::None,
        ..StackConfig::light()
    };
    let median = stated(
        vec![vec![0.3; 2], vec![0.4; 2], vec![0.5; 2]],
        &[0.01; 3],
        equal(Combine::median()),
    );
    assert_eq!(median, Some(0.01));
    let winsorized = stated(
        vec![vec![0.3; 2], vec![0.5; 2]],
        &[0.01, 0.02],
        equal(Combine::winsorized(2.5)),
    );
    assert_eq!(winsorized, Some(0.02));
    let rejected = stated(
        (0..8)
            .map(|frame| vec![[1.0, 2.0, 3.0, 4.0, 5.0, 6.0, 7.0, 100.0][frame], 1.0])
            .collect(),
        &[0.01; 8],
        equal(mean(Rejection::sigma_clip(2.0))),
    );
    assert_eq!(rejected, Some(0.01));
}

/// The load-bearing guarantee for memory-aware stacking: spilling frames to disk (mmap) and
/// combining must be **bit-identical** to the all-RAM combine — same frames, same math, only the
/// plane storage differs. Exercises σ-clip rejection + noise weighting + a partial coverage map (so
/// the coverage spill round-trips too), mono and RGB, with no normalization and with the global
/// one, whose common-domain medians are then read from mapped planes.
#[test]
fn disk_tier_output_is_bit_identical_to_memory_tier() {
    let (w, h, n) = (40usize, 30usize, 12usize);
    for channels in [1, 3] {
        for normalization in [Normalization::None, Normalization::Global] {
            let label = format!("{channels} channels, {normalization:?}");
            let dims = ImageDimensions::new((w, h), channels);
            let make_frame = |f: usize| -> StackFrame {
                let mut rng = TestRng::new(f as u64);
                let planes = (0..channels).map(|channel| {
                    let mut px: Vec<f32> = (0..w * h)
                        .map(|_| {
                            0.2 + (f as f32) * 0.01
                                + channel as f32 * 0.05
                                + (rng.next_f32() - 0.5) * 0.02
                        })
                        .collect();
                    px[(f * 7 + channel) % (w * h)] = 0.95; // an outlier so rejection fires
                    px
                });
                let mut image = LinearImage::from_planar_channels(dims, planes.collect::<Vec<_>>());
                // Every third frame was divided by a vignetted flat, whose gain the noise model
                // reads per sample: a mono flat for a mono frame, a mosaic's for a colour one.
                if f.is_multiple_of(3) {
                    let divisor = Buffer2::new(
                        w,
                        h,
                        (0..w * h)
                            .map(|index| 1.0 - 0.4 * (index % w) as f32 / w as f32)
                            .collect(),
                    );
                    let cfa = if channels == 1 {
                        CfaType::Mono
                    } else {
                        CfaType::Bayer(CfaPattern::Rggb)
                    };
                    image.metadata.flat_gain =
                        Some(Arc::new(FlatGain::of_divisor(&divisor, &cfa, |_| false)));
                }
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
            let config = StackConfig {
                normalization,
                ..StackConfig::light()
            };

            // `make_frame` is deterministic, so building each tier's frames from it is what makes
            // the two sets identical — the alternative, cloning one set, has to restate that the
            // stats came along.
            let ram = combine((0..n).map(make_frame).collect(), &config.clone()).unwrap();

            let scratch = TempDir::new("lumos_tier_test");
            let run_scratch = RunScratch::create(&scratch.join("cache")).unwrap();
            let spilled = |memory: u64| {
                let frames: Vec<StackFrame> = (0..n).map(make_frame).collect();
                let metadata = frames[0].image.metadata.clone();
                let stored = frames
                    .into_iter()
                    .map(|f| {
                        StoredFrame::spill(&run_scratch, &f.image, &f.quality, f.source_stats)
                            .unwrap()
                    })
                    .collect();
                stack_stored_frames(
                    stored,
                    CacheTier::of(true, RunMemory::new(memory, None)),
                    dims,
                    metadata,
                    &config,
                    ProgressCallback::default(),
                    CancelToken::never(),
                )
                .unwrap()
            };
            let disk = spilled(1 << 30);
            assert_eq!(disk.report.chunk_overcommit_bytes, 0, "{label}");
            // With no memory at all the floor holds the whole 30-row image past the budget: its
            // resident image, weight and inverse variance planes, the coverage plane the gather
            // writes and the flag byte a pixel no frame reached is flagged in, 40 × 30 × (12c + 5)
            // B, beside the rows of every channel of the 12 frames, the six coverage-and-confidence
            // pairs and the four flat gain grids at a byte a pixel, 40 × 30 × (48c + 52) B. The
            // stack is the same.
            let starved = spilled(0);
            assert_eq!(
                starved.report.chunk_overcommit_bytes,
                72_000 * channels as u64 + 68_400,
                "{label}"
            );
            for channel in 0..channels {
                assert_eq!(
                    bits(ram.image.channel(channel).pixels()),
                    bits(starved.image.channel(channel).pixels()),
                    "{label}: starved channel {channel} differs"
                );
            }

            let bits = |plane: &Buffer2<f32>| bits(plane.pixels());
            assert_eq!(
                bits(&ram.coverage.as_ref().unwrap().to_plane(0)),
                bits(&disk.coverage.as_ref().unwrap().to_plane(0)),
                "{label}: coverage differs"
            );
            let ram_variance = ram.inverse_variance.as_ref().unwrap();
            let disk_variance = disk.inverse_variance.as_ref().unwrap();
            for channel in 0..channels {
                assert_eq!(
                    bits(ram.image.channel(channel)),
                    bits(disk.image.channel(channel)),
                    "{label}: stacked channel {channel} differs"
                );
                assert_eq!(
                    bits(ram.weight.as_ref().unwrap().channel(channel)),
                    bits(disk.weight.as_ref().unwrap().channel(channel)),
                    "{label}: weight channel {channel} differs"
                );
                assert_eq!(
                    bits(ram_variance.channel(channel)),
                    bits(disk_variance.channel(channel)),
                    "{label}: variance channel {channel} differs"
                );
            }
        }
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
        &StackConfig {
            weighting: Weighting::Equal,
            normalization: Normalization::None,
            ..StackConfig::light()
        },
        &IngestConfig::default(),
        ProgressCallback::default(),
        CancelToken::never(),
    );
    assert!(matches!(result.unwrap_err(), StackError::NoFrames));
}

#[test]
fn stack_images_empty() {
    let result = combine(
        Vec::new(),
        &StackConfig {
            weighting: Weighting::Equal,
            normalization: Normalization::None,
            ..StackConfig::light()
        },
    );
    assert!(matches!(result.unwrap_err(), StackError::NoFrames));
}

#[test]
fn stack_nonexistent_file() {
    let paths = vec![PathBuf::from("/nonexistent/image.fits")];
    let result = stack(
        &paths,
        &StackConfig {
            weighting: Weighting::Equal,
            normalization: Normalization::None,
            ..StackConfig::light()
        },
        &IngestConfig::default(),
        ProgressCallback::default(),
        CancelToken::never(),
    );
    assert!(matches!(result.unwrap_err(), StackError::ImageLoad(_)));
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
        &StackConfig {
            weighting: Weighting::Manual(vec![1.0, 2.0]),
            normalization: Normalization::None,
            ..StackConfig::light()
        },
        &IngestConfig::default(),
        ProgressCallback::default(),
        CancelToken::never(),
    )
    .unwrap_err();
    assert!(matches!(
        error,
        StackError::Config(StackConfigError::ManualWeightCountMismatch {
            expected: 3,
            actual: 2,
        })
    ));

    let error = stack(
        &paths,
        &StackConfig {
            combine: Combine::sigma_clipped(-1.0),
            weighting: Weighting::Equal,
            normalization: Normalization::None,
            ..StackConfig::light()
        },
        &IngestConfig::default(),
        ProgressCallback::default(),
        CancelToken::never(),
    )
    .unwrap_err();
    assert!(matches!(
        error,
        StackError::Config(StackConfigError::Field(invalid))
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
        combine: Combine::mean(),
        weighting: Weighting::Equal,
        normalization: Normalization::None,
        ..StackConfig::light()
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
            image.flags = PixelFlags::of_non_finite(dims.size(), &[&samples]);
        }
        StackFrame::from(image)
    };
    let config = StackConfig {
        combine: Combine::mean(),
        normalization: Normalization::None,
        weighting: Weighting::Equal,
        ..StackConfig::light()
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
    let coverage = stacked
        .coverage
        .as_ref()
        .unwrap()
        .per_pixel()
        .unwrap()
        .channel(0);
    assert_eq!(coverage.pixels(), &[1.0, 2.0 / 3.0, 1.0, 1.0]);
    // Some frame reached every pixel, so the stack flags none.
    assert!(stacked.image.flags.is_none());

    // A pixel every frame declares null is reached by none: 0, with no coverage, and flagged.
    let stacked = combine(
        vec![
            frame(1.0, Some(3)),
            frame(2.0, Some(3)),
            frame(3.0, Some(3)),
        ],
        &config.clone(),
    )
    .unwrap();
    assert_eq!(stacked.image.channel(0).pixels(), &[2.0, 2.0, 2.0, 0.0]);
    let coverage = stacked
        .coverage
        .as_ref()
        .unwrap()
        .per_pixel()
        .unwrap()
        .channel(0);
    assert_eq!(coverage.pixels(), &[1.0, 1.0, 1.0, 0.0]);
    let flags = stacked.image.flags.as_ref().unwrap();
    assert_eq!(
        (0..4).map(|index| flags.at(index)).collect::<Vec<_>>(),
        [
            QualityFlags::default(),
            QualityFlags::default(),
            QualityFlags::default(),
            QualityFlags::NO_DATA
        ]
    );

    // A frame null everywhere contributes nowhere, so the stack is the other two throughout —
    // (2 + 3) / 2 = 2.5 — rather than a division by a zero contributor count.
    let mut all_null = LinearImage::from_pixels(dims, vec![1.0; 4]);
    all_null.flags = PixelFlags::of_non_finite(dims.size(), &[&[f32::NAN; 4]]);
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
    low.flags =
        PixelFlags::of_non_finite(dims.size(), &[&[0.0, 0.0, 0.0, 0.0, f32::NAN, f32::NAN]]);
    let mut high = LinearImage::from_pixels(dims, vec![900.0, 900.0, 30.0, 40.0, 50.0, 60.0]);
    high.flags =
        PixelFlags::of_non_finite(dims.size(), &[&[f32::NAN, f32::NAN, 0.0, 0.0, 0.0, 0.0]]);
    let config = StackConfig {
        combine: Combine::mean(),
        normalization: Normalization::Multiplicative,
        weighting: Weighting::Equal,
        ..StackConfig::light()
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
    all_null.flags = PixelFlags::of_non_finite(dims.size(), &[&[f32::NAN; 6]]);
    assert!(matches!(
        combine(
            vec![
                StackFrame::from(all_null),
                StackFrame::from(LinearImage::from_pixels(dims, vec![20.0; 6])),
            ],
            &config
        )
        .unwrap_err(),
        StackError::NoCommonCoverage
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
            &StackConfig {
                weighting: Weighting::Equal,
                normalization: Normalization::None,
                ..StackConfig::light()
            },
        )
    };

    assert!(
        matches!(
            stack([Some(RowOrder::TopDown), Some(RowOrder::BottomUp)]).unwrap_err(),
            StackError::RowOrderMismatch {
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
    type Declared<'a> = Option<(f64, ScaleOrigin, Option<&'a str>)>;

    // The case decode-time normalization introduced: a `uint16` FITS is divided by 65535, a
    // `float32` one holding the same ADU is taken as already normalized and divided by 1. Both
    // present as `FitsNormalized` and agree on every other axis, and `Normalization::Global` would
    // absorb the 65535× into its fitted gain and hand back a plausible-looking stack.
    // FITS files from anyone but lumos record no pedestal.
    let domain = |scale: f64, origin: ScaleOrigin, unit: Option<&str>| SampleDomain {
        scale,
        origin,
        pedestal: Pedestal::Unknown,
        unit: unit.map(str::to_owned),
    };
    let frame = |declared: Declared<'_>| {
        let mut image = LinearImage::from_pixels(ImageDimensions::new((2, 2), 1), vec![1.0; 4]);
        image.metadata.domain = declared.map(|(scale, origin, unit)| domain(scale, origin, unit));
        image
    };
    let stack = |frames: [Declared<'_>; 2]| {
        combine(
            frames
                .map(|declared| frame(declared).into())
                .into_iter()
                .collect(),
            &StackConfig {
                weighting: Weighting::Equal,
                normalization: Normalization::None,
                ..StackConfig::light()
            },
        )
    };

    assert!(
        matches!(
            stack([
                Some((65_535.0, ScaleOrigin::Declared, None)),
                Some((1.0, ScaleOrigin::Assumed, None)),
            ])
            .unwrap_err(),
            StackError::SampleDomainMismatch {
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

    // Known pedestals convert by an offset. Frame 0 keeps 0.5 on unit scale and frame 1 has none,
    // so frame 1's 1.0 is 1.0 + 0.5 = 1.5 in frame 0's domain and the mean is 1.25.
    let pedestal_frame = |pedestal| {
        let mut image = LinearImage::from_pixels(ImageDimensions::new((2, 2), 1), vec![1.0; 4]);
        image.metadata.domain = Some(SampleDomain {
            pedestal,
            ..domain(1.0, ScaleOrigin::Declared, None)
        });
        image
    };
    let offset = combine(
        vec![
            pedestal_frame(Pedestal::Kept(0.5)).into(),
            pedestal_frame(Pedestal::Removed).into(),
        ],
        &StackConfig {
            weighting: Weighting::Equal,
            normalization: Normalization::None,
            ..StackConfig::light()
        },
    )
    .unwrap();
    assert_eq!(offset.image.channel(0).pixels(), &[1.25; 4]);

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
            StackError::SampleDomainMismatch {
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
                &StackConfig {
                    weighting: Weighting::Equal,
                    normalization: Normalization::None,
                    ..StackConfig::light()
                }
            )
            .unwrap_err(),
            StackError::SampleDomainMismatch {
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
    let result = combine(
        vec![a.into(), b.into()],
        &StackConfig {
            weighting: Weighting::Equal,
            normalization: Normalization::None,
            ..StackConfig::light()
        },
    );
    assert!(matches!(
        result.unwrap_err(),
        StackError::DimensionMismatch(FrameDimensionMismatch { index: 1, .. })
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
        let error = combine(
            vec![frame],
            &StackConfig {
                weighting: Weighting::Equal,
                normalization: Normalization::None,
                ..StackConfig::light()
            },
        )
        .unwrap_err();
        assert!(
            matches!(
                error,
                StackError::WarpPlaneDimensionMismatch {
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
            &StackConfig {
                weighting: Weighting::Equal,
                normalization: Normalization::None,
                ..StackConfig::light()
            },
        )
        .unwrap_err();
        assert!(
            matches!(
                error,
                StackError::InvalidWarpPlaneValue {
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
            &StackConfig {
                weighting: Weighting::Equal,
                normalization: Normalization::None,
                ..StackConfig::light()
            },
        )
        .unwrap_err();
        assert!(
            matches!(
                error,
                StackError::FrameQualityPairMismatch {
                    index: 0,
                    pixel: 1,
                    plane: FramePlane::Coverage,
                    support,
                    confidence,
                } if support == expected_coverage && confidence == expected_confidence
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
            combine: Combine::mean(),
            normalization: Normalization::None,
            weighting: Weighting::Equal,
            ..StackConfig::light()
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
            &StackConfig {
                combine: Combine::mean(),
                weighting: Weighting::Equal,
                normalization: Normalization::None,
                ..StackConfig::light()
            },
        )
        .unwrap_err();

        let StackError::NonFiniteImageSample {
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
        combine: Combine {
            method: CombineMethod::Mean(Rejection::None),
            small_n: SmallN::none(),
        },
        normalization: Normalization::None,
        weighting: Weighting::Equal,
        ..StackConfig::light()
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
            StackError::Cancelled
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
        &StackConfig {
            weighting: Weighting::Equal,
            normalization: Normalization::None,
            ..StackConfig::light()
        },
        ProgressCallback::default(),
        cancel,
    );
    assert!(matches!(result.unwrap_err(), StackError::Cancelled));
}

/// Coverage decides which frames reach each pixel, and the product's planes count them. Frame A
/// holds 10 and covers both pixels, frame B holds 40 and covers pixel 0 alone, and frame C holds
/// 10 with no quality planes, which is full support. Unit weights and unit noise, mean combine:
///   px0: A, B, C → 60/3 = 20, coverage 3/3, weight 3, inverse variance 3²/(3·1) = 3
///   px1: A, C    → 20/2 = 10, coverage 2/3, weight 2, inverse variance 2²/(2·1) = 2
/// B taken in at px1 would read 20 there. A pixel no frame covers is the warp border's 0.
#[test]
fn coverage_decides_which_frames_reach_each_pixel() {
    let dims = ImageDimensions::new((2, 1), 1);
    let config = StackConfig {
        combine: Combine::mean(),
        normalization: Normalization::None,
        weighting: Weighting::Equal,
        ..StackConfig::light()
    };
    let covered = |value: f32, coverage: [f32; 2]| {
        with_noise(
            stack_frame(
                LinearImage::from_pixels(dims, vec![value; 2]),
                FrameQuality::from_coverage(Buffer2::new(2, 1, coverage.to_vec())),
            ),
            1.0,
        )
    };
    let product = combine(
        vec![
            covered(10.0, [1.0, 1.0]),
            covered(40.0, [1.0, 0.0]),
            with_noise(
                StackFrame::from(LinearImage::from_pixels(dims, vec![10.0; 2])),
                1.0,
            ),
        ],
        &config.clone(),
    )
    .unwrap();
    assert_eq!(product.image.channel(0).pixels(), &[20.0, 10.0]);
    assert_eq!(
        product.coverage.as_ref().unwrap().to_plane(0).pixels(),
        &[1.0, 2.0 / 3.0]
    );
    assert_eq!(
        product.weight.as_ref().unwrap().channel(0).pixels(),
        &[3.0, 2.0]
    );
    assert_eq!(
        product
            .inverse_variance
            .as_ref()
            .unwrap()
            .channel(0)
            .pixels(),
        &[3.0, 2.0]
    );

    let alone = combine(vec![covered(10.0, [1.0, 0.0])], &config).unwrap();
    assert_eq!(alone.image.channel(0).pixels(), &[10.0, 0.0]);
    assert_eq!(
        alone.coverage.as_ref().unwrap().to_plane(0).pixels(),
        &[1.0, 0.0]
    );
    // The uncovered pixel holds no information, not an exact value.
    assert_eq!(
        alone.inverse_variance.as_ref().unwrap().channel(0).pixels(),
        &[1.0, 0.0]
    );
}

/// Three frames covering pixels 2 and 3 alone. Their stated medians 12, 24, 31 and MADs 2, 4, 1
/// make frame 2 the reference and the multiplicative gains 31/12, 31/24 and 1. As combined, each
/// frame's σ is its gain times its noise, 1.4826 × MAD here — 31/6, 31/6 and 1 in units of 1.4826 —
/// so the inverse-variance weights stand 36 : 36 : 961, out of 1033. Pixel 2 is
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
                medians: [median].into_iter().collect(),
                noise: [mad_to_sigma(mad)].into_iter().collect(),
                read_share: [0.0; 3].into_iter().collect(),
                sky: [median].into_iter().collect(),
                quantization_sigma: None,
                electrons_per_unit: None,
                facts: FrameFacts {
                    domain: None,
                    row_order: None,
                    cfa_type: None,
                    saturation_flagged: false,
                    conditions: CaptureConditions::default(),
                    unverified_dark: UnverifiedConditions::NONE,
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
        assert_eq!(norms[0].slots[0].gain, 31.0 / 12.0);
        assert_eq!(norms[1].slots[0].gain, 31.0 / 24.0);
        assert_eq!(norms[2].slots[0].gain, 1.0);
        assert!(norms.iter().all(|norm| norm.slots[0].offset == 0.0));

        // σ, its gain, square, inverse, the sum and the quotient: 7 f32 roundings, 4ε relative.
        let weights = FrameWeights::resolve(
            &Weighting::Noise,
            source_stats(cache),
            Some(norms),
            Slots::new(None, 1),
        )
        .unwrap()
        .unwrap();
        let total: f32 = (0..3).map(|frame| weights.weight(frame, 0)).sum();
        for (frame, expected) in [36.0, 36.0, 961.0].into_iter().enumerate() {
            let expected = expected / 1033.0;
            assert_close!(
                weights.weight(frame, 0) / total,
                expected,
                4.0 * f64::from(f32::EPSILON) * expected
            );
        }
    }

    let config = StackConfig {
        combine: Combine::mean(),
        weighting: Weighting::Noise,
        normalization: Normalization::Multiplicative,
        ..StackConfig::light()
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

/// Two warped frames that share no pixel cannot be normalized, but combine without it: each
/// pixel is the one frame that reached it, and the third, which neither reached, is 0 and flagged
/// `NO_DATA` although neither frame carries a flag.
#[test]
fn only_normalization_requires_common_coverage() {
    let dims = ImageDimensions::new((3, 1), 1);
    let frames = || {
        vec![
            stack_frame(
                LinearImage::from_pixels(dims, vec![1.0, 2.0, 5.0]),
                FrameQuality::from_coverage(Buffer2::new(3, 1, vec![1.0, 0.0, 0.0])),
            ),
            stack_frame(
                LinearImage::from_pixels(dims, vec![3.0, 4.0, 6.0]),
                FrameQuality::from_coverage(Buffer2::new(3, 1, vec![0.0, 1.0, 0.0])),
            ),
        ]
    };
    let error = combine(
        frames(),
        &StackConfig {
            normalization: Normalization::Global,
            weighting: Weighting::Equal,
            ..StackConfig::light()
        },
    )
    .unwrap_err();
    assert!(matches!(error, StackError::NoCommonCoverage));

    let product = combine(
        frames(),
        &StackConfig {
            normalization: Normalization::None,
            weighting: Weighting::Equal,
            ..StackConfig::light()
        },
    )
    .unwrap();
    assert_eq!(product.image.channel(0).pixels(), &[1.0, 4.0, 0.0]);
    let flags = product.image.flags.as_ref().unwrap();
    assert_eq!(
        (0..3).map(|index| flags.at(index)).collect::<Vec<_>>(),
        [
            QualityFlags::default(),
            QualityFlags::default(),
            QualityFlags::NO_DATA
        ]
    );
}

#[test]
fn confidence_scales_a_samples_noise_rather_than_its_weight() {
    // px0: A (q 1, val 10) + B (q .5, val 20) → 30/2 = 15: B's half confidence leaves its weight
    // alone and doubles its sample's variance, 1/q = 2, so the inverse variance is
    // 2²/(1²·1 + 1²·2) = 4/3. At px1 B has no support at all, which is what does exclude a frame:
    // A alone, inverse variance 1, coverage 1/2. Every sum is exact; each figure rounds once.
    let dims = ImageDimensions::new((2, 1), 1);
    let a = LinearImage::from_pixels(dims, vec![10.0, 10.0]);
    let b = LinearImage::from_pixels(dims, vec![20.0, 20.0]);
    let config = StackConfig {
        combine: Combine::mean(),
        normalization: Normalization::None,
        weighting: Weighting::Equal,
        ..StackConfig::light()
    };
    let frames = vec![
        with_noise(
            stack_frame(
                a,
                FrameQuality::Planes {
                    coverage: Buffer2::new(2, 1, vec![1.0, 1.0]),
                    confidence: Buffer2::new(2, 1, vec![1.0, 1.0]),
                },
            ),
            1.0,
        ),
        with_noise(
            stack_frame(
                b,
                FrameQuality::Planes {
                    coverage: Buffer2::new(2, 1, vec![1.0, 0.0]),
                    confidence: Buffer2::new(2, 1, vec![0.5, 0.0]),
                },
            ),
            1.0,
        ),
    ];
    let product = combine(frames, &config).unwrap();
    assert_eq!(product.image.channel(0).pixels(), &[15.0, 10.0]);
    assert_eq!(
        product.coverage.as_ref().unwrap().to_plane(0).pixels(),
        &[1.0, 0.5]
    );
    assert_eq!(
        product.weight.as_ref().unwrap().channel(0).pixels(),
        &[2.0, 1.0]
    );
    assert_eq!(
        product
            .inverse_variance
            .as_ref()
            .unwrap()
            .channel(0)
            .pixels(),
        &[4.0 / 3.0, 1.0]
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
        combine: Combine::mean(),
        normalization: Normalization::None,
        weighting: Weighting::Equal,
        ..StackConfig::light()
    };

    for method in InterpolationMethod::ALL {
        let warped = resample::warp(
            &source,
            &transform,
            registration_config::internals::warp_params(method),
        );
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
    let mads = [&a, &b]
        .map(|image| f64::from(MedianMad::of_mut(&mut image.channel(0).pixels().to_vec()).mad));
    let params = registration_config::internals::warp_params(InterpolationMethod::Bilinear);
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
    let gain = f64::from(norms[1].slots[0].gain);

    let truth = SkyField::render(size, sky, 1.5, &stars, 0).pixels;
    let mean = truth.pixels().iter().map(|&t| f64::from(t)).sum::<f64>() / truth.len() as f64;
    let spread: f64 = truth
        .pixels()
        .iter()
        .map(|&t| (0.8 * (f64::from(t) - mean)).powi(2))
        .sum();
    let standard_error = ((0.002f64.powi(2) + 1.25f64.powi(2) * 0.006f64.powi(2)) / spread).sqrt();
    assert_eq!(norms[0].slots[0].gain, 1.0, "frame A is the reference");
    assert_close!(gain, 1.25, 5.0 * standard_error);
    assert!(
        (mads[0] / mads[1] - 1.25).abs() > 100.0 * standard_error,
        "premise: the sky spreads alone must miss the gain"
    );
}

/// A half-pixel bilinear warp averages four source pixels equally, so its confidence `1/Σw²` is 4,
/// and an identity warp's is 1. The two frames share one source and so one noise level `σ²` and one
/// weight `w`. The confidence divides each sample's variance and leaves the weights alone: the
/// product's weight is `2w`, and its inverse variance `(2w)²/(w²·σ² + w²·σ²/4) = 1/(0.3125·σ²)`.
#[test]
fn registered_confidence_divides_the_noise_and_leaves_the_weight() {
    let dims = ImageDimensions::new((64, 48), 1);
    let mut rng = TestRng::new(0x1234_5678);
    let pixels = (0..dims.pixel_count())
        .map(|_| rng.next_f32() - 0.5)
        .collect();
    let source = LinearImage::from_pixels(dims, pixels);
    let params = registration_config::internals::warp_params(InterpolationMethod::Bilinear);
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
    let weights = FrameWeights::resolve(
        &Weighting::Noise,
        source_stats(&cache),
        None,
        Slots::new(None, 1),
    )
    .unwrap()
    .unwrap();
    let base_weights = [weights.weight(0, 0), weights.weight(1, 0)];
    assert_eq!(base_weights[0], base_weights[1]);

    let pixel = 12 * dims.width() + 12;
    let identity_confidence = cache.frames[0]
        .quality
        .confidence(0)
        .unwrap()
        .chunk(pixel, pixel + 1)[0];
    let half_pixel_confidence = cache.frames[1]
        .quality
        .confidence(0)
        .unwrap()
        .chunk(pixel, pixel + 1)[0];
    assert_eq!(identity_confidence, 1.0);
    assert_eq!(half_pixel_confidence, 4.0);
    let sigma_squared = source_stats(&cache)
        .next()
        .unwrap()
        .ccd_noise(0)
        .background_at(1.0);

    let product = run_stacking(
        &cache,
        &StackConfig {
            combine: Combine {
                method: CombineMethod::Mean(Rejection::None),
                small_n: SmallN::none(),
            },
            weighting: Weighting::Noise,
            normalization: Normalization::None,
            ..StackConfig::light()
        },
    )
    .expect("this cache is never cancelled");
    assert_eq!(
        product.weight.as_ref().unwrap().channel(0)[pixel],
        2.0 * base_weights[0]
    );
    let expected = 1.0 / (0.3125 * sigma_squared);
    assert_close!(
        product.inverse_variance.as_ref().unwrap().channel(0)[pixel],
        expected,
        2.0 * f32::EPSILON * expected
    );
}

/// A clipped pixel's dispersion reads the frames' full scatter: its survivors scatter as a Gaussian
/// truncated to the band, so the sum of squares is divided by that variance, 0.9733369 at ±3σ.
/// Six frames at 0, 1, …, 5 lie within ±3 of the MAD σ, 2.645, about 2.5, so nothing is clipped:
/// the uncorrected dispersion is Σ(x − x̄)² / ((n − 1)·n) = 17.5 / 30, and the clip's is that
/// times 1.0273935. A plain mean cuts nothing, and three frames, as many as `min_survivors`, are
/// never clipped: both keep the uncorrected figure, 1/3 for 0, 1, 2.
#[test]
fn a_clipped_pixels_dispersion_reads_the_full_scatter() {
    let dims = ImageDimensions::new((1, 1), 1);
    let dispersion = |count: usize, rejection: Rejection| {
        let frames = (0..count)
            .map(|i| StackFrame::from(LinearImage::from_pixels(dims, vec![i as f32])))
            .collect();
        let stacked = combine(
            frames,
            &StackConfig {
                combine: Combine {
                    method: CombineMethod::Mean(rejection),
                    small_n: SmallN::none(),
                },
                quality: QualityPlanes::ALL,
                weighting: Weighting::Equal,
                normalization: Normalization::None,
                ..StackConfig::light()
            },
        )
        .unwrap();
        f64::from(stacked.dispersion.unwrap().channel(0)[0])
    };
    let full = 17.5 / 30.0;
    let clipped = dispersion(6, Rejection::sigma_clip(3.0));
    let expected = full * 1.027_393_469_477_902;
    assert!((clipped - expected).abs() <= 1e-6 * expected, "{clipped}");
    assert!((dispersion(6, Rejection::None) - full).abs() <= 1e-6 * full);
    assert!((dispersion(3, Rejection::sigma_clip(3.0)) - 1.0 / 3.0).abs() <= 1e-6);
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
        combine: Combine::mean(),
        quality: QualityPlanes::ALL,
        weighting: Weighting::Equal,
        normalization: Normalization::None,
        ..StackConfig::light()
    });
    assert!(all.coverage.is_some());
    assert!(all.weight.is_some());
    assert!(all.inverse_variance.is_some());
    assert!(all.dispersion.is_some());

    // The default asks for the standard planes, which leave out the dispersion.
    let standard = stack(StackConfig {
        combine: Combine::mean(),
        weighting: Weighting::Equal,
        normalization: Normalization::None,
        ..StackConfig::light()
    });
    assert!(standard.inverse_variance.is_some() && standard.dispersion.is_none());

    // A median is not a linear combination, so its variance plane is absent even though the
    // request asked for it — and is never allocated, not allocated and cleared.
    let median = stack(StackConfig {
        combine: Combine {
            method: CombineMethod::Median,
            small_n: SmallN::none(),
        },
        quality: QualityPlanes::ALL,
        weighting: Weighting::Equal,
        normalization: Normalization::None,
        ..StackConfig::light()
    });
    assert!(median.coverage.is_some());
    assert!(median.weight.is_some(), "a median still reports weight");
    assert!(
        median.inverse_variance.is_none() && median.dispersion.is_none(),
        "a median has no linear-combine variance factor and no dispersion"
    );

    // Image only: no ancillary plane survives, whatever the method would support.
    let bare = stack(StackConfig {
        combine: Combine::mean(),
        quality: QualityPlanes::IMAGE_ONLY,
        weighting: Weighting::Equal,
        normalization: Normalization::None,
        ..StackConfig::light()
    });
    assert!(bare.coverage.is_none());
    assert!(bare.weight.is_none());
    assert!(bare.inverse_variance.is_none() && bare.dispersion.is_none());
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
        combine: Combine {
            method: CombineMethod::Mean(Rejection::sigma_clip(2.5)),
            small_n: SmallN::median_below(5),
        },
        normalization: Normalization::None,
        weighting: Weighting::Equal,
        ..StackConfig::light()
    };
    let edge = combine(frames, &config).unwrap();
    assert_eq!(edge.image.channel(0).pixels(), &[0.125]);

    let uncovered: Vec<StackFrame> = [0.125, 0.125, 0.0, 0.0, 0.0]
        .iter()
        .map(|&v| LinearImage::from_pixels(dims, vec![v]).into())
        .collect();
    let mean = StackConfig {
        combine: Combine::mean(),
        normalization: Normalization::None,
        weighting: Weighting::Equal,
        ..StackConfig::light()
    };
    let dark = combine(uncovered, &mean).unwrap();
    assert_eq!(dark.image.channel(0).pixels(), &[0.05]);
}

/// Noise weights read a demosaiced frame's mosaic noise, which its own pixels cannot show: one
/// pixel has no noise to measure, and weighting refuses the frames without it. Frame A reads 1
/// with σ 1 in every channel; frame B reads 3 with σ 1, 2 and 1/2. The weights are 1/σ²: red
/// (1 + 3)/2 = 2, green (1 + 3/4)/(1 + 1/4) = 1.4, blue (1 + 12)/(1 + 4) = 2.6. The master was
/// made by the combine and carries no mosaic noise.
#[test]
fn noise_weights_read_a_demosaiced_frames_mosaic_noise() {
    let dims = ImageDimensions::new((1, 1), 3);
    let frame = |value: f32, sigma: Option<[f32; 3]>| {
        let mut image = LinearImage::from_pixels(dims, vec![value; 3]);
        image.metadata.mosaic_noise = sigma.map(|sigma| MosaicNoise {
            sigma,
            read_share: [0.0; 3],
            sky: [value; 3],
            quantization_sigma: None,
        });
        StackFrame::from(image)
    };
    let config = StackConfig {
        combine: Combine {
            method: CombineMethod::Mean(Rejection::None),
            small_n: SmallN::none(),
        },
        weighting: Weighting::Noise,
        normalization: Normalization::None,
        ..StackConfig::light()
    };
    let product = combine(
        vec![
            frame(1.0, Some([1.0; 3])),
            frame(3.0, Some([1.0, 2.0, 0.5])),
        ],
        &config,
    )
    .unwrap();
    for (channel, expected) in [2.0f32, 1.4, 2.6].into_iter().enumerate() {
        assert_close!(
            product.image.channel(channel).pixels()[0],
            expected,
            f32::EPSILON * expected,
            "channel {channel}"
        );
    }
    assert_eq!(product.image.metadata.mosaic_noise, None);
    assert!(matches!(
        combine(vec![frame(1.0, None), frame(3.0, None)], &config),
        Err(StackError::NoNoiseToWeigh { index: 0 })
    ));
}

#[test]
fn rejection_emits_channel_shaped_survivor_weight_and_inverse_variance() {
    // Trimming removes each channel's high value, hence a different source frame: R keeps f0/f1, G
    // keeps f1/f2, B keeps f0/f2. Manual weights [1,2,3] stay as given, and every frame has unit
    // noise, so the inverse variance is (Σw)²/Σw²: R 9/5, G 25/13, B 16/10. Three frames leave two
    // survivors, so the minimum comes down to 2. The dispersion is Σw(x − x̄)² / ((n − 1)·Σw): R's
    // 1 and 2 lie −2/3 and 1/3 about 5/3, so (4/9 + 2/9) / 3 = 2/9; G's 2 and 3 lie −0.6 and 0.4
    // about 2.6, so (0.72 + 0.48) / 5 = 6/25; B's 1 and 3 lie −1.5 and 0.5 about 2.5, so (2.25 +
    // 0.75) / 4 = 3/4.
    let dims = ImageDimensions::new((1, 1), 3);
    let frame = |pixels: Vec<f32>| with_noise(LinearImage::from_pixels(dims, pixels).into(), 1.0);
    let frames = vec![
        frame(vec![1.0, 100.0, 1.0]),
        frame(vec![2.0, 2.0, 100.0]),
        frame(vec![100.0, 3.0, 3.0]),
    ];
    let config = StackConfig {
        combine: Combine {
            method: CombineMethod::Mean(Rejection::Trim(TrimConfig::new(0.0, 34.0))),
            small_n: SmallN::none(),
        },
        weighting: Weighting::Manual(vec![1.0, 2.0, 3.0]),
        normalization: Normalization::None,
        min_survivors: 2,
        quality: QualityPlanes::ALL,
    };

    let result = combine(frames, &config).unwrap();

    assert_eq!(result.coverage.as_ref().unwrap()[0], 1.0);
    let expected_values: [f64; 3] = [5.0 / 3.0, 13.0 / 5.0, 5.0 / 2.0];
    let expected_dispersions: [f64; 3] = [2.0 / 9.0, 6.0 / 25.0, 3.0 / 4.0];
    let expected_weights: [f64; 3] = [3.0, 5.0, 4.0];
    let expected_inverse_variances: [f64; 3] = [9.0 / 5.0, 25.0 / 13.0, 16.0 / 10.0];
    let inverse_variance = result.inverse_variance.as_ref().unwrap();
    assert!(matches!(&result.weight, Some(QualityMap::PerChannel(_))));
    assert!(matches!(inverse_variance, QualityMap::PerChannel(_)));
    // Each figure is an exact quotient of exact sums, rounded once: within half an ulp, ε relative.
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
            inverse_variance.channel(channel)[0],
            expected_inverse_variances[channel],
            "inverse variance",
        );
        // Five roundings, each within half an ulp: two products, a sum, a product and the
        // quotient. The mean's own rounding moves the sum only to second order, since a weighted
        // sum of squares is least at the weighted mean.
        assert_close!(
            result.dispersion.as_ref().unwrap().channel(channel)[0],
            expected_dispersions[channel],
            4.0 * f64::from(f32::EPSILON) * expected_dispersions[channel],
            "channel {channel} dispersion"
        );
    }
    assert_ne!(
        result.weight.as_ref().unwrap().channel(0)[0],
        result.weight.as_ref().unwrap().channel(1)[0]
    );
    assert_ne!(
        inverse_variance.channel(1)[0],
        inverse_variance.channel(2)[0]
    );
}

/// On frames whose noise is what their model says, the dispersion and the reciprocal of the inverse
/// variance estimate the same figure, `1 / Σwᵢ` for inverse-variance weights. Eight frames of a
/// flat 0.5 with Gaussian noise of σ from 0.01 to 0.03, weighted by `1/σ²`: each pixel's dispersion
/// is that figure times a χ² of 7 degrees of freedom over 7, so the mean of 4096 independent pixels
/// is within `√(2 / (7·4096))` = 0.84% of it at one σ, and 4.2% at five. A model that halves every
/// σ quarters the variance — so quadruples the inverse variance — and leaves the dispersion alone,
/// so the two then differ by 4.
#[test]
fn dispersion_agrees_with_the_variance_where_the_model_holds() {
    const SIDE: usize = 64;
    const SIGMAS: [f32; 8] = [0.01, 0.02, 0.01, 0.03, 0.015, 0.02, 0.025, 0.01];
    let dims = ImageDimensions::new((SIDE, SIDE), 1);
    let mut rng = TestRng::new(17);
    let pixels: Vec<Vec<f32>> = SIGMAS
        .iter()
        .map(|&sigma| {
            (0..SIDE * SIDE)
                .map(|_| 0.5 + sigma * rng.next_gaussian_f32())
                .collect()
        })
        .collect();
    let config = StackConfig {
        combine: Combine::mean(),
        weighting: Weighting::Manual(SIGMAS.iter().map(|sigma| 1.0 / (sigma * sigma)).collect()),
        normalization: Normalization::None,
        quality: QualityPlanes::ALL,
        ..StackConfig::light()
    };
    let inverse_variance: f64 = SIGMAS
        .iter()
        .map(|&sigma| 1.0 / f64::from(sigma * sigma))
        .sum();
    for (model_scale, expected_ratio) in [(1.0f32, 1.0f64), (0.5, 4.0)] {
        let frames: Vec<StackFrame> = pixels
            .iter()
            .zip(SIGMAS)
            .map(|(pixels, sigma)| {
                with_noise(
                    LinearImage::from_pixels(dims, pixels.clone()).into(),
                    sigma * model_scale,
                )
            })
            .collect();
        let product = combine(frames, &config).unwrap();
        let mean = |plane: &QualityMap| {
            plane
                .channel(0)
                .pixels()
                .iter()
                .map(|&value| f64::from(value))
                .sum::<f64>()
                / (SIDE * SIDE) as f64
        };
        let dispersion = mean(product.dispersion.as_ref().unwrap());
        let model_inverse = mean(product.inverse_variance.as_ref().unwrap());
        assert_close!(dispersion * inverse_variance, 1.0, 0.042, "dispersion");
        assert_close!(
            dispersion * model_inverse,
            expected_ratio,
            0.042 * expected_ratio,
            "model σ × {model_scale}"
        );
    }
}

#[test]
fn median_quality_uses_equal_weights_and_has_no_variance() {
    let dims = ImageDimensions::new((8, 1), 1);
    let mk = |base: f32, spread: f32| -> Vec<f32> {
        (0..8).map(|i| base + i as f32 * spread / 7.0).collect()
    };
    let frames = || -> Vec<StackFrame> {
        [mk(100.0, 1.0), mk(100.0, 20.0), mk(100.0, 2.0)]
            .into_iter()
            .map(|pixels| with_noise(LinearImage::from_pixels(dims, pixels).into(), 1.0))
            .collect()
    };
    let stack = |config: &StackConfig| combine(frames(), config).unwrap();

    let explicit = stack(&StackConfig {
        combine: Combine {
            method: CombineMethod::Median,
            small_n: SmallN::median_below(5),
        },
        weighting: Weighting::Noise,
        normalization: Normalization::None,
        ..StackConfig::light()
    });
    assert!(explicit.inverse_variance.is_none());
    // The middle frame is the median at every pixel, sample for sample.
    assert_eq!(explicit.image.channel(0).pixels(), mk(100.0, 2.0));
    assert_eq!(
        explicit.coverage.as_ref().unwrap().to_plane(0).pixels(),
        &[1.0; 8]
    );
    assert_eq!(
        explicit.weight.as_ref().unwrap().channel(0).pixels(),
        &[3.0; 8],
        "median quality must count unit-confidence contributors"
    );

    for (name, config) in [
        (
            "default",
            StackConfig {
                weighting: Weighting::Equal,
                normalization: Normalization::None,
                ..StackConfig::light()
            },
        ),
        (
            "sigma",
            StackConfig {
                combine: Combine::sigma_clipped(2.5),
                weighting: Weighting::Equal,
                normalization: Normalization::None,
                ..StackConfig::light()
            },
        ),
        (
            "linear fit",
            StackConfig {
                combine: Combine::linear_fit(3.0),
                weighting: Weighting::Equal,
                normalization: Normalization::None,
                ..StackConfig::light()
            },
        ),
        (
            "GESD",
            StackConfig {
                combine: Combine::gesd(),
                weighting: Weighting::Equal,
                normalization: Normalization::None,
                ..StackConfig::light()
            },
        ),
        ("flat", StackConfig::flat()),
        ("light", StackConfig::light()),
        (
            "manual weighting",
            StackConfig {
                weighting: Weighting::Manual(vec![1.0, 2.0, 3.0]),
                normalization: Normalization::None,
                ..StackConfig::light()
            },
        ),
    ] {
        let downgraded = stack(&config);
        assert!(
            downgraded.inverse_variance.is_none(),
            "{name} must expose no variance after its small-N median downgrade"
        );
        assert_eq!(
            downgraded.weight.as_ref().unwrap().channel(0).pixels(),
            &[3.0; 8],
            "{name} median fallback must count unit-confidence contributors"
        );
    }

    let linear_fallback = stack(&StackConfig {
        combine: Combine {
            method: CombineMethod::Mean(Rejection::sigma_clip(2.5)),
            small_n: SmallN {
                min_frames: 4,
                fallback: CombineMethod::Mean(Rejection::None),
            },
        },
        weighting: Weighting::Equal,
        normalization: Normalization::None,
        ..StackConfig::light()
    });
    assert_eq!(
        linear_fallback
            .inverse_variance
            .unwrap()
            .channel(0)
            .pixels(),
        &[3.0; 8]
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
        combine: Combine::mean(),
        normalization: Normalization::None,
        weighting: Weighting::Equal,
        ..StackConfig::light()
    };
    let ingest = IngestConfig {
        memory_override: Some(1), // forces disk-backed (mmap) storage
        ..IngestConfig::with_cache_dir(temp_dir.join("cache"))
    };
    let result = stack(
        &paths,
        &config,
        &ingest,
        ProgressCallback::default(),
        CancelToken::never(),
    )
    .unwrap()
    .image;
    assert_eq!(result.channel(0).pixels(), &[20.0; 16]);
}

/// Noise weighting needs a measured noise: constant frames have none, and the combine names the
/// first such frame rather than falling back to equal weights.
#[test]
fn noise_weighting_refuses_a_frame_with_no_noise() {
    let dims = ImageDimensions::new((16, 1), 1);
    let cache = FrameCache::from_images(
        vec![LinearImage::from_pixels(dims, vec![0.5; 16]); 3],
        Normalization::None,
    );
    let config = StackConfig {
        combine: Combine::mean(),
        weighting: Weighting::Noise,
        normalization: Normalization::None,
        ..StackConfig::light()
    };
    assert!(matches!(
        run_stacking(&cache, &config),
        Err(StackError::NoNoiseToWeigh { index: 0 })
    ));
}

/// Noise weights survive the rejection: pixel 0 holds 100, 90 and 999 across three frames, the
/// first two with noise 1/16 and 4. Sigma clipping at 2σ — median 100, MAD 10, σ = 10 · 1.4826 ·
/// 1.4869, so a band of ±44.1 — drops the 999, and the two left average under weights in the
/// ratio (4·16)² = 4096 : 1: (4096·100 + 90)/4097. The median of the three, 100, is what the
/// small-stack fallback would give, so `SmallN::none()` and a minimum of 2 survivors are what let
/// the clip run. The weights' roundings move the mean by under 1e-9; the result rounds once, half
/// an ulp of 100.
#[test]
fn noise_weights_survive_rejection() {
    let ramp = |start: f32, spacing: f32| -> Vec<f32> {
        (0..16).map(|i| start + i as f32 * spacing).collect()
    };
    let mut hit = ramp(100.0, 1.0 / 32.0);
    hit[0] = 999.0;
    let dims = ImageDimensions::new((16, 1), 1);
    let mut cache = FrameCache::from_images(
        [ramp(100.0, 1.0 / 64.0), ramp(90.0, 1.0), hit]
            .into_iter()
            .map(|pixels| LinearImage::from_pixels(dims, pixels))
            .collect(),
        Normalization::None,
    );
    for (frame, sigma) in cache.frames.iter_mut().zip([1.0 / 16.0, 4.0, 1.0]) {
        frame.source_stats.noise[0] = sigma;
    }
    let config = StackConfig {
        combine: Combine {
            method: CombineMethod::Mean(Rejection::sigma_clip(2.0)),
            small_n: SmallN::none(),
        },
        weighting: Weighting::Noise,
        min_survivors: 2,
        normalization: Normalization::None,
        ..StackConfig::light()
    };
    let result = run_stacking(&cache, &config).expect("this cache is never cancelled");
    assert_close!(
        result.image.channel(0)[0],
        (4096.0 * 100.0 + 90.0) / 4097.0,
        f64::from(f32::EPSILON) * 100.0 / 2.0
    );
}

/// A flagged sample is left out while `min_survivors` (3) unflagged samples remain at its pixel,
/// and kept otherwise. Ten frames, no rejection, three pixels:
/// - pixel 0: frames 0..3 saturated at 1.0, the rest 0.25 — the mean of the seven is 0.25;
/// - pixel 1: every frame saturated at 1.0 — all are kept, and the stack's pixel is flagged;
/// - pixel 2: frames 0..8 saturated at 0.5, two clean at 0.25 — two is below three, so all ten
///   stay: (8 × 0.5 + 2 × 0.25) / 10 = 0.45, flagged.
///
/// The report counts the 3 left out, and the 10 + 8 kept.
#[test]
fn flagged_samples_are_left_out_while_enough_clean_ones_remain() {
    let dims = ImageDimensions::new((3, 1), 1);
    let frames: Vec<StackFrame> = (0..10)
        .map(|frame| {
            let saturated = [frame < 3, true, frame < 8];
            let values = [
                if saturated[0] { 1.0 } else { 0.25 },
                1.0,
                if saturated[2] { 0.5 } else { 0.25 },
            ];
            let mut image = LinearImage::from_pixels(dims, values.to_vec());
            image.metadata.saturation_flagged = true;
            image.flags = PixelFlags::from_fn(dims.size(), |index| {
                if saturated[index] {
                    QualityFlags::SATURATED
                } else {
                    QualityFlags::default()
                }
            });
            image.into()
        })
        .collect();
    let config = StackConfig {
        combine: Combine::mean(),
        weighting: Weighting::Equal,
        normalization: Normalization::None,
        ..StackConfig::light()
    };
    let product = combine(frames, &config).unwrap();
    let pixels = product.image.channel(0).pixels();
    assert_eq!(pixels[0], 0.25);
    assert_eq!(pixels[1], 1.0);
    assert_close!(pixels[2], 0.45, f32::EPSILON);

    assert!(product.image.metadata.saturation_flagged);
    let flags = product.image.flags.as_ref().unwrap();
    let saturated: Vec<bool> = (0..3)
        .map(|index| flags.at(index).intersects(QualityFlags::SATURATED))
        .collect();
    assert_eq!(saturated, [false, true, true]);
    assert_eq!(product.report.excluded_samples.saturated, 3);
    assert_eq!(product.report.kept_flagged_samples.saturated, 18);
    assert_eq!(product.report.excluded_samples.cosmic_ray, 0);
}

/// A star core that saturates in every frame of a registered stack keeps its clipped value: the
/// warp carries the bound and its flag rather than leaving the pixel out, the reference keeps its
/// own, and with no clean sample at the core the combine keeps all three. An 8×8 field of 0.25 with
/// a 2×2 core of 1.0 at (3..5, 3..5): the reference as it is, a frame warped by the identity, and
/// one whose source sits a column to the left of the field, warped back by a whole pixel. At whole
/// shifts every tap but the centre weighs exactly zero, so each sample is its source pixel:
/// the core reads (1 + 1 + 1)/3 = 1, flagged, and the pixel beside it 0.25, unflagged.
#[test]
fn a_core_saturated_in_every_frame_keeps_its_clipped_value() {
    let dims = ImageDimensions::new((8, 8), 1);
    let core = |x: usize, y: usize| (3..5).contains(&x) && (3..5).contains(&y);
    let field = |offset: usize| {
        let at = |index: usize| (index % 8 + offset, index / 8);
        let mut image = LinearImage::from_pixels(
            dims,
            (0..64)
                .map(|index| {
                    let (x, y) = at(index);
                    if core(x, y) { 1.0 } else { 0.25 }
                })
                .collect(),
        );
        image.metadata.saturation_flagged = true;
        image.flags = PixelFlags::from_fn(dims.size(), |index| {
            let (x, y) = at(index);
            if core(x, y) {
                QualityFlags::SATURATED
            } else {
                QualityFlags::default()
            }
        });
        image
    };
    let params = registration_config::internals::warp_params(InterpolationMethod::Lanczos3);
    let reference = field(0);
    let shifted = field(1);
    let frames = vec![
        stack_frame(
            reference.clone(),
            FrameQuality::for_reference(reference.flags.as_ref()),
        ),
        StackFrame::registered(
            &reference,
            resample::warp(
                &reference,
                &WarpTransform::new(Transform::identity()),
                params,
            ),
        ),
        StackFrame::registered(
            &shifted,
            resample::warp(
                &shifted,
                &WarpTransform::new(Transform::translation(DVec2::new(-1.0, 0.0))),
                params,
            ),
        ),
    ];
    let config = StackConfig {
        combine: Combine::mean(),
        weighting: Weighting::Equal,
        normalization: Normalization::None,
        ..StackConfig::light()
    };
    let product = combine(frames, &config).unwrap();
    let pixels = product.image.channel(0).pixels();
    let flags = product.image.flags.as_ref().unwrap();
    assert_eq!(pixels[3 * 8 + 3], 1.0);
    assert_eq!(flags.at(3 * 8 + 3), QualityFlags::SATURATED);
    assert_eq!(pixels[3 * 8 + 5], 0.25);
    assert_eq!(flags.at(3 * 8 + 5), QualityFlags::default());
}

/// Each sample's noise is its frame's model at the gain its flat multiplied it by. Two frames of 10
/// and 14, each its own sky and unit noise where no flat amplified it, were divided by a flat of
/// 0.5, gain 2. Half of their background is read noise, which the flat amplified twice, half sky,
/// amplified once: `1·2·(½·2 + ½)` = 3 each, so the inverse variance is 2²/(3 + 3) = 2/3, against 2
/// with no flat. All read noise gives 2²/(4 + 4) = 1/2, all sky 2²/(2 + 2) = 1. With 0.5
/// electrons per unit, the first frame's photons above its sky at the combined 12 add
/// `(12 − 10)·2/0.5` = 8, the flat's gain on the source term: 2²/(11 + 3) = 2/7.
#[test]
fn each_samples_noise_reads_the_gain_its_flat_applied() {
    let dims = ImageDimensions::new((2, 1), 1);
    let gain = Arc::new(FlatGain::of_divisor(
        &Buffer2::new(2, 1, vec![0.5; 2]),
        &CfaType::Mono,
        |_| false,
    ));
    let frame = |value: f32, read_share: f32, flat: bool, electrons: Option<f32>| {
        let mut image = LinearImage::from_pixels(dims, vec![value; 2]);
        if flat {
            image.metadata.flat_gain = Some(Arc::clone(&gain));
        }
        let mut frame = with_noise(image.into(), 1.0);
        frame.source_stats.read_share = [read_share].into_iter().collect();
        frame.source_stats.electrons_per_unit = electrons;
        frame
    };
    let config = StackConfig {
        combine: Combine::mean(),
        normalization: Normalization::None,
        weighting: Weighting::Equal,
        ..StackConfig::light()
    };
    for (read_share, flat, electrons, expected) in [
        (0.5, true, None, 2.0 / 3.0),
        (0.5, false, None, 2.0),
        (1.0, true, None, 0.5),
        (0.0, true, None, 1.0),
        (0.5, true, Some(0.5), 2.0 / 7.0),
    ] {
        let product = combine(
            vec![
                frame(10.0, read_share, flat, electrons),
                frame(14.0, read_share, flat, electrons),
            ],
            &config,
        )
        .unwrap();
        assert_eq!(
            product
                .inverse_variance
                .as_ref()
                .unwrap()
                .channel(0)
                .pixels(),
            &[expected; 2],
            "ρ {read_share}, flat {flat}, electrons {electrons:?}"
        );
    }
}

/// The inverse variance plane takes each frame's CCD model at the combined value. Frames of 10 and
/// 14, each its own sky, with unit background noise and 0.5 electrons per unit, combine to 12: the
/// first frame's model adds (12 − 10)/0.5 = 4 of photon noise above its sky, the second's adds none
/// below its own, so the inverse variance is 2²/(5 + 1) = 2/3. With the second frame's gain unknown
/// the figure is the same here, but the report says the plane lacks a source term.
#[test]
fn the_inverse_variance_plane_carries_the_source_term_above_the_sky() {
    let dims = ImageDimensions::new((2, 1), 1);
    let frame = |value: f32, electrons: Option<f32>| {
        let mut frame = with_noise(LinearImage::from_pixels(dims, vec![value; 2]).into(), 1.0);
        frame.source_stats.electrons_per_unit = electrons;
        frame
    };
    let config = StackConfig {
        combine: Combine::mean(),
        normalization: Normalization::None,
        weighting: Weighting::Equal,
        ..StackConfig::light()
    };
    let known = combine(
        vec![frame(10.0, Some(0.5)), frame(14.0, Some(0.5))],
        &config,
    )
    .unwrap();
    assert_eq!(known.image.channel(0).pixels(), &[12.0; 2]);
    assert_eq!(
        known.inverse_variance.as_ref().unwrap().channel(0).pixels(),
        &[2.0 / 3.0; 2]
    );
    assert!(!known.report.variance_background_only);

    let unknown = combine(vec![frame(10.0, Some(0.5)), frame(14.0, None)], &config).unwrap();
    assert_eq!(
        unknown
            .inverse_variance
            .as_ref()
            .unwrap()
            .channel(0)
            .pixels(),
        &[2.0 / 3.0; 2]
    );
    assert!(unknown.report.variance_background_only);
}
