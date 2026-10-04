use crate::combine::cache::*;
use crate::combine::config::DEFAULT_MIN_SURVIVORS;
use crate::combine::config::Weighting;
use crate::combine::rejection::Rejection;
use crate::frame_store::frame_quality::{FramePlane, FrameQuality};
use crate::frame_store::frame_stats::FrameStats;
use crate::frame_store::run_scratch::RunScratch;
use crate::internals::cfa::make_cfa;
use crate::internals::prelude::*;
use crate::io::image::cfa::CfaType;
use crate::io::raw::demosaic::bayer::CfaPattern;
use crate::math::statistics;
use crate::memory::run_memory::RunMemory;
use common::TempDir;

/// A mono request with the default survivor minimum, no weights and no noise.
fn request(planes: QualityPlanes) -> CombineRequest<'static> {
    CombineRequest {
        weights: None,
        planes,
        min_survivors: DEFAULT_MIN_SURVIVORS,
        noise: None,
        slots: Slots::new(None, 1),
    }
}

/// `cache` with every frame measured to have unit noise in every slot, so a sample's variance is
/// one over its confidence.
fn with_unit_noise(mut cache: FrameCache) -> FrameCache {
    for frame in &mut cache.frames {
        for noise in &mut frame.source_stats.noise {
            *noise = 1.0;
        }
    }
    cache
}

#[test]
fn unrequested_quality_planes_are_never_allocated() {
    // A plane the request declines is never built by the reducer, which only the combine's own
    // output shows, before `finish_product` shapes it.
    let dims = ImageDimensions::new((4, 2), 1);
    let cache = FrameCache::from_images(
        vec![
            LinearImage::from_pixels(dims, vec![1.0; 8]),
            LinearImage::from_pixels(dims, vec![3.0; 8]),
        ],
        Normalization::None,
    );
    let reduce = |samples: PixelSamples<'_>, _: &mut ScratchBuffers| {
        let PixelSamples {
            values, weights, ..
        } = samples;
        let count = values.len();
        CombinedSample::from_survivors(
            values.iter().sum::<f32>() / count as f32,
            values,
            weights,
            0..count,
            None,
        )
    };

    let weight_only = cache.process_chunked(
        request(QualityPlanes {
            variance: false,
            dispersion: false,
            ..QualityPlanes::ALL
        }),
        reduce,
    );
    assert!(weight_only.weight.is_some());
    assert!(
        weight_only.variance.is_none() && weight_only.dispersion.is_none(),
        "a variance or dispersion plane was allocated for a combine that did not ask for one"
    );

    let bare = cache.process_chunked(request(QualityPlanes::IMAGE_ONLY), reduce);
    assert!(bare.weight.is_none());
    assert!(bare.variance.is_none() && bare.dispersion.is_none());

    // Skipping the planes must not disturb the combined pixels. Two equal-weight samples, 1 and 3,
    // about their mean 2: (1 + 1) / ((2 − 1)·2) = 1.
    let all = cache.process_chunked(request(QualityPlanes::ALL), reduce);
    assert!(
        all.dispersion
            .as_ref()
            .unwrap()
            .channel(0)
            .pixels()
            .iter()
            .all(|&dispersion| dispersion == 1.0)
    );
    assert_eq!(
        bare.pixels.channel(0).pixels(),
        all.pixels.channel(0).pixels()
    );
}

#[test]
fn quality_plane_request_drops_variance_for_a_non_linear_combine() {
    assert_eq!(
        QualityPlanes::ALL.resolve(false),
        QualityPlanes::STANDARD.resolve(false),
        "a reducer that is not a weighted mean reports no variance factor and no dispersion"
    );
    assert_eq!(
        QualityPlanes::ALL.resolve(false),
        QualityPlanes {
            variance: false,
            dispersion: false,
            ..QualityPlanes::ALL
        },
    );
    assert_eq!(QualityPlanes::ALL.resolve(true), QualityPlanes::ALL);
    assert_eq!(
        QualityPlanes::IMAGE_ONLY.resolve(true),
        QualityPlanes::IMAGE_ONLY,
        "resolving never adds a plane the caller declined"
    );
}

