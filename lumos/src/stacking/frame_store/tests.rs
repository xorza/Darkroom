use crate::stacking::frame_store::*;
use crate::testing::ScratchDirectory;

const GB: u64 = 1024 * 1024 * 1024;
const FRAME_96MB: usize = 6240 * 4160 * size_of::<f32>();

#[test]
fn stored_image_roundtrip_overwrites_stale_pixels_and_cleans_spill_files() {
    let directory = ScratchDirectory::new("frame_store_image");
    let dimensions = ImageDimensions::new((2, 2), 1);
    let mut image = LinearImage::from_pixels(dimensions, vec![0.1, 0.2, 0.3, 0.4]);
    image.metadata.exposure_time = Some(30.0);
    let path = directory.join("calibrated_c0.bin");
    write_plane(&path, &[9.0; 4]).unwrap();

    let stored = store_image(&directory, "calibrated", &image).unwrap();
    let loaded = stored.load();
    assert_eq!(loaded.channel(0).pixels(), &[0.1, 0.2, 0.3, 0.4]);
    assert_eq!(loaded.metadata.exposure_time, Some(30.0));

    drop(stored);
    assert!(!path.exists());
}

#[test]
fn light_frame_keeps_quality_with_its_planes() {
    let dimensions = ImageDimensions::new((2, 2), 1);
    let image = LinearImage::from_pixels(dimensions, vec![1.0, 2.0, 3.0, 4.0]);
    let coverage = Buffer2::new(2, 2, vec![1.0, 0.5, 0.25, 0.0]);
    let confidence = Buffer2::new(2, 2, vec![4.0, 3.0, 2.0, 1.0]);
    let source_stats = compute_frame_stats(&image);
    let frame =
        StoredLightFrame::from_memory(image, Some(coverage), Some(confidence), source_stats);
    assert_eq!(frame.channels[0].chunk(0, 4), &[1.0, 2.0, 3.0, 4.0]);
    assert_eq!(
        frame.coverage.as_ref().unwrap().chunk(0, 4),
        &[1.0, 0.5, 0.25, 0.0]
    );
    assert_eq!(
        frame.confidence.as_ref().unwrap().chunk(0, 4),
        &[4.0, 3.0, 2.0, 1.0]
    );
    assert_eq!(frame.source_stats.channels[0].median, 2.5);
    assert_eq!(frame.source_stats.channels[0].mad, 1.0);
}

#[test]
fn plane_persistence_validates_dimensions_and_roundtrips_pixels() {
    let directory = ScratchDirectory::new("frame_store_plane");
    let path = directory.join("plane.bin");
    let dimensions = ImageDimensions::new((4, 3), 1);
    let pixels: Vec<f32> = (0..12).map(|value| value as f32).collect();
    write_plane(&path, &pixels).unwrap();

    assert!(reusable_plane(&path, dimensions));
    assert!(!reusable_plane(&path, ImageDimensions::new((8, 3), 1)));
    assert!(!reusable_plane(&directory.join("missing.bin"), dimensions));
    let mapped = StoredPlane::Mapped(map_plane(path.clone()).unwrap());
    assert_eq!(mapped.chunk(0, pixels.len()), pixels);

    drop(mapped);
}

#[test]
fn cache_names_are_stable_and_path_specific() {
    let path = Path::new("/test/deterministic.fits");
    let expected = cache_filename(path);
    assert_eq!(expected.len(), 64 + ".bin".len());
    assert!(
        expected
            .strip_suffix(".bin")
            .unwrap()
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
    );
    assert_eq!(cache_filename(path), expected);
    assert_ne!(cache_filename(Path::new("/test/other.fits")), expected);
    assert_eq!(
        channel_filename(&expected, 0),
        format!("{}_c0.bin", expected.trim_end_matches(".bin"))
    );
    assert_eq!(channel_filename("frame", 2), "frame_c2.bin");
}

