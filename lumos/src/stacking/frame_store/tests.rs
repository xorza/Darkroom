use crate::io::image::null_mask::NullMask;
use crate::stacking::frame_store::spill::{CachedQuality, SpillDirectory};
use crate::stacking::frame_store::*;
use crate::testing::ScratchDirectory;

#[test]
fn stored_image_roundtrip_overwrites_stale_pixels() {
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

    // Dropping the image does not remove its planes: the spill directory owns that decision, so
    // that `keep_cache` can hold them. See `spill_directory_removes_planes_unless_asked_to_keep`.
    drop(stored);
    assert!(path.exists());
}

/// The one owner of spilled-file cleanup, and the only thing `keep_cache` acts through.
///
/// Both `StoredImage::spill` and `StoredFrame::spill` write into a directory owned here and keep no
/// per-file guard of their own. A guard on either would delete planes the user asked to keep, and
/// would do it the moment that frame dropped rather than at the end of the run.
#[test]
fn spill_directory_removes_planes_unless_asked_to_keep() {
    let scratch = ScratchDirectory::new("frame_store_keep");
    let dimensions = ImageDimensions::new((2, 2), 1);
    let image = LinearImage::from_pixels(dimensions, vec![0.1, 0.2, 0.3, 0.4]);

    for (keep, should_survive) in [(false, false), (true, true)] {
        let root = scratch.join(format!("keep_{keep}"));
        let directory = SpillDirectory::create(root.clone(), keep).unwrap();

        let stored = StoredImage::spill(&directory.path, "calibrated", &image).unwrap();
        let plane = root.join("calibrated_c0.bin");
        assert!(plane.exists(), "keep={keep}: plane was not written");

        // The frame going away must not take the file with it — only the directory decides.
        drop(stored);
        assert!(
            plane.exists(),
            "keep={keep}: dropping the frame removed its plane"
        );

        drop(directory);
        assert_eq!(
            plane.exists(),
            should_survive,
            "keep={keep}: plane survival is wrong after the directory dropped"
        );
    }
}

#[test]
fn light_frame_keeps_quality_with_its_planes() {
    let dimensions = ImageDimensions::new((2, 2), 1);
    let image = LinearImage::from_pixels(dimensions, vec![1.0, 2.0, 3.0, 4.0]);
    // The last pixel has no support, so its confidence is zero too — the pairing every consumer of
    // a frame-quality pair relies on.
    let coverage = Buffer2::new(2, 2, vec![1.0, 0.5, 0.25, 0.0]);
    let confidence = Buffer2::new(2, 2, vec![4.0, 3.0, 2.0, 0.0]);
    let source_stats = FrameStats::measure(&image);
    let frame = StoredFrame::from_memory(
        image,
        FrameQuality::Planes {
            coverage,
            confidence,
        },
        source_stats,
    );
    assert_eq!(frame.channels[0].chunk(0, 4), &[1.0, 2.0, 3.0, 4.0]);
    assert_eq!(
        frame.quality.coverage().unwrap().chunk(0, 4),
        &[1.0, 0.5, 0.25, 0.0]
    );
    assert_eq!(
        frame.quality.confidence().unwrap().chunk(0, 4),
        &[4.0, 3.0, 2.0, 0.0]
    );
    assert_eq!(frame.source_stats.channels[0].median, 2.5);
    assert_eq!(frame.source_stats.channels[0].mad, 1.0);
}

#[test]
fn frame_statistics_are_measured_over_the_pixels_that_hold_a_measurement() {
    // Eight pixels: four real samples and four the source declared null, which the decoder filled
    // at the frame's own median. Those fills are zero-deviation samples, so counting them collapses
    // the MAD — and MAD is what weighting divides by, so the frame would be trusted far beyond what
    // its noise deserves.
    //
    // Valid 1, 3, 5, 7 → median (3 + 5) / 2 = 4, deviations 3, 1, 1, 3 → MAD (1 + 3) / 2 = 2.
    // All eight → median still 4, deviations 3, 1, 1, 3, 0, 0, 0, 0 → MAD (0 + 1) / 2 = 0.5.
    let dimensions = ImageDimensions::new((8, 1), 1);
    let samples = vec![1.0f32, 3.0, 5.0, 7.0, 4.0, 4.0, 4.0, 4.0];
    let mut masked = LinearImage::from_pixels(dimensions, samples.clone());
    masked.nulls = NullMask::of_non_finite(
        dimensions.size(),
        &[&[0.0, 0.0, 0.0, 0.0, f32::NAN, f32::NAN, f32::NAN, f32::NAN]],
    );

    let stats = FrameStats::measure(&masked);
    assert_eq!(stats.channels[0].median, 4.0);
    assert_eq!(stats.channels[0].mad, 2.0);

    // The same pixels with nothing declared null, so the fills count as data and the spread halves
    // twice over. The two must not agree, or the exclusion is doing nothing.
    let plain = LinearImage::from_pixels(dimensions, samples);
    assert_eq!(FrameStats::measure(&plain).channels[0].mad, 0.5);

    // Nothing measured anywhere has no statistics to report, and asking for the median of an empty
    // set would panic rather than say so.
    let mut all_null = LinearImage::from_pixels(dimensions, vec![4.0; 8]);
    all_null.nulls = NullMask::of_non_finite(dimensions.size(), &[&[f32::NAN; 8]]);
    let empty = FrameStats::measure(&all_null);
    assert_eq!(empty.channels[0].median, 0.0);
    assert_eq!(empty.channels[0].mad, 0.0);
}