/// Every frame of `frames` through the per-frame checks, in order.
fn validate_frames(frames: &[StoredFrame], dimensions: ImageDimensions) -> Result<(), StackError> {
    let mut facts = SetFacts::default();
    for (index, frame) in frames.iter().enumerate() {
        FrameCheck {
            index,
            cancel: &CancelToken::never(),
        }
        .stored(frame, dimensions, &mut facts)?;
    }
    Ok(())
}

/// The per-frame geometry check names the frame and the plane. It guards every stored plane a
/// caller hands in; the pipeline's own frames hold it as a contract, asserted in debug builds.
#[test]
fn stored_frames_of_the_wrong_shape_are_rejected_not_sliced() {
    // Every read of a stored plane slices it to the cache's pixel count, so a frame that does not
    // match would fault out of a slice index naming neither the frame nor the field. The geometry
    // check runs first and names both.
    let dimensions = ImageDimensions::new((4, 2), 1);
    let core = || CacheCore::plain(CacheTier::Resident, dimensions);
    let frame = |pixels: usize| {
        let image =
            LinearImage::from_pixels(ImageDimensions::new((pixels, 1), 1), vec![1.0; pixels]);
        let stats = FrameStats::measure(&image);
        StoredFrame::from_memory(image, FrameQuality::None, stats)
    };

    // Short channel plane: 4 samples where the cache wants 8.
    let error = validate_frames(&[frame(8), frame(4)], dimensions).unwrap_err();
    assert!(
        matches!(
            error,
            StackError::StoredFramePlaneSamples {
                index: 1,
                plane: FramePlane::Channel,
                expected: 8,
                actual: 4,
            }
        ),
        "expected a geometry error naming frame 1, got {error:?}"
    );

    // A quality plane of the wrong length is caught the same way.
    let image = LinearImage::from_pixels(dimensions, vec![1.0; 8]);
    let stats = FrameStats::measure(&image);
    let short_coverage = StoredFrame::from_memory(
        image,
        FrameQuality::from_coverage(Buffer2::new(2, 1, vec![1.0; 2])),
        stats,
    );
    let error = validate_frames(&[short_coverage], dimensions).unwrap_err();
    assert!(
        matches!(
            error,
            StackError::StoredFramePlaneSamples {
                plane: FramePlane::Coverage,
                expected: 8,
                actual: 2,
                ..
            }
        ),
        "expected a coverage geometry error, got {error:?}"
    );

    // A correctly shaped set passes, and builds.
    assert!(validate_frames(&[frame(8), frame(8)], dimensions).is_ok());
    assert!(
        FrameCache::from_stored_frames(vec![frame(8), frame(8)], core(), Normalization::None)
            .is_ok()
    );
}

/// A stored frame that breaks the geometry the pipeline promised is a bug in the pipeline, which
/// a debug build reports at the cache.
#[test]
#[cfg(debug_assertions)]
#[should_panic(expected = "breaks the pipeline's own frame contract")]
fn a_stored_frame_of_the_wrong_shape_is_a_pipeline_bug() {
    let image = LinearImage::from_pixels(ImageDimensions::new((4, 1), 1), vec![1.0; 4]);
    let stats = FrameStats::measure(&image);
    let core = CacheCore::plain(CacheTier::Resident, ImageDimensions::new((4, 2), 1));
    let _cache = FrameCache::from_stored_frames(
        vec![StoredFrame::from_memory(image, FrameQuality::None, stats)],
        core,
        Normalization::None,
    );
}

