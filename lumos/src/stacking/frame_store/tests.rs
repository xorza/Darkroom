use crate::stacking::frame_store::*;
use crate::testing::ScratchDirectory;

const GB: u64 = 1024 * 1024 * 1024;
const FRAME_96MB: usize = 6240 * 4160 * size_of::<f32>();

#[test]
fn stored_image_roundtrip_overwrites_stale_pixels_and_cleans_spill_files() {
    let directory = ScratchDirectory::new("frame_store_image");
    let dimensions = ImageDimensions::new((2, 2), 1);
    let mut image = AstroImage::from_pixels(dimensions, vec![0.1, 0.2, 0.3, 0.4]);
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
fn light_frame_keeps_statistics_with_its_planes() {
    let dimensions = ImageDimensions::new((2, 2), 1);
    let image = AstroImage::from_pixels(dimensions, vec![1.0, 2.0, 3.0, 4.0]);
    let frame = StoredLightFrame::from_memory(image, None);
    assert_eq!(frame.channels[0].chunk(0, 4), &[1.0, 2.0, 3.0, 4.0]);
    assert_eq!(frame.stats.channels[0].median, 2.5);
    assert_eq!(frame.stats.channels[0].mad, 1.0);
    assert!(frame.coverage.is_none());
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
    let expected = "6f63e2eb959a4c65.bin";
    assert_eq!(cache_filename(path), expected);
    assert_eq!(cache_filename(path), expected);
    assert_ne!(cache_filename(Path::new("/test/other.fits")), expected);
    assert_eq!(channel_filename(expected, 0), "6f63e2eb959a4c65_c0.bin");
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
        let bytes_per_row = (width * channels * size_of::<f32>() * frames) as u64;
        let usable = (available as u128 * 75 / 100) as u64;
        let expected = (usable / bytes_per_row).max(MIN_CHUNK_ROWS as u64) as usize;
        assert_eq!(
            optimal_chunk_rows(width, channels, frames, available),
            expected
        );
    }

    assert_eq!(optimal_chunk_rows(0, 3, 20, 8 * GB), MIN_CHUNK_ROWS);
    assert_eq!(memory_budget(8 * GB), 6 * GB);
}
