use std::sync::OnceLock;

use crate::io::image::cfa::{CfaImage, CfaType};
use crate::memory::ChunkMemoryLayout;
use crate::stacking::combine::cache::core::{
    coverage_chunk_memory_layout, weighted_chunk_memory_layout,
};
use crate::stacking::combine::cache::sample::CombinedSample;
use crate::stacking::combine::cache::*;
use crate::stacking::combine::config::Normalization;
use crate::stacking::combine::pixel_coverage::PixelCoverage;
use crate::stacking::combine::rejection::Rejection;
use crate::stacking::combine::rejection::scratch_buffers::ScratchBuffers;
use crate::stacking::frame_store::StoredFrame;
use crate::stacking::frame_store::frame_quality::FramePlane;
use crate::stacking::frame_store::frame_stats::FrameStats;
use crate::stacking::stack_product::coverage::Coverage;
use crate::stacking::stack_product::quality_planes::QualityPlanes;
use crate::testing::ScratchDirectory;
use crate::testing::prelude::*;

/// Create an in-memory [`FrameCache`] from loaded images, with no coverage (test helper).
pub(crate) fn make_test_cache(images: Vec<LinearImage>) -> FrameCache {
    let frames = images.into_iter().map(StackFrame::from).collect();
    FrameCache::from_stack_frames(
        frames,
        &CacheConfig::default(),
        Normalization::None,
        ProgressCallback::default(),
        CancelToken::never(),
    )
    .expect("test images must be non-empty and dimension-consistent")
}

#[test]
fn unrequested_quality_planes_are_never_allocated() {
    // The observable `StackProduct` has said `linear_variance: None` for a median for as long as
    // the field has existed — it used to be allocated, written per pixel, and then cleared. What
    // matters is that the reducer no longer builds it, which is only visible on the combine's own
    // output, before `finish_product` shapes it.
    let dims = ImageDimensions::new((4, 2), 1);
    let cache = make_test_cache(vec![
        LinearImage::from_pixels(dims, vec![1.0; 8]),
        LinearImage::from_pixels(dims, vec![3.0; 8]),
    ]);
    let reduce = |values: &mut [f32], weights: &[f32], _: &mut ScratchBuffers| {
        CombinedSample::from_all(values.iter().sum::<f32>() / values.len() as f32, weights)
    };

    let weight_only = cache.process_chunked(
        None,
        None,
        QualityPlanes {
            variance: false,
            ..QualityPlanes::ALL
        },
        reduce,
    );
    assert!(weight_only.weight.is_some());
    assert!(
        weight_only.linear_variance.is_none(),
        "a variance plane was allocated for a combine that did not ask for one"
    );

    let bare = cache.process_chunked(None, None, QualityPlanes::IMAGE_ONLY, reduce);
    assert!(bare.weight.is_none());
    assert!(bare.linear_variance.is_none());

    // Skipping the planes must not disturb the combined pixels.
    let all = cache.process_chunked(None, None, QualityPlanes::ALL, reduce);
    assert_eq!(
        bare.pixels.channel(0).pixels(),
        all.pixels.channel(0).pixels()
    );
}

#[test]
fn quality_plane_request_drops_variance_for_a_non_linear_combine() {
    assert_eq!(
        QualityPlanes::ALL.resolve(false),
        QualityPlanes {
            variance: false,
            ..QualityPlanes::ALL
        },
        "a reducer that is not a linear combination reports no variance factor"
    );
    assert_eq!(QualityPlanes::ALL.resolve(true), QualityPlanes::ALL);
    assert_eq!(
        QualityPlanes::IMAGE_ONLY.resolve(true),
        QualityPlanes::IMAGE_ONLY,
        "resolving never adds a plane the caller declined"
    );
}