/// Every frame must carry the first frame's mosaic pattern, or, like it, none: the same pixel is a
/// different colour under another pattern, and a demosaiced frame is no mosaic at all.
#[test]
fn stored_frames_must_share_one_cfa_pattern() {
    let size = Size2us::new(4, 2);
    let dimensions = ImageDimensions::new(size, 1);
    let core = || CacheCore::plain(CacheTier::Resident, dimensions);
    let mosaic = |cfa_type: CfaType| {
        let image = make_cfa(size, vec![1.0; size.pixel_count()], cfa_type);
        let stats = FrameStats::measure(&image);
        StoredFrame::from_memory(image, FrameQuality::None, stats)
    };
    let linear = || {
        let image = LinearImage::from_pixels(dimensions, vec![1.0; size.pixel_count()]);
        let stats = FrameStats::measure(&image);
        StoredFrame::from_memory(image, FrameQuality::None, stats)
    };
    let rggb = CfaType::Bayer(CfaPattern::Rggb);
    let bggr = CfaType::Bayer(CfaPattern::Bggr);

    for (frames, actual, expected) in [
        (vec![mosaic(rggb), mosaic(bggr)], Some(bggr), Some(rggb)),
        (vec![mosaic(rggb), linear()], None, Some(rggb)),
        (
            vec![linear(), mosaic(CfaType::Mono)],
            Some(CfaType::Mono),
            None,
        ),
    ] {
        let error =
            FrameCache::from_stored_frames(frames, core(), Normalization::None).unwrap_err();
        assert!(
            matches!(
                error,
                StackError::CfaPatternMismatch {
                    index: 1,
                    actual: a,
                    reference_index: 0,
                    expected: e,
                } if a == actual && e == expected
            ),
            "{error:?}"
        );
    }
    assert!(
        FrameCache::from_stored_frames(
            vec![mosaic(rggb), mosaic(rggb)],
            core(),
            Normalization::None
        )
        .is_ok()
    );
    assert!(
        FrameCache::from_stored_frames(vec![linear(), linear()], core(), Normalization::None)
            .is_ok()
    );
}

/// Stored frames are held to the same pairing as caller-supplied ones
/// (`stack_images_rejects_warp_quality_planes_that_disagree_about_support`): support and confidence
/// must agree on which pixels a frame reaches.
#[test]
fn stored_frames_with_planes_that_disagree_about_support_are_rejected() {
    let dimensions = ImageDimensions::new((4, 1), 1);
    let core = || CacheCore::plain(CacheTier::Resident, dimensions);
    let frame = |coverage: Vec<f32>, confidence: Vec<f32>| {
        let image = LinearImage::from_pixels(dimensions, vec![1.0; 4]);
        let stats = FrameStats::measure(&image);
        StoredFrame::from_memory(
            image,
            FrameQuality::Planes {
                coverage: Buffer2::new(4, 1, coverage),
                confidence: Buffer2::new(4, 1, confidence),
            },
            stats,
        )
    };

    let error = validate_frames(
        &[frame(vec![1.0, 1.0, 1.0, 1.0], vec![1.0, 1.0, 0.0, 1.0])],
        dimensions,
    )
    .unwrap_err();
    assert!(
        matches!(
            error,
            StackError::FrameQualityPairMismatch {
                index: 0,
                pixel: 2,
                coverage: 1.0,
                confidence: 0.0,
            }
        ),
        "expected a pair mismatch at pixel 2, got {error:?}"
    );

    // The pair the warp would have produced there builds.
    assert!(
        FrameCache::from_stored_frames(
            vec![frame(vec![1.0, 1.0, 0.0, 1.0], vec![1.0, 1.0, 0.0, 1.0])],
            core(),
            Normalization::None,
        )
        .is_ok()
    );
}

/// The plain weighted mean of `cache` with every plane, under manual `weights` or equal ones.
fn mean_product(cache: &FrameCache, weights: Option<&[f32]>) -> StackProduct {
    let slots = Slots::new(None, cache.core.dimensions.channels());
    let weights = weights.map(|weights| {
        FrameWeights::resolve(&Weighting::Manual(weights.to_vec()), [], None, slots)
            .unwrap()
            .unwrap()
    });
    let noise = SampleNoise::new(
        cache.frames.iter().map(|frame| &frame.source_stats),
        None,
        slots,
    );
    let request = CombineRequest {
        weights: weights.as_ref(),
        planes: QualityPlanes::ALL,
        min_survivors: DEFAULT_MIN_SURVIVORS,
        noise: Some(&noise),
        slots,
    };
    let combined = cache.process_chunked(request, |samples, scratch| {
        Rejection::None.combine_mean(samples, DEFAULT_MIN_SURVIVORS, scratch, true)
    });
    cache.finish_product(combined, QualityPlanes::ALL, None)
}