#[test]
fn an_unwarped_frames_nulls_become_the_pair_the_combine_gates_on() {
    let dimensions = ImageDimensions::new((2, 2), 1);
    let mut image = LinearImage::from_pixels(dimensions, vec![1.0, 2.0, 3.0, 4.0]);

    // A frame whose source declared nothing undefined carries no planes at all — the case that
    // keeps this free for every RAW frame and almost every camera FITS.
    assert!(FrameQuality::for_unwarped(&image).is_none());

    // Declaring pixel 2 null turns it into zero coverage there and full coverage elsewhere, with
    // confidence matching bit for bit: nothing was interpolated, so every sample that exists is a
    // whole one, and `coverage == 0` exactly where `confidence == 0` as the pairing requires.
    image.nulls = NullMask::of_non_finite(dimensions.size(), &[&[1.0, 2.0, f32::NAN, 4.0]]);
    let quality = FrameQuality::for_unwarped(&image);
    assert_eq!(quality.coverage().unwrap().pixels(), &[1.0, 1.0, 0.0, 1.0]);
    assert_eq!(
        quality.confidence().unwrap().pixels(),
        quality.coverage().unwrap().pixels()
    );
}

#[test]
fn a_spilled_frames_quality_planes_survive_the_cache_round_trip() {
    // The warm-cache case: reusing a frame's channels without its quality planes would put the
    // fill under its nulls back into the stack as data on every run after the first.
    let directory = ScratchDirectory::new("frame_store_cached_quality");
    let dimensions = ImageDimensions::new((2, 2), 1);
    let mut image = LinearImage::from_pixels(dimensions, vec![1.0, 2.0, 3.0, 4.0]);
    image.nulls = NullMask::of_non_finite(dimensions.size(), &[&[1.0, 2.0, f32::NAN, 4.0]]);
    let quality = FrameQuality::for_unwarped(&image);
    let stats = FrameStats::measure(&image);
    let frame = StoredFrame::spill(&directory, "frame.bin", &image, &quality, stats).unwrap();
    drop(frame);

    let spill = FrameSpill::new(&directory, "frame.bin");
    assert_eq!(spill.cached_quality(dimensions), CachedQuality::Present);
    let reread =
        FrameQuality::read_spilled(|kind| StoredPlane::map(spill.quality_path(kind))).unwrap();
    assert_eq!(
        reread.coverage().unwrap().chunk(0, 4),
        &[1.0, 1.0, 0.0, 1.0]
    );
    assert_eq!(
        reread.confidence().unwrap().chunk(0, 4),
        &[1.0, 1.0, 0.0, 1.0]
    );
    drop(reread);

    // A frame that wrote no planes reads back as carrying none, so the two states stay
    // distinguishable rather than both looking like "nothing cached".
    let plain = LinearImage::from_pixels(dimensions, vec![1.0, 2.0, 3.0, 4.0]);
    let stats = FrameStats::measure(&plain);
    let frame = StoredFrame::spill(
        &directory,
        "plain.bin",
        &plain,
        &FrameQuality::for_unwarped(&plain),
        stats,
    )
    .unwrap();
    drop(frame);
    assert_eq!(
        FrameSpill::new(&directory, "plain.bin").cached_quality(dimensions),
        CachedQuality::Absent
    );

    // One plane without the other is neither state, and must not be read as either: the cache is
    // rebuilt instead.
    std::fs::remove_file(spill.quality_path("confidence")).unwrap();
    assert_eq!(spill.cached_quality(dimensions), CachedQuality::Torn);
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
