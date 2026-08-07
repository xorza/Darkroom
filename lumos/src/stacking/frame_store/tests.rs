use crate::stacking::frame_store::*;
use crate::testing::ScratchDirectory;

#[test]
fn stored_image_roundtrip_overwrites_stale_pixels_and_cleans_spill_files() {
    let directory = ScratchDirectory::new("frame_store_image");
    let dimensions = ImageDimensions::new((2, 2), 1);
    let mut image = LinearImage::from_pixels(dimensions, vec![0.1, 0.2, 0.3, 0.4]);
    image.metadata.exposure_time = Some(30.0);
    let path = directory.join("calibrated_c0.bin");
    write_plane(&path, &[9.0; 4]).unwrap();

    let stored = StoredImage::spill(&directory, "calibrated", &image).unwrap();
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
    let source_stats = FrameStats::measure(&image);
    let frame = StoredFrame::from_memory(
        image,
        WarpQuality::new(Some(coverage), Some(confidence)),
        source_stats,
    );
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
fn plane_persistence_roundtrips_pixels() {
    let directory = ScratchDirectory::new("frame_store_plane");
    let path = directory.join("plane.bin");
    let pixels: Vec<f32> = (0..12).map(|value| value as f32).collect();
    write_plane(&path, &pixels).unwrap();

    let mapped = StoredPlane::map(path.clone()).unwrap();
    assert_eq!(mapped.chunk(0, pixels.len()), pixels);

    drop(mapped);
}

#[test]
fn spill_names_are_stable_path_specific_and_share_one_stem() {
    let path = Path::new("/test/deterministic.fits");
    let expected = FrameSpill::cache_name(path);
    assert_eq!(expected.len(), 64 + ".bin".len());
    assert!(
        expected
            .strip_suffix(".bin")
            .unwrap()
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
    );
    assert_eq!(FrameSpill::cache_name(path), expected);
    assert_ne!(
        FrameSpill::cache_name(Path::new("/test/other.fits")),
        expected
    );

    // Every file of one frame hangs off the same stem: the `.bin` of a cache name is stripped
    // once, so channels and quality planes sit beside each other instead of one gaining a
    // doubled extension.
    let stem = expected.trim_end_matches(".bin");
    let cache_dir = Path::new("/cache");
    let hashed = FrameSpill::new(cache_dir, &expected);
    assert_eq!(
        hashed.channel_path(0),
        cache_dir.join(format!("{stem}_c0.bin"))
    );
    assert_eq!(
        hashed.quality_path("coverage"),
        cache_dir.join(format!("{stem}_coverage.bin"))
    );

    let plain = FrameSpill::new(cache_dir, "frame");
    assert_eq!(plain.channel_path(2), cache_dir.join("frame_c2.bin"));
    assert_eq!(
        plain.quality_path("confidence"),
        cache_dir.join("frame_confidence.bin")
    );
}

#[test]
fn channels_reusable_requires_every_plane_at_the_expected_size() {
    let directory = ScratchDirectory::new("frame_store_reuse");
    let dimensions = ImageDimensions::new((4, 3), 3);
    let spill = FrameSpill::new(&directory, "reuse");

    // 4×3 f32 = 48 bytes per plane, three planes. Nothing on disk yet.
    assert!(!spill.channels_reusable(dimensions));

    write_plane(&spill.channel_path(0), &[0.0f32; 12]).unwrap();
    write_plane(&spill.channel_path(1), &[0.0f32; 12]).unwrap();
    assert!(
        !spill.channels_reusable(dimensions),
        "two of three channels present is not reusable"
    );

    write_plane(&spill.channel_path(2), &[0.0f32; 12]).unwrap();
    assert!(spill.channels_reusable(dimensions));

    // Same files, geometry that implies 8×3 = 24 pixels = 96 bytes: stale, not reusable.
    assert!(!spill.channels_reusable(ImageDimensions::new((8, 3), 3)));
}