#[test]
fn weighted_chunk_memory_counts_active_inputs_and_full_outputs() {
    let dimensions = ImageDimensions::new((2, 1), 3);
    let image = || LinearImage::from_pixels(dimensions, vec![1.0; 6]);
    let plane = || Buffer2::new(2, 1, vec![1.0; 2]);
    let mut frames = vec![
        StackFrame::from(image()),
        StackFrame::from(image()),
        StackFrame::from(image()),
    ];
    frames[1].quality = FrameQuality::from_coverage(plane());
    frames[2].quality = FrameQuality::from_coverage(plane());

    let cache = FrameCache::from_stack_frames(
        frames,
        Normalization::None,
        ProgressCallback::default(),
        CancelToken::never(),
    )
    .expect("frames are valid");

    // Inputs: 3 frames × 1 channel, plus the coverage + confidence pair frames 1 and 2 each carry.
    // Residents: 3 channels × (pixels + weight + variance), and the dispersion when asked for.
    assert_eq!(
        cache.weighted_layout(QualityPlanes::STANDARD),
        ChunkMemoryLayout {
            input_bytes: 7 * 4,
            resident_planes: 9,
        }
    );
    assert_eq!(
        cache.weighted_layout(QualityPlanes::ALL),
        ChunkMemoryLayout {
            input_bytes: 7 * 4,
            resident_planes: 12,
        }
    );

    // Declining the quality planes drops their residency, so the same frames buy more rows.
    assert_eq!(
        cache.weighted_layout(QualityPlanes::IMAGE_ONLY),
        ChunkMemoryLayout {
            input_bytes: 7 * 4,
            resident_planes: 3,
        }
    );

    // The coverage pass reads only the two frames carrying frame quality, and adds the plane it is
    // accumulating to the combine's residents.
    assert_eq!(
        cache.coverage_layout(QualityPlanes::STANDARD),
        ChunkMemoryLayout {
            input_bytes: 2 * 4,
            resident_planes: 10,
        }
    );
}

#[test]
fn finish_product_uniform_equal_weights() {
    // 4 frames, no coverage maps → fast path. Equal weights and unit noise: every pixel sees all 4
    // frames at weight 1, so weight = Σw = 4, variance = Σw²/(Σw)² = 4/16 = 0.25, coverage = 1.
    let dims = ImageDimensions::new((3, 2), 1);
    let images: Vec<LinearImage> = (0..4)
        .map(|i| LinearImage::from_pixels(dims, vec![i as f32; 6]))
        .collect();
    let product = mean_product(
        &with_unit_noise(FrameCache::from_images(images, Normalization::None)),
        None,
    );
    let linear_variance = product.variance.as_ref().unwrap();
    assert!(matches!(
        product.weight.as_ref().unwrap(),
        QualityMap::Shared(_)
    ));
    assert!(matches!(linear_variance, QualityMap::Shared(_)));
    assert_eq!(product.image.channel(0).pixels(), &[1.5; 6]);
    // No frame carried a coverage map, so coverage is the constant 1.0 and no plane is built —
    // at a full-frame master that is the difference between one number and 240 MB.
    let coverage = product.coverage.as_ref().unwrap();
    assert!(
        matches!(
            coverage,
            Coverage::Uniform { value, size } if *value == 1.0 && *size == Size2us::new(3, 2)
        ),
        "fully-covered stack should not materialize a plane: {coverage:?}"
    );
    // It still materializes like the plane it stands for.
    assert_eq!(coverage.to_plane().pixels(), &[1.0; 6]);
    let Some(QualityMap::Shared(weight)) = product.weight.as_ref() else {
        panic!("a mono stack has one weight plane");
    };
    let QualityMap::Shared(variance) = linear_variance else {
        panic!("a mono stack has one variance plane");
    };
    assert_eq!(weight.pixels(), &[4.0; 6]);
    assert_eq!(variance.pixels(), &[0.25; 6]);
}