#[test]
fn stored_frames_of_the_wrong_shape_are_rejected_not_sliced() {
    // Every read of a stored plane slices it to the cache's pixel count, so a frame that does not
    // match would fault out of a slice index naming neither the frame nor the field. The geometry
    // check runs first and names both.
    let dimensions = ImageDimensions::new((4, 2), 1);
    let params = || FrameCacheParams {
        spill_directory: None,
        dimensions,
        metadata: ImageMetadata::default(),
        config: CacheConfig::default(),
        normalization: Normalization::None,
        progress: ProgressCallback::default(),
        cancel: CancelToken::never(),
    };
    let frame = |pixels: usize| {
        let image =
            LinearImage::from_pixels(ImageDimensions::new((pixels, 1), 1), vec![1.0; pixels]);
        let stats = FrameStats::measure(&image);
        StoredFrame::from_memory(image, FrameQuality::None, stats)
    };

    // Short channel plane: 4 samples where the cache wants 8.
    let error = FrameCache::from_stored_frames(vec![frame(8), frame(4)], params()).unwrap_err();
    assert!(
        matches!(
            error,
            Error::StoredFramePlaneSamples {
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
    let error = FrameCache::from_stored_frames(vec![short_coverage], params()).unwrap_err();
    assert!(
        matches!(
            error,
            Error::StoredFramePlaneSamples {
                plane: FramePlane::Coverage,
                expected: 8,
                actual: 2,
                ..
            }
        ),
        "expected a coverage geometry error, got {error:?}"
    );

    // A correctly shaped set still builds.
    assert!(FrameCache::from_stored_frames(vec![frame(8), frame(8)], params()).is_ok());
}

/// Frames arriving through the frame store are held to the same pairing as caller-supplied ones
/// (`stack_images_rejects_warp_quality_planes_that_disagree_about_support`), so a spilled or
/// pipeline-built frame cannot reach the combine with support and confidence disagreeing.
#[test]
fn stored_frames_with_planes_that_disagree_about_support_are_rejected() {
    let dimensions = ImageDimensions::new((4, 1), 1);
    let params = || FrameCacheParams {
        spill_directory: None,
        dimensions,
        metadata: ImageMetadata::default(),
        config: CacheConfig::default(),
        normalization: Normalization::None,
        progress: ProgressCallback::default(),
        cancel: CancelToken::never(),
    };
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

    let error = FrameCache::from_stored_frames(
        vec![frame(vec![1.0, 1.0, 1.0, 1.0], vec![1.0, 1.0, 0.0, 1.0])],
        params(),
    )
    .unwrap_err();
    assert!(
        matches!(
            error,
            Error::FrameQualityPairMismatch {
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
            params(),
        )
        .is_ok()
    );
}

fn mean_product(cache: &FrameCache, weights: Option<&[f32]>) -> StackProduct {
    let combined = cache.process_chunked(
        weights,
        None,
        QualityPlanes::ALL,
        |values, weights, scratch| Rejection::None.combine_mean(values, weights, scratch, true),
    );
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
        &CacheConfig::default(),
        Normalization::None,
        ProgressCallback::default(),
        CancelToken::never(),
    )
    .expect("frames are valid");

    // Inputs: 3 frames × 1 channel, plus the coverage + confidence pair frames 1 and 2 each carry.
    // Residents: 3 channels × (pixels + weight + variance).
    assert_eq!(
        weighted_chunk_memory_layout(&cache.frames, dimensions.channels(), QualityPlanes::ALL),
        ChunkMemoryLayout {
            input_planes: 7,
            resident_planes: 9,
        }
    );

    // Declining the quality planes drops their residency, so the same frames buy more rows.
    assert_eq!(
        weighted_chunk_memory_layout(
            &cache.frames,
            dimensions.channels(),
            QualityPlanes::IMAGE_ONLY
        ),
        ChunkMemoryLayout {
            input_planes: 7,
            resident_planes: 3,
        }
    );

    // The coverage pass reads only the two frames carrying frame quality, and adds the plane it is
    // accumulating to the combine's residents.
    assert_eq!(
        coverage_chunk_memory_layout(&cache.frames, dimensions.channels(), QualityPlanes::ALL),
        ChunkMemoryLayout {
            input_planes: 2,
            resident_planes: 10,
        }
    );
}

#[test]
fn finish_product_uniform_equal_weights() {
    // 4 frames, no coverage maps → fast path. Equal weights: every pixel sees all 4 frames at
    // weight 1, so weight = Σw = 4, variance = Σw²/(Σw)² = 4/16 = 0.25, coverage = 4/4 = 1.
    let dims = ImageDimensions::new((3, 2), 1);
    let images: Vec<LinearImage> = (0..4)
        .map(|i| LinearImage::from_pixels(dims, vec![i as f32; 6]))
        .collect();
    let product = mean_product(&make_test_cache(images), None);
    let linear_variance = product.linear_variance.as_ref().unwrap();
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
        matches!(coverage, Coverage::Uniform { value, .. } if *value == 1.0),
        "fully-covered stack should not materialize a plane: {coverage:?}"
    );
    assert!(coverage.per_pixel().is_none());
    assert_eq!(coverage.size(), Size2us::new(3, 2));
    // It still reads and materializes like the plane it stands for.
    assert_eq!(coverage.to_plane().pixels(), &[1.0; 6]);
    for p in 0..6 {
        assert_eq!(product.coverage.as_ref().unwrap()[p], 1.0);
        assert_eq!(product.weight.as_ref().unwrap().channel(0)[p], 4.0);
        assert_eq!(linear_variance.channel(0)[p], 0.25);
    }
}

#[test]
fn finish_product_uniform_manual_weights() {
    // weights [1,2,3,4], full coverage: weight = 10, Σw² = 1+4+9+16 = 30, variance = 30/100 = 0.30.
    let dims = ImageDimensions::new((2, 1), 1);
    let images: Vec<LinearImage> = (0..4)
        .map(|_| LinearImage::from_pixels(dims, vec![0.5; 2]))
        .collect();
    let product = mean_product(&make_test_cache(images), Some(&[1.0, 2.0, 3.0, 4.0]));
    let linear_variance = product.linear_variance.as_ref().unwrap();
    for p in 0..2 {
        assert_eq!(product.coverage.as_ref().unwrap()[p], 1.0);
        assert_eq!(product.weight.as_ref().unwrap().channel(0)[p], 10.0);
        assert!(
            (linear_variance.channel(0)[p] - 0.30).abs() < 1e-6,
            "variance = {}",
            linear_variance.channel(0)[p]
        );
    }
}

#[test]
fn finish_product_partial_coverage() {
    // width-3 frames. px1 has support from f0, f1, and f3, while f2 is unsupported. px2 excludes
    // f1 the other way: coverage exactly at the floor, which is border fill rather than data — the
    // two exclusions have to produce the same counts, since one rule decides both.
    // Coverage gates inclusion but does not scale statistical weight.
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
    let cache = FrameCache::from_stack_frames(
        frames,
        &CacheConfig::default(),
        Normalization::None,
        ProgressCallback::default(),
        CancelToken::never(),
    )
    .expect("frames are valid");
    let product = mean_product(&cache, None);
    let linear_variance = product.linear_variance.as_ref().unwrap();

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
        assert!(
            (linear_variance.channel(0)[pixel] - 1.0 / 3.0).abs() < 1e-6,
            "px{pixel} variance = {}",
            linear_variance.channel(0)[pixel]
        );
    }
}

/// Build an in-memory [`FrameCache`] from single-channel CFA frame pixels: a calibration cache,
/// carrying no coverage or confidence.
fn make_cfa_cache(frames_pixels: Vec<Vec<f32>>, dims: ImageDimensions) -> FrameCache {
    let images = frames_pixels
        .into_iter()
        .map(|pixels| CfaImage {
            data: Buffer2::new(dims.width(), dims.height(), pixels),
            metadata: ImageMetadata {
                cfa_type: Some(CfaType::Mono),
                ..Default::default()
            },
            quantization_sigma: None,
            nulls: None,
        })
        .collect();
    FrameCache::from_images(images, Normalization::None)
}

#[test]
fn process_chunked_median() {
    // Create in-memory cache with 3 grayscale frames
    let dims = ImageDimensions::new((4, 4), 1);
    let images = vec![
        LinearImage::from_pixels(dims, vec![1.0; 16]),
        LinearImage::from_pixels(dims, vec![3.0; 16]),
        LinearImage::from_pixels(dims, vec![2.0; 16]),
    ];

    let cache = make_test_cache(images);
    assert_eq!(cache.core.chunk_available_memory(), None);

    // Median of [1, 3, 2] = 2
    let result = cache.process_chunked(None, None, QualityPlanes::ALL, |values, weights, _| {
        values.sort_by(|a, b| a.partial_cmp(b).unwrap());
        CombinedSample::from_all(values[values.len() / 2], weights)
    });

    assert_eq!(result.pixels.channel_count(), 1);
    assert_eq!(result.pixels.channel(0).len(), 16);
    for &pixel in result.pixels.channel(0).pixels() {
        assert_eq!(pixel, 2.0);
    }
}

#[test]
fn process_chunked_rgb() {
    // Create in-memory cache with 2 RGB frames
    let dims = ImageDimensions::new((2, 2), 3);
    // Frame 1: R=1, G=2, B=3 for all pixels
    let pixels1: Vec<f32> = (0..4).flat_map(|_| vec![1.0, 2.0, 3.0]).collect();
    // Frame 2: R=5, G=6, B=7 for all pixels
    let pixels2: Vec<f32> = (0..4).flat_map(|_| vec![5.0, 6.0, 7.0]).collect();

    let images = vec![
        LinearImage::from_pixels(dims, pixels1),
        LinearImage::from_pixels(dims, pixels2),
    ];

    let cache = make_test_cache(images);

    // Mean: R=(1+5)/2=3, G=(2+6)/2=4, B=(3+7)/2=5
    let result = cache.process_chunked(None, None, QualityPlanes::ALL, |values, weights, _| {
        CombinedSample::from_all(values.iter().sum::<f32>() / values.len() as f32, weights)
    });

    assert_eq!(result.pixels.channel_count(), 3);
    for &pixel in result.pixels.channel(0).pixels() {
        assert!((pixel - 3.0).abs() < f32::EPSILON, "R channel");
    }
    for &pixel in result.pixels.channel(1).pixels() {
        assert!((pixel - 4.0).abs() < f32::EPSILON, "G channel");
    }
    for &pixel in result.pixels.channel(2).pixels() {
        assert!((pixel - 5.0).abs() < f32::EPSILON, "B channel");
    }
}

#[test]
fn process_chunked_with_weights() {
    let dims = ImageDimensions::new((2, 2), 1);
    let images = vec![
        LinearImage::from_pixels(dims, vec![10.0; 4]),
        LinearImage::from_pixels(dims, vec![20.0; 4]),
    ];

    let cache = make_test_cache(images);

    // Weighted mean with weights [1, 3]: (10*1 + 20*3) / (1+3) = 70/4 = 17.5
    let weights = vec![1.0, 3.0];
    let result = cache.process_chunked(Some(&weights), None, QualityPlanes::ALL, |values, w, _| {
        let sum: f32 = values.iter().zip(w.iter()).map(|(v, wt)| v * wt).sum();
        let weight_sum: f32 = w.iter().sum();
        CombinedSample::from_all(sum / weight_sum, w)
    });

    for &pixel in result.pixels.channel(0).pixels() {
        assert_eq!(pixel, 17.5);
    }
}

#[test]
fn calibration_frames_combine_through_the_same_engine_as_lights() {
    // A calibration cache carries no coverage, so every frame contributes at every pixel — the
    // behaviour the separate calibration reducer used to provide.
    let dims = ImageDimensions::new((2, 2), 1);
    let planes = QualityPlanes::IMAGE_ONLY;

    // Median of [1, 3, 2] = 2 at every pixel.
    let cache = make_cfa_cache(vec![vec![1.0; 4], vec![3.0; 4], vec![2.0; 4]], dims);
    let median = cache.process_chunked(None, None, planes, |values, _, _| {
        let value = crate::math::statistics::median_f32_mut(values);
        CombinedSample::value_only(value, values.len())
    });
    assert_eq!(median.pixels.channel_count(), 1);
    for &pixel in median.pixels.channel(0).pixels() {
        assert!(
            (pixel - 2.0).abs() < f32::EPSILON,
            "calibration median should be 2, got {pixel}"
        );
    }

    // Weighted mean of [10, 20] with weights [1, 3] = (10 + 60) / 4 = 17.5 — per-frame weights
    // reach the reducer unscaled, since no coverage or confidence modulates them.
    let cache = make_cfa_cache(vec![vec![10.0; 4], vec![20.0; 4]], dims);
    let weights = [1.0, 3.0];
    let weighted = cache.process_chunked(Some(&weights), None, planes, |values, w, scratch| {
        Rejection::None.combine_mean(values, w, scratch, false)
    });
    for &pixel in weighted.pixels.channel(0).pixels() {
        assert!(
            (pixel - 17.5).abs() < f32::EPSILON,
            "calibration weighted mean should be 17.5, got {pixel}"
        );
    }
}

#[test]
fn frame_count() {
    let dims = ImageDimensions::new((2, 2), 1);
    let images = vec![
        LinearImage::from_pixels(dims, vec![1.0; 4]),
        LinearImage::from_pixels(dims, vec![2.0; 4]),
        LinearImage::from_pixels(dims, vec![3.0; 4]),
    ];

    let cache = make_test_cache(images);

    assert_eq!(cache.frames.len(), 3);
}

#[test]
fn cleanup_removes_files() {
    let temp_dir = ScratchDirectory::new("lumos_cleanup_test");

    let dims = ImageDimensions::new((2, 2), 3);
    let pixels: Vec<f32> = (0..12).map(|i| i as f32).collect();
    let image = LinearImage::from_pixels(dims, pixels);

    let cached_frame = StoredFrame::spill(
        &temp_dir,
        "cleanup_test.bin",
        &image,
        &FrameQuality::None,
        FrameStats::measure(&image),
    )
    .unwrap();

    // Verify cache dir has files
    assert!(temp_dir.exists());
    assert!(temp_dir.read_dir().unwrap().count() > 0);

    let config = CacheConfig::default();

    let cache = FrameCache {
        frames: vec![cached_frame],
        frame_norms: None,
        normalization: Normalization::None,
        core: CacheCore {
            spill_directory: Some(SpillDirectory::create(temp_dir.to_path_buf(), false).unwrap()),
            dimensions: dims,
            metadata: ImageMetadata::default(),
            config,
            progress: ProgressCallback::default(),
            cancel: CancelToken::never(),
            chunk_memory: OnceLock::new(),
        },
    };

    // Drop the cache - should trigger cleanup via the core's Drop
    drop(cache);

    // Entire cache directory should be removed
    assert!(
        !temp_dir.exists(),
        "Cache directory should be deleted on cleanup"
    );
}

#[test]
fn read_channel_chunk_in_memory() {
    let dims = ImageDimensions::new((4, 3), 1);
    // Pixels 0-11 in row-major order
    let pixels: Vec<f32> = (0..12).map(|i| i as f32).collect();
    let images = vec![LinearImage::from_pixels(dims, pixels)];

    let cache = make_test_cache(images);

    // Read row 1 (pixels 4-7)
    let chunk = cache
        .core
        .read_channel_chunk(&cache.frames, |frame| &frame.channels, 0, 0, 1, 2);
    let expected: Vec<f32> = (4..8).map(|i| i as f32).collect();
    assert_eq!(chunk, &expected[..]);

    // Read all rows
    let all = cache
        .core
        .read_channel_chunk(&cache.frames, |frame| &frame.channels, 0, 0, 0, 3);
    assert_eq!(all.len(), 12);
}

#[test]
fn read_channel_chunk_disk_backed() {
    let temp_dir = ScratchDirectory::new("lumos_read_chunk_disk_test");

    let dims = ImageDimensions::new((4, 3), 1);
    let pixels: Vec<f32> = (0..12).map(|i| i as f32).collect();
    let image = LinearImage::from_pixels(dims, pixels);

    // Cache the image to disk
    let base_filename = "test_chunk.bin";
    let cached_frame = StoredFrame::spill(
        &temp_dir,
        base_filename,
        &image,
        &FrameQuality::None,
        FrameStats::measure(&image),
    )
    .unwrap();

    let cache = FrameCache {
        frames: vec![cached_frame],
        frame_norms: None,
        normalization: Normalization::None,
        core: CacheCore {
            spill_directory: Some(SpillDirectory::create(temp_dir.to_path_buf(), false).unwrap()),
            dimensions: dims,
            metadata: ImageMetadata::default(),
            config: CacheConfig {
                available_memory: Some(123_456),
                ..Default::default()
            },
            progress: ProgressCallback::default(),
            cancel: CancelToken::never(),
            chunk_memory: OnceLock::new(),
        },
    };

    // Read row 1 (pixels 4-7)
    let chunk = cache
        .core
        .read_channel_chunk(&cache.frames, |frame| &frame.channels, 0, 0, 1, 2);
    assert_eq!(cache.core.chunk_available_memory(), Some(123_456));
    let expected: Vec<f32> = (4..8).map(|i| i as f32).collect();
    assert_eq!(chunk, &expected[..]);

    // Read all rows
    let all = cache
        .core
        .read_channel_chunk(&cache.frames, |frame| &frame.channels, 0, 0, 0, 3);
    assert_eq!(all.len(), 12);
    for (i, &val) in all.iter().enumerate() {
        assert!((val - i as f32).abs() < f32::EPSILON);
    }

    drop(cache);
}

#[test]
fn frame_count_disk_backed() {
    let temp_dir = ScratchDirectory::new("lumos_frame_count_disk_test");

    let dims = ImageDimensions::new((2, 2), 1);

    // Create 3 cached frames
    let mut frames = Vec::new();
    for i in 0..3 {
        let pixels: Vec<f32> = vec![i as f32; 4];
        let image = LinearImage::from_pixels(dims, pixels);
        let base_filename = format!("frame{}.bin", i);
        let cached_frame = StoredFrame::spill(
            &temp_dir,
            &base_filename,
            &image,
            &FrameQuality::None,
            FrameStats::measure(&image),
        )
        .unwrap();
        frames.push(cached_frame);
    }

    let cache = FrameCache {
        frames,
        frame_norms: None,
        normalization: Normalization::None,
        core: CacheCore {
            spill_directory: Some(SpillDirectory::create(temp_dir.to_path_buf(), false).unwrap()),
            dimensions: dims,
            metadata: ImageMetadata::default(),
            config: CacheConfig::default(),
            progress: ProgressCallback::default(),
            cancel: CancelToken::never(),
            chunk_memory: OnceLock::new(),
        },
    };

    assert_eq!(cache.frames.len(), 3);

    drop(cache);
}

#[test]
fn compute_channel_stats_grayscale() {
    // 3 grayscale frames, 3x3 pixels each
    let dims = ImageDimensions::new((3, 3), 1);

    // Frame 0: all 5.0 → median=5.0, MAD=0.0
    let frame0 = LinearImage::from_pixels(dims, vec![5.0; 9]);

    // Frame 1: [1,2,3,4,5,6,7,8,9] → median=5.0, deviations=[4,3,2,1,0,1,2,3,4] → MAD=2.0
    let frame1 = LinearImage::from_pixels(dims, (1..=9).map(|i| i as f32).collect());

    // Frame 2: [10,10,10,20,20,20,30,30,30] → median=20.0, deviations=[10,10,10,0,0,0,10,10,10] → MAD=10.0
    let frame2 = LinearImage::from_pixels(
        dims,
        vec![10.0, 10.0, 10.0, 20.0, 20.0, 20.0, 30.0, 30.0, 30.0],
    );

    let cache = make_test_cache(vec![frame0, frame1, frame2]);
    let stats: Vec<_> = cache
        .frames
        .iter()
        .map(|frame| &frame.source_stats)
        .collect();

    assert_eq!(stats.len(), 3); // 3 frames
    assert_eq!(stats[0].channels.len(), 1);
    assert_eq!(stats[0].channels[0].median, 5.0);
    assert_eq!(stats[0].channels[0].mad, 0.0);
    assert_eq!(stats[1].channels[0].median, 5.0);
    assert_eq!(stats[1].channels[0].mad, 2.0);
    assert_eq!(stats[2].channels[0].median, 20.0);
    assert_eq!(stats[2].channels[0].mad, 10.0);
}

#[test]
fn compute_channel_stats_rgb() {
    // 2 RGB frames, 2x2 pixels each
    let dims = ImageDimensions::new((2, 2), 3);

    // Frame 0: R=[1,3,5,7] G=[10,10,10,10] B=[0,0,100,100]
    let frame0 = LinearImage::from_planar_channels(
        dims,
        vec![
            vec![1.0, 3.0, 5.0, 7.0],
            vec![10.0, 10.0, 10.0, 10.0],
            vec![0.0, 0.0, 100.0, 100.0],
        ],
    );
    // Frame 0 expected:
    //   R: median=4.0 (avg of 3,5), deviations=[3,1,1,3] → MAD=2.0 (avg of 1,3)
    //   G: median=10.0, MAD=0.0
    //   B: median=50.0 (avg of 0,100), deviations=[50,50,50,50] → MAD=50.0

    // Frame 1: R=[2,2,2,2] G=[1,2,3,4] B=[10,20,30,40]
    let frame1 = LinearImage::from_planar_channels(
        dims,
        vec![
            vec![2.0, 2.0, 2.0, 2.0],
            vec![1.0, 2.0, 3.0, 4.0],
            vec![10.0, 20.0, 30.0, 40.0],
        ],
    );
    // Frame 1 expected:
    //   R: median=2.0, MAD=0.0
    //   G: median=2.5, deviations=[1.5,0.5,0.5,1.5] → MAD=1.0
    //   B: median=25.0, deviations=[15,5,5,15] → MAD=10.0

    let cache = make_test_cache(vec![frame0, frame1]);
    let stats: Vec<_> = cache
        .frames
        .iter()
        .map(|frame| &frame.source_stats)
        .collect();

    assert_eq!(stats.len(), 2); // 2 frames
    assert_eq!(stats[0].channels.len(), 3); // 3 channels each

    // Frame 0
    assert!(
        (stats[0].channels[0].median - 4.0).abs() < f32::EPSILON,
        "F0 R median"
    );
    assert!(
        (stats[0].channels[0].mad - 2.0).abs() < f32::EPSILON,
        "F0 R MAD"
    );
    assert!(
        (stats[0].channels[1].median - 10.0).abs() < f32::EPSILON,
        "F0 G median"
    );
    assert!(
        (stats[0].channels[1].mad - 0.0).abs() < f32::EPSILON,
        "F0 G MAD"
    );
    assert!(
        (stats[0].channels[2].median - 50.0).abs() < f32::EPSILON,
        "F0 B median"
    );
    assert!(
        (stats[0].channels[2].mad - 50.0).abs() < f32::EPSILON,
        "F0 B MAD"
    );

    // Frame 1
    assert!(
        (stats[1].channels[0].median - 2.0).abs() < f32::EPSILON,
        "F1 R median"
    );
    assert!(
        (stats[1].channels[0].mad - 0.0).abs() < f32::EPSILON,
        "F1 R MAD"
    );
    assert!(
        (stats[1].channels[1].median - 2.5).abs() < f32::EPSILON,
        "F1 G median"
    );
    assert!(
        (stats[1].channels[1].mad - 1.0).abs() < f32::EPSILON,
        "F1 G MAD"
    );
    assert!(
        (stats[1].channels[2].median - 25.0).abs() < f32::EPSILON,
        "F1 B median"
    );
    assert!(
        (stats[1].channels[2].mad - 10.0).abs() < f32::EPSILON,
        "F1 B MAD"
    );
}
