use std::fs::OpenOptions;
use std::path::PathBuf;
use std::time::{Duration, UNIX_EPOCH};

use crate::io::astro_image::AstroImage;
use crate::stacking::combine::cache::loader::*;
use crate::testing::ScratchDirectory;

#[test]
fn from_paths_reports_empty_and_missing_sources() {
    let config = CacheConfig::default();
    let empty = LightCache::from_paths(
        &Vec::<PathBuf>::new(),
        &config,
        ProgressCallback::default(),
        CancelToken::never(),
    );
    assert!(matches!(empty.unwrap_err(), Error::NoFrames));

    let missing_path = PathBuf::from(".tmp/missing/image.fits");
    let missing = LightCache::from_paths(
        &[missing_path],
        &config,
        ProgressCallback::default(),
        CancelToken::never(),
    );
    assert!(matches!(missing.unwrap_err(), Error::ImageLoad { .. }));
}

#[test]
fn test_load_and_cache_frame_fresh() {
    let temp_dir = ScratchDirectory::new("lumos_load_cache_fresh_test");

    let dims = ImageDimensions::new((4, 3), 1);
    let pixels: Vec<f32> = (0..12).map(|i| i as f32).collect();
    let image = AstroImage::from_pixels(dims, pixels.clone());

    // Write a temp TIFF file to load from
    let source_path = temp_dir.join("source.tiff");
    image.save(&source_path).unwrap();

    let base_filename = "cached_frame.bin";

    // First call should load and cache
    let cached_frame =
        load_and_cache_frame::<AstroImage>(&temp_dir, base_filename, &source_path, dims, 0)
            .unwrap();

    assert_eq!(cached_frame.frame.channels.len(), 1);

    // Verify cached data matches original
    let cached_data = cached_frame.frame.channels[0].chunk(0, dims.pixel_count());
    assert_eq!(cached_data, &pixels[..]);

    // Cleanup
    drop(cached_frame);
}

#[test]
fn test_load_and_cache_frame_reuse() {
    let temp_dir = ScratchDirectory::new("lumos_load_cache_reuse_test");

    let dims = ImageDimensions::new((4, 3), 1);
    let pixels: Vec<f32> = (0..12).map(|i| i as f32).collect();
    let image = AstroImage::from_pixels(dims, pixels.clone());

    // Write a temp TIFF file
    let source_path = temp_dir.join("source.tiff");
    image.save(&source_path).unwrap();

    let base_filename = "cached_frame.bin";

    // First call - creates cache
    let first_frame =
        load_and_cache_frame::<AstroImage>(&temp_dir, base_filename, &source_path, dims, 0)
            .unwrap();

    // Second call - should reuse cache
    let second_frame =
        load_and_cache_frame::<AstroImage>(&temp_dir, base_filename, &source_path, dims, 0)
            .unwrap();

    // Both should have same data
    let n = dims.pixel_count();
    let first_data = first_frame.frame.channels[0].chunk(0, n);
    let second_data = second_frame.frame.channels[0].chunk(0, n);
    assert_eq!(first_data, second_data);
    assert_eq!(first_data, &pixels[..]);

    // Cleanup
    drop(first_frame);
}

#[test]
fn test_load_and_cache_frame_dimension_mismatch() {
    let temp_dir = ScratchDirectory::new("lumos_load_cache_mismatch_test");

    // Create image with different dimensions than expected
    let actual_dims = ImageDimensions::new((4, 3), 1);
    let pixels: Vec<f32> = (0..12).map(|i| i as f32).collect();
    let image = AstroImage::from_pixels(actual_dims, pixels);

    let source_path = temp_dir.join("source.tiff");
    image.save(&source_path).unwrap();

    // Try to load with wrong expected dimensions
    let expected_dims = ImageDimensions::new((8, 6), 1);
    let result =
        load_and_cache_frame::<AstroImage>(&temp_dir, "cached.bin", &source_path, expected_dims, 5);

    assert!(matches!(
        result.unwrap_err(),
        Error::DimensionMismatch {
            index: 5,
            expected,
            actual,
            ..
        } if expected == expected_dims && actual == actual_dims
    ));

    // Cleanup
}

#[test]
fn test_source_meta_validates_mtime() {
    let temp_dir = ScratchDirectory::new("test_source_meta_validates");

    let source = temp_dir.join("source.fits");
    std::fs::write(&source, b"original data").unwrap();
    let missing = temp_dir.join("missing.fits");
    let error = source_mtime(&missing).unwrap_err();
    assert!(matches!(
        error,
        FrameStoreError::ReadMetadata { path, .. } if path == missing
    ));

    let base = "abc123.bin";

    // No meta file yet — validation should fail
    assert!(!validate_source_meta(&temp_dir, base, &source));

    // Write meta for current source
    let mtime = source_mtime(&source).unwrap();
    write_source_meta(&temp_dir, base, mtime).unwrap();

    // Now validation should pass
    assert!(validate_source_meta(&temp_dir, base, &source));

    let file = OpenOptions::new().write(true).open(&source).unwrap();
    file.set_modified(UNIX_EPOCH + Duration::from_secs(mtime + 2))
        .unwrap();

    // Validation should fail — source changed
    assert!(!validate_source_meta(&temp_dir, base, &source));

    // Cleanup
}