#[test]
fn finish_product_uniform_manual_weights() {
    // Weights [1,2,3,4] and unit noise, full coverage: weight = 10, Σw² = 1+4+9+16 = 30, variance =
    // 30/100 = 0.30.
    let dims = ImageDimensions::new((2, 1), 1);
    let images: Vec<LinearImage> = (0..4)
        .map(|_| LinearImage::from_pixels(dims, vec![0.5; 2]))
        .collect();
    let product = mean_product(
        &with_unit_noise(FrameCache::from_images(images, Normalization::None)),
        Some(&[1.0, 2.0, 3.0, 4.0]),
    );
    let linear_variance = product.variance.as_ref().unwrap();
    for p in 0..2 {
        assert_eq!(product.coverage.as_ref().unwrap()[p], 1.0);
        assert_eq!(product.weight.as_ref().unwrap().channel(0)[p], 10.0);
        assert_eq!(linear_variance.channel(0)[p], 0.3);
    }
}

#[test]
fn finish_product_partial_coverage() {
    // width-3 frames. px1 has support from f0, f1, and f3, while f2 is unsupported. px2 excludes
    // f1 the other way: coverage exactly at the floor, which is border fill rather than data — the
    // two exclusions have to produce the same counts, since one rule decides both.
    // Coverage gates inclusion but does not scale statistical weight. Unit noise:
    //   px0: count 4, Σw = 4, Σw² = 4 → coverage 1.0,  weight 4.0, variance 0.25
    //   px1: count 3, Σw = 3, Σw² = 3 → coverage 0.75, weight 3.0, variance 1/3
    //   px2: count 3, as px1
    let dims = ImageDimensions::new((3, 1), 1);
    let cov = [
        [1.0_f32, 1.0, 1.0],
        [1.0, 0.5, PixelCoverage::MIN_CONTRIBUTING],
        [1.0, 0.0, 1.0],
        [1.0, 1.0, 1.0],
    ];
    let frames: Vec<StackFrame> = cov
        .iter()
        .map(|c| {
            let mut frame = StackFrame::from(LinearImage::from_pixels(dims, vec![0.5; 3]));
            frame.quality = FrameQuality::from_coverage(Buffer2::new(3, 1, c.to_vec()));
            frame
        })
        .collect();
    let cache = with_unit_noise(
        FrameCache::from_stack_frames(
            frames,
            Normalization::None,
            ProgressCallback::default(),
            CancelToken::never(),
        )
        .expect("frames are valid"),
    );
    let product = mean_product(&cache, None);
    let linear_variance = product.variance.as_ref().unwrap();

    assert_eq!(product.coverage.as_ref().unwrap()[0], 1.0);
    assert_eq!(product.weight.as_ref().unwrap().channel(0)[0], 4.0);
    assert_eq!(linear_variance.channel(0)[0], 0.25);

    for pixel in [1, 2] {
        assert_eq!(product.coverage.as_ref().unwrap()[pixel], 0.75, "px{pixel}");
        assert_eq!(
            product.weight.as_ref().unwrap().channel(0)[pixel],
            3.0,
            "px{pixel}"
        );
        assert_eq!(linear_variance.channel(0)[pixel], 1.0 / 3.0, "px{pixel}");
    }
}

