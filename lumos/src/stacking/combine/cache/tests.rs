use crate::io::image::LinearImage;
use crate::io::image::cfa::{CfaImage, CfaType};
use crate::stacking::combine::cache::*;
use crate::stacking::combine::config::Normalization;
use crate::stacking::combine::rejection::Rejection;
use crate::stacking::frame_store::{frame_from_memory, store_frame};
use crate::testing::ScratchDirectory;

/// Create an in-memory [`LightCache`] from loaded images, with no coverage (test helper).
pub(crate) fn make_test_cache(images: Vec<LinearImage>) -> LightCache {
    let frames = images.into_iter().map(StackFrame::from).collect();
    LightCache::from_stack_frames(
        frames,
        &CacheConfig::default(),
        Normalization::None,
        ProgressCallback::default(),
        CancelToken::never(),
    )
    .expect("test images must be non-empty and dimension-consistent")
}

fn mean_product(cache: &LightCache, weights: Option<&[f32]>) -> StackProduct {
    let combined = cache.process_chunked_weighted(weights, None, |values, weights, scratch| {
        Rejection::None.combine_mean_with_quality(values, weights, scratch)
    });
    cache.finish_product(combined)
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
    frames[1].coverage = Some(plane());
    frames[2].coverage = Some(plane());
    frames[2].confidence = Some(plane());

    let cache = LightCache::from_stack_frames(
        frames,
        &CacheConfig::default(),
        Normalization::None,
        ProgressCallback::default(),
        CancelToken::never(),
    )
    .expect("frames are valid");

    assert_eq!(
        weighted_chunk_memory_layout(&cache.frames, dimensions.channels()),
        ChunkMemoryLayout {
            input_planes: 6,
            resident_planes: 9,
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
    assert!(matches!(&product.weight, QualityMap::Shared(_)));
    assert!(matches!(linear_variance, QualityMap::Shared(_)));
    assert_eq!(product.image.channel(0).pixels(), &[1.5; 6]);
    for p in 0..6 {
        assert_eq!(product.coverage[p], 1.0);
        assert_eq!(product.weight.channel(0)[p], 4.0);
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
        assert_eq!(product.coverage[p], 1.0);
        assert_eq!(product.weight.channel(0)[p], 10.0);
        assert!(
            (linear_variance.channel(0)[p] - 0.30).abs() < 1e-6,
            "variance = {}",
            linear_variance.channel(0)[p]
        );
    }
}

#[test]
fn finish_product_partial_coverage() {
    // width-2 frames; px1 has support from f0, f1, and f3, while f2 is unsupported.
    // Coverage gates inclusion but does not scale statistical weight.
    //   px0: count 4, Σw = 4, Σw² = 4 → coverage 1.0,  weight 4.0, variance 0.25
    //   px1: count 3, Σw = 3, Σw² = 3 → coverage 0.75, weight 3.0, variance 1/3
    let dims = ImageDimensions::new((2, 1), 1);
    let cov = [[1.0_f32, 1.0], [1.0, 0.5], [1.0, 0.0], [1.0, 1.0]];
    let frames: Vec<StackFrame> = cov
        .iter()
        .map(|c| {
            let mut frame = StackFrame::from(LinearImage::from_pixels(dims, vec![0.5, 0.5]));
            frame.coverage = Some(Buffer2::new(2, 1, c.to_vec()));
            frame
        })
        .collect();
    let cache = LightCache::from_stack_frames(
        frames,
        &CacheConfig::default(),
        Normalization::None,
        ProgressCallback::default(),
        CancelToken::never(),
    )
    .expect("frames are valid");
    let product = mean_product(&cache, None);
    let linear_variance = product.linear_variance.as_ref().unwrap();

    assert_eq!(product.coverage[0], 1.0);
    assert_eq!(product.weight.channel(0)[0], 4.0);
    assert_eq!(linear_variance.channel(0)[0], 0.25);

    assert_eq!(product.coverage[1], 0.75);
    assert_eq!(product.weight.channel(0)[1], 3.0);
    assert!(
        (linear_variance.channel(0)[1] - 1.0 / 3.0).abs() < 1e-6,
        "variance = {}",
        linear_variance.channel(0)[1]
    );
}

/// Build an in-memory [`CfaCache`] from single-channel CFA frame pixels (test helper for the
/// plain combine; `process_chunked` ignores statistics.
fn make_cfa_cache(frames_pixels: Vec<Vec<f32>>, dims: ImageDimensions) -> CfaCache {
    let frames = frames_pixels
        .into_iter()
        .map(|pixels| {
            let image = CfaImage {
                data: Buffer2::new(dims.width(), dims.height(), pixels),
                metadata: ImageMetadata {
                    cfa_type: Some(CfaType::Mono),
                    ..Default::default()
                },
                quantization_sigma: None,
            };
            frame_from_memory(image)
        })
        .collect();
    CfaCache {
        frames,
        frame_stats: vec![],
        core: CacheCore {
            spill_directory: None,
            dimensions: dims,
            metadata: ImageMetadata::default(),
            config: CacheConfig::default(),
            progress: ProgressCallback::default(),
            cancel: CancelToken::never(),
        },
    }
}

#[test]
fn test_process_chunked_median() {
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
    let result = cache.process_chunked_weighted(None, None, |values, weights, _| {
        values.sort_by(|a, b| a.partial_cmp(b).unwrap());
        CombinedSample::from_all(values[values.len() / 2], weights)
    });

    assert_eq!(result.chunk_available_memory, None);
    assert_eq!(result.pixels.channels(), 1);
    assert_eq!(result.pixels.channel(0).len(), 16);
    for &pixel in result.pixels.channel(0).pixels() {
        assert!((pixel - 2.0).abs() < f32::EPSILON);
    }
}

#[test]
fn test_process_chunked_rgb() {
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
    let result = cache.process_chunked_weighted(None, None, |values, weights, _| {
        CombinedSample::from_all(values.iter().sum::<f32>() / values.len() as f32, weights)
    });

    assert_eq!(result.pixels.channels(), 3);
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
fn test_process_chunked_with_weights() {
    let dims = ImageDimensions::new((2, 2), 1);
    let images = vec![
        LinearImage::from_pixels(dims, vec![10.0; 4]),
        LinearImage::from_pixels(dims, vec![20.0; 4]),
    ];

    let cache = make_test_cache(images);

    // Weighted mean with weights [1, 3]: (10*1 + 20*3) / (1+3) = 70/4 = 17.5
    let weights = vec![1.0, 3.0];
    let result = cache.process_chunked_weighted(Some(&weights), None, |values, w, _| {
        let sum: f32 = values.iter().zip(w.iter()).map(|(v, wt)| v * wt).sum();
        let weight_sum: f32 = w.iter().sum();
        CombinedSample::from_all(sum / weight_sum, w)
    });

    for &pixel in result.pixels.channel(0).pixels() {
        assert!((pixel - 17.5).abs() < f32::EPSILON);
    }
}

#[test]
fn test_cfa_cache_plain_combine() {
    // The plain `CfaCache::process_chunked` path (calibration): no coverage, every frame
    // contributes at every pixel.
    let dims = ImageDimensions::new((2, 2), 1);

    // Median of [1, 3, 2] = 2 at every pixel.
    let cache = make_cfa_cache(vec![vec![1.0; 4], vec![3.0; 4], vec![2.0; 4]], dims);
    let median = cache.process_chunked(None, None, |values, _, _| {
        values.sort_by(|a, b| a.partial_cmp(b).unwrap());
        values[values.len() / 2]
    });
    assert_eq!(median.channels(), 1);
    for &pixel in median.channel(0).pixels() {
        assert!(
            (pixel - 2.0).abs() < f32::EPSILON,
            "CFA plain median should be 2, got {pixel}"
        );
    }

    // Weighted mean of [10, 20] with weights [1, 3] = (10 + 60) / 4 = 17.5 — weights flow
    // through to the combine closure unchanged (no coverage scaling on the plain path).
    let cache = make_cfa_cache(vec![vec![10.0; 4], vec![20.0; 4]], dims);
    let weights = [1.0, 3.0];
    let weighted = cache.process_chunked(Some(&weights), None, |values, w, _| {
        let w = w.unwrap();
        let sum: f32 = values.iter().zip(w).map(|(v, wt)| v * wt).sum();
        sum / w.iter().sum::<f32>()
    });
    for &pixel in weighted.channel(0).pixels() {
        assert!(
            (pixel - 17.5).abs() < f32::EPSILON,
            "CFA plain weighted mean should be 17.5, got {pixel}"
        );
    }
}

#[test]
fn test_frame_count() {
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
fn test_cleanup_removes_files() {
    let temp_dir = ScratchDirectory::new("lumos_cleanup_test");

    let dims = ImageDimensions::new((2, 2), 3);
    let pixels: Vec<f32> = (0..12).map(|i| i as f32).collect();
    let image = LinearImage::from_pixels(dims, pixels);

    let cached_frame = store_frame(&temp_dir, "cleanup_test.bin", &image).unwrap();

    // Verify cache dir has files
    assert!(temp_dir.exists());
    assert!(temp_dir.read_dir().unwrap().count() > 0);

    // Use keep_cache: false to actually test cleanup
    let config = CacheConfig {
        keep_cache: false,
        ..Default::default()
    };

    let cache = CfaCache {
        frames: vec![cached_frame],
        frame_stats: vec![],
        core: CacheCore {
            spill_directory: Some(SpillDirectory::create(temp_dir.to_path_buf(), false).unwrap()),
            dimensions: dims,
            metadata: ImageMetadata::default(),
            config,
            progress: ProgressCallback::default(),
            cancel: CancelToken::never(),
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
fn test_read_channel_chunk_in_memory() {
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
fn test_read_channel_chunk_disk_backed() {
    let temp_dir = ScratchDirectory::new("lumos_read_chunk_disk_test");

    let dims = ImageDimensions::new((4, 3), 1);
    let pixels: Vec<f32> = (0..12).map(|i| i as f32).collect();
    let image = LinearImage::from_pixels(dims, pixels);

    // Cache the image to disk
    let base_filename = "test_chunk.bin";
    let cached_frame = store_frame(&temp_dir, base_filename, &image).unwrap();

    let cache = CfaCache {
        frames: vec![cached_frame],
        frame_stats: vec![],
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
fn test_frame_count_disk_backed() {
    let temp_dir = ScratchDirectory::new("lumos_frame_count_disk_test");

    let dims = ImageDimensions::new((2, 2), 1);

    // Create 3 cached frames
    let mut frames = Vec::new();
    for i in 0..3 {
        let pixels: Vec<f32> = vec![i as f32; 4];
        let image = LinearImage::from_pixels(dims, pixels);
        let base_filename = format!("frame{}.bin", i);
        let cached_frame = store_frame(&temp_dir, &base_filename, &image).unwrap();
        frames.push(cached_frame);
    }

    let cache = CfaCache {
        frames,
        frame_stats: vec![],
        core: CacheCore {
            spill_directory: Some(SpillDirectory::create(temp_dir.to_path_buf(), false).unwrap()),
            dimensions: dims,
            metadata: ImageMetadata::default(),
            config: CacheConfig::default(),
            progress: ProgressCallback::default(),
            cancel: CancelToken::never(),
        },
    };

    assert_eq!(cache.frames.len(), 3);

    drop(cache);
}

#[test]
fn test_compute_channel_stats_grayscale() {
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
    assert!((stats[0].channels[0].median - 5.0).abs() < f32::EPSILON);
    assert!((stats[0].channels[0].mad - 0.0).abs() < f32::EPSILON);
    assert!((stats[1].channels[0].median - 5.0).abs() < f32::EPSILON);
    assert!((stats[1].channels[0].mad - 2.0).abs() < f32::EPSILON);
    assert!((stats[2].channels[0].median - 20.0).abs() < f32::EPSILON);
    assert!((stats[2].channels[0].mad - 10.0).abs() < f32::EPSILON);
}

#[test]
fn test_compute_channel_stats_rgb() {
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