#[test]
fn test_frame_stats_sidecar_roundtrip() {
    let temp_dir = ScratchDirectory::new("lumos_stats_roundtrip_test");

    let base = "test_frame.bin";

    // 1-channel stats
    let stats_1ch = FrameStats {
        channels: [ChannelStats {
            median: 42.5,
            mad: 3.25,
        }]
        .into_iter()
        .collect(),
    };
    write_frame_stats(&temp_dir, base, &stats_1ch).unwrap();
    let read_1ch = read_frame_stats(&temp_dir, base).unwrap();
    assert_eq!(read_1ch.channels.len(), 1);
    assert_eq!(read_1ch.channels[0].median, 42.5);
    assert_eq!(read_1ch.channels[0].mad, 3.25);

    // 3-channel stats (overwrites the file)
    let stats_3ch = FrameStats {
        channels: [
            ChannelStats {
                median: 100.0,
                mad: 1.5,
            },
            ChannelStats {
                median: 200.0,
                mad: 2.5,
            },
            ChannelStats {
                median: 300.0,
                mad: 3.5,
            },
        ]
        .into_iter()
        .collect(),
    };
    write_frame_stats(&temp_dir, base, &stats_3ch).unwrap();
    let read_3ch = read_frame_stats(&temp_dir, base).unwrap();
    assert_eq!(read_3ch.channels.len(), 3);
    // Verify exact f32 roundtrip for each channel
    for (i, (got, expected)) in read_3ch
        .channels
        .iter()
        .zip(stats_3ch.channels.iter())
        .enumerate()
    {
        assert_eq!(got.median, expected.median, "channel {i} median");
        assert_eq!(got.mad, expected.mad, "channel {i} mad");
    }

    // Missing file returns None
    assert!(read_frame_stats(&temp_dir, "nonexistent.bin").is_none());

    // Corrupt file returns None
    let corrupt_path = stats_path(&temp_dir, "corrupt.bin");
    std::fs::write(&corrupt_path, b"bad").unwrap();
    assert!(read_frame_stats(&temp_dir, "corrupt.bin").is_none());

    let blocker = temp_dir.join("not_a_directory");
    std::fs::write(&blocker, b"file").unwrap();
    let error = write_frame_stats(&blocker, base, &stats_1ch).unwrap_err();
    let expected_path = blocker.join("test_frame.stats");
    assert!(matches!(
        error,
        FrameStoreError::WriteFile { path, .. } if path == expected_path
    ));

    // Cleanup
}

#[test]
fn test_load_and_cache_frame_reuse_preserves_stats() {
    // Verify that stats computed on first load match stats read from sidecar on reuse.
    let temp_dir = ScratchDirectory::new("lumos_cache_reuse_stats_test");

    // Non-uniform data so median and MAD are non-trivial
    let dims = ImageDimensions::new((4, 3), 1);
    // [0,1,2,3,4,5,6,7,8,9,10,11] → median=5.5, deviations=[5.5,4.5,3.5,2.5,1.5,0.5,0.5,1.5,2.5,3.5,4.5,5.5] → MAD=3.0
    let pixels: Vec<f32> = (0..12).map(|i| i as f32).collect();
    let image = AstroImage::from_pixels(dims, pixels);

    let source_path = temp_dir.join("source.tiff");
    image.save(&source_path).unwrap();

    let base_filename = "stats_test.bin";

    // First call — loads image, computes stats, writes sidecar
    let first = load_and_cache_frame::<AstroImage>(&temp_dir, base_filename, &source_path, dims, 0)
        .unwrap();
    let first_stats = first.stats;

    assert_eq!(first_stats.channels.len(), 1);
    assert!((first_stats.channels[0].median - 5.5).abs() < f32::EPSILON);
    assert!((first_stats.channels[0].mad - 3.0).abs() < f32::EPSILON);

    // Second call — reuses cache, reads stats from sidecar
    let reused_stats =
        load_and_cache_frame::<AstroImage>(&temp_dir, base_filename, &source_path, dims, 0)
            .unwrap()
            .stats;

    // Stats must be identical (exact f32 roundtrip via le_bytes)
    assert_eq!(reused_stats.channels.len(), first_stats.channels.len());
    assert_eq!(
        reused_stats.channels[0].median,
        first_stats.channels[0].median
    );
    assert_eq!(reused_stats.channels[0].mad, first_stats.channels[0].mad);

    // Cleanup
}

#[test]
fn test_missing_stats_sidecar_forces_reload() {
    // If the .stats file is deleted but .meta and .bin remain,
    // load_and_cache_frame should NOT reuse cache (can_reuse = false).
    let temp_dir = ScratchDirectory::new("lumos_missing_stats_test");

    let dims = ImageDimensions::new((4, 3), 1);
    let pixels: Vec<f32> = (0..12).map(|i| i as f32).collect();
    let image = AstroImage::from_pixels(dims, pixels);

    let source_path = temp_dir.join("source.tiff");
    image.save(&source_path).unwrap();

    let base_filename = "missing_stats.bin";

    // First call — creates cache + sidecars
    let first_stats =
        load_and_cache_frame::<AstroImage>(&temp_dir, base_filename, &source_path, dims, 0)
            .unwrap()
            .stats;

    // Delete only the .stats sidecar
    let sp = stats_path(&temp_dir, base_filename);
    assert!(sp.exists());
    std::fs::remove_file(&sp).unwrap();

    // Second call — should reload (not panic) and recompute stats
    let reloaded_stats =
        load_and_cache_frame::<AstroImage>(&temp_dir, base_filename, &source_path, dims, 0)
            .unwrap()
            .stats;

    // Stats should match (same source image)
    assert_eq!(
        reloaded_stats.channels[0].median,
        first_stats.channels[0].median
    );
    assert_eq!(reloaded_stats.channels[0].mad, first_stats.channels[0].mad);

    // .stats file should be recreated
    assert!(sp.exists());

    // Cleanup
}