/// Light and calibration frames combine through one engine: a resident cache of linear frames and
/// one of mono CFA frames give the same median of [1, 3, 2], 2, and the same weighted mean of
/// [10, 20] under [1, 3], 70/4 = 17.5, at every pixel. Neither carries coverage, so the weights
/// reach the reducer unscaled. An RGB set combines per channel: (1 + 5)/2, (2 + 6)/2, (3 + 7)/2.
#[test]
fn light_and_calibration_frames_combine_through_one_engine() {
    let dims = ImageDimensions::new((2, 2), 1);
    let caches = |values: &[f32]| {
        let linear = FrameCache::from_images(
            values
                .iter()
                .map(|&value| LinearImage::from_pixels(dims, vec![value; 4]))
                .collect(),
            Normalization::None,
        );
        let mosaic = FrameCache::from_images(
            values
                .iter()
                .map(|&value| make_cfa(dims.size(), vec![value; 4], CfaType::Mono))
                .collect(),
            Normalization::None,
        );
        [linear, mosaic]
    };
    for cache in caches(&[1.0, 3.0, 2.0]) {
        assert_eq!(cache.core.tier.chunk_memory(), None);
        let median = cache.process_chunked(request(QualityPlanes::IMAGE_ONLY), |samples, _| {
            let count = samples.values.len();
            CombinedSample::value_only(statistics::median_mut(samples.values), count)
        });
        assert_eq!(median.pixels.channel(0).pixels(), &[2.0; 4]);
    }
    for cache in caches(&[10.0, 20.0]) {
        let slots = Slots::new(cache.frames[0].source_stats.facts.cfa_type, 1);
        let weights = FrameWeights::resolve(&Weighting::Manual(vec![1.0, 3.0]), [], None, slots)
            .unwrap()
            .unwrap();
        let weighted = cache.process_chunked(
            CombineRequest {
                weights: Some(&weights),
                slots,
                ..request(QualityPlanes::IMAGE_ONLY)
            },
            |samples, scratch| {
                Rejection::None.combine_mean(samples, DEFAULT_MIN_SURVIVORS, scratch, false)
            },
        );
        assert_eq!(weighted.pixels.channel(0).pixels(), &[17.5; 4]);
    }

    let rgb = |values: [f32; 3]| {
        LinearImage::from_planar_channels(
            ImageDimensions::new((2, 2), 3),
            values.map(|value| vec![value; 4]),
        )
    };
    let cache = FrameCache::from_images(
        vec![rgb([1.0, 2.0, 3.0]), rgb([5.0, 6.0, 7.0])],
        Normalization::None,
    );
    let mean = cache.process_chunked(request(QualityPlanes::IMAGE_ONLY), |samples, scratch| {
        Rejection::None.combine_mean(samples, DEFAULT_MIN_SURVIVORS, scratch, false)
    });
    for (channel, level) in [3.0, 4.0, 5.0].into_iter().enumerate() {
        assert_eq!(mean.pixels.channel(channel).pixels(), &[level; 4]);
    }
}

/// A frame's plane reads the rows asked for, resident or memory-mapped alike: row 1 of a 4 × 3
/// ramp is pixels 4..8, and the whole plane is the ramp. A spilled cache sizes its chunks against
/// the run's planning figure.
#[test]
fn stored_planes_read_their_rows_in_memory_and_on_disk() {
    let temp_dir = TempDir::new("lumos_read_chunk_disk_test");
    let scratch = RunScratch::create(temp_dir.path()).unwrap();
    let dims = ImageDimensions::new((4, 3), 1);
    let ramp: Vec<f32> = (0..12).map(|i| i as f32).collect();
    let image = LinearImage::from_pixels(dims, ramp.clone());

    let resident = FrameCache::from_images(vec![image.clone()], Normalization::None);
    let spilled = FrameCache::from_stored_frames(
        vec![
            StoredFrame::spill(
                &scratch,
                &image,
                &FrameQuality::None,
                FrameStats::measure(&image),
            )
            .unwrap(),
        ],
        CacheCore::plain(
            CacheTier::of(true, RunMemory::new(1 << 30, Some(123_456))),
            dims,
        ),
        Normalization::None,
    )
    .unwrap();
    assert_eq!(spilled.core.tier.chunk_memory(), Some(123_456));
    for cache in [&resident, &spilled] {
        let plane = &cache.frames[0].channels[0];
        assert_eq!(plane.chunk(4, 8), &ramp[4..8]);
        assert_eq!(plane.chunk(0, 12), ramp.as_slice());
    }
}