#[test]
fn load_concurrency_accounts_for_resident_and_transient_memory() {
    let cases = [
        (FRAME_96MB, 2 * FRAME_96MB, 20, 27 * GB, 16, 16),
        (FRAME_96MB, 2 * FRAME_96MB, 200, 25 * GB, 16, 1),
        (GB as usize, GB as usize, 0, 4 * GB, 64, 3),
        (GB as usize, 2 * GB as usize, 0, 4 * GB, 64, 1),
        (FRAME_96MB, 2 * FRAME_96MB, 0, 4 * GB, 8, 8),
        (GB as usize, GB as usize, 0, 2 * GB, 16, 1),
        (GB as usize, GB as usize, 0, 8 * GB, 16, 6),
        (0, 0, 0, 0, 16, 1),
        (FRAME_96MB, 2 * FRAME_96MB, 5, 27 * GB, 0, 1),
    ];

    for (resident, transient, frames, available, workers, expected) in cases {
        assert_eq!(
            load_concurrency(resident, transient, frames, available, workers),
            expected
        );
    }
}

#[test]
fn fits_in_memory_honors_budget_boundary_channels_and_overflow() {
    let bytes_per_image = 1000 * 1000 * size_of::<f32>();
    let frame_count = 10;
    let bytes_needed = (bytes_per_image * frame_count) as u64;
    let available_at_boundary = (bytes_needed * 100).div_ceil(75);

    assert!(fits_in_memory(
        bytes_per_image,
        frame_count,
        available_at_boundary
    ));
    assert!(!fits_in_memory(
        bytes_per_image,
        frame_count,
        available_at_boundary - 2
    ));
    assert!(fits_in_memory(6000 * 4000 * 4, 20, 4 * GB));
    assert!(!fits_in_memory(6000 * 4000 * 3 * 4, 20, 4 * GB));
    assert!(!fits_in_memory(usize::MAX, 2, u64::MAX));
}

#[test]
fn optimal_chunk_rows_matches_budget_arithmetic() {
    let cases = [
        (6000, 3, 20, 8 * GB),
        (1000, 3, 5, 4 * GB),
        (8000, 3, 100, 16 * GB),
        (6000, 3, 20, GB),
        (6000, 3, 20, 256 * 1024 * 1024),
        (6000, 1, 20, 8 * GB),
        (100, 1, 2, 0),
    ];

    for (width, channels, frames, available) in cases {
        let input_planes = channels * frames;
        let bytes_per_row = (width * input_planes * size_of::<f32>()) as u64;
        let usable = (available as u128 * 75 / 100) as u64;
        let expected = (usable / bytes_per_row).max(MIN_CHUNK_ROWS as u64) as usize;
        assert_eq!(
            optimal_chunk_rows(
                width,
                100,
                ChunkMemoryLayout {
                    input_planes,
                    resident_planes: 0,
                },
                available,
            ),
            expected
        );
    }

    // 1 MiB available → 786,432 usable bytes. Six resident 100×200 f32 planes consume 480,000
    // bytes; nine active input planes consume 3,600 bytes/row, leaving exactly 85 whole rows.
    assert_eq!(
        optimal_chunk_rows(
            100,
            200,
            ChunkMemoryLayout {
                input_planes: 9,
                resident_planes: 6,
            },
            1024 * 1024,
        ),
        85
    );
    assert_eq!(
        optimal_chunk_rows(
            0,
            100,
            ChunkMemoryLayout {
                input_planes: 60,
                resident_planes: 3,
            },
            8 * GB,
        ),
        MIN_CHUNK_ROWS
    );
    assert_eq!(
        optimal_chunk_rows(
            100,
            200,
            ChunkMemoryLayout {
                input_planes: 9,
                resident_planes: 10,
            },
            1024 * 1024,
        ),
        MIN_CHUNK_ROWS
    );
    assert_eq!(memory_budget(8 * GB), 6 * GB);
}
