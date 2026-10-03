use crate::stacking::frame_store::frame_quality::FramePlane;
use crate::stacking::frame_store::spill_directory::SpillDirectory;
use crate::stacking::frame_store::spill_directory::internals::{marker, stale_run_directory};
use crate::stacking::frame_store::*;
use common::{FileIdentity, TempDir};
use std::fs;

/// A spilled image reads back with its pixels, metadata and null mask, over whatever stale file
/// held its name; one with no nulls writes no mask and reads back with none.
#[test]
fn stored_image_roundtrip_overwrites_stale_pixels() {
    let directory = TempDir::new("frame_store_image");
    let dimensions = ImageDimensions::new((3, 2), 1);
    let mut image = LinearImage::from_pixels(dimensions, vec![0.1, 0.2, 0.3, 0.4, 0.5, 0.6]);
    image.metadata.exposure_time = Some(30.0);
    let spill = FrameSpill::new(directory.path(), "calibrated");
    let path = spill.channel_path(0);
    StoredPlane::write(&path, &[9.0; 6]).unwrap();

    let stored = StoredImage::spill(&spill, &image).unwrap();
    let loaded = stored.load();
    assert_eq!(loaded.channel(0).pixels(), &[0.1, 0.2, 0.3, 0.4, 0.5, 0.6]);
    assert_eq!(loaded.metadata.exposure_time, Some(30.0));
    assert!(loaded.nulls.is_none());
    assert!(!spill.nulls_path().exists());

    // Dropping the image does not remove its planes: the spill directory owns that decision, so
    // that `keep_cache` can hold them. See `spill_directory_removes_planes_unless_asked_to_keep`.
    drop(stored);
    assert!(path.exists());

    // Pixels 1 and 5 null: the spill tier warps under the same mask as the RAM tier.
    let masked_spill = FrameSpill::new(directory.path(), "masked");
    image.nulls = NullMask::of_non_finite(
        dimensions.size(),
        &[&[0.0, f32::NAN, 0.0, 0.0, 0.0, f32::NAN]],
    );
    let loaded = StoredImage::spill(&masked_spill, &image).unwrap().load();
    let nulls = loaded.nulls.expect("the mask is spilled with the planes");
    assert_eq!(nulls.count(), 2);
    assert_eq!(
        (0..6)
            .map(|index| nulls.bits().get(index))
            .collect::<Vec<_>>(),
        [false, true, false, false, false, true]
    );
}

/// The one owner of spilled-file cleanup, and the only thing `keep_cache` acts through.
///
/// Both `StoredImage::spill` and `StoredFrame::spill` write into a directory owned here and keep no
/// per-file guard of their own. A guard on either would delete planes the user asked to keep, and
/// would do it the moment that frame dropped rather than at the end of the run.
///
/// The root is a directory the caller already uses for something else: a run must remove only the
/// subdirectory it created there, whatever `keep_cache` says.
#[test]
fn spill_directory_removes_only_its_own_planes_unless_asked_to_keep() {
    let scratch = TempDir::new("frame_store_keep");
    let dimensions = ImageDimensions::new((2, 2), 1);
    let image = LinearImage::from_pixels(dimensions, vec![0.1, 0.2, 0.3, 0.4]);

    for (keep, should_survive) in [(false, false), (true, true)] {
        let root = scratch.join(format!("keep_{keep}"));
        fs::create_dir_all(&root).unwrap();
        let sentinel = root.join("user_file.txt");
        fs::write(&sentinel, b"not lumos's").unwrap();

        let directory = SpillDirectory::create(&root, keep).unwrap();
        assert_eq!(directory.path().parent(), Some(root.as_path()));
        assert!(directory.path().join(marker()).is_file());

        let spill = FrameSpill::new(directory.path(), "calibrated");
        let stored = StoredImage::spill(&spill, &image).unwrap();
        let plane = spill.channel_path(0);
        assert!(plane.exists(), "keep={keep}: plane was not written");

        // The frame going away must not take the file with it — only the directory decides.
        drop(stored);
        assert!(
            plane.exists(),
            "keep={keep}: dropping the frame removed its plane"
        );

        let spill_path = directory.path().to_path_buf();
        drop(directory);
        assert_eq!(
            plane.exists(),
            should_survive,
            "keep={keep}: plane survival is wrong after the directory dropped"
        );
        assert_eq!(spill_path.exists(), should_survive);
        assert!(root.is_dir(), "keep={keep}: the caller's root was removed");
        assert!(
            sentinel.is_file(),
            "keep={keep}: a file lumos did not write was removed"
        );
    }
}

/// Two runs without `keep_cache` never share a directory, so one run's drop cannot remove the
/// other's planes; two runs with it always share one, so the second can reuse the first's planes.
#[test]
fn spill_directories_are_per_run_unless_kept() {
    let scratch = TempDir::new("frame_store_per_run");
    let root = scratch.join("root");

    let first = SpillDirectory::create(&root, false).unwrap();
    let second = SpillDirectory::create(&root, false).unwrap();
    assert_ne!(first.path(), second.path());
    let second_path = second.path().to_path_buf();
    drop(first);
    assert!(second_path.is_dir(), "dropping one run removed another");
    drop(second);

    let first = SpillDirectory::create(&root, true).unwrap();
    let second = SpillDirectory::create(&root, true).unwrap();
    assert_eq!(first.path(), second.path());
}

/// A run killed before its drop leaves a marked directory behind; the next run removes it. An
/// unmarked directory of the same shape belongs to someone else and stays.
#[test]
fn stale_run_directories_are_removed_only_when_marked() {
    let scratch = TempDir::new("frame_store_stale");
    let root = scratch.join("root");
    let stale = stale_run_directory(&root);
    let foreign = root.join(format!("run-{}-1", u32::MAX));
    fs::create_dir_all(&foreign).unwrap();

    let directory = SpillDirectory::create(&root, false).unwrap();
    assert!(!stale.exists(), "a dead run's marked directory was kept");
    assert!(foreign.is_dir(), "an unmarked directory was removed");
    drop(directory);
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

/// One median and MAD per channel, each over that channel alone. Hand-computed:
/// - `[1..=9]`: median 5, absolute deviations 4,3,2,1,0,1,2,3,4 → MAD 2.
/// - `[10,10,10,20,20,20,30,30,30]`: median 20, deviations six 10s and three 0s → MAD 10.
/// - `[1,3,5,7]`: median (3+5)/2 = 4, deviations 3,1,1,3 → MAD (1+3)/2 = 2.
/// - `[0,0,100,100]`: median 50, every deviation 50 → MAD 50.
/// - `[1,2,3,4]`: median 2.5, deviations 1.5,0.5,0.5,1.5 → MAD 1.
#[test]
fn frame_statistics_are_a_median_and_mad_per_channel() {
    let gray = ImageDimensions::new((3, 3), 1);
    let rgb = ImageDimensions::new((2, 2), 3);
    let cases: [(LinearImage, &[(f32, f32)]); 4] = [
        (LinearImage::from_pixels(gray, vec![5.0; 9]), &[(5.0, 0.0)]),
        (
            LinearImage::from_pixels(gray, (1..=9).map(|i| i as f32).collect()),
            &[(5.0, 2.0)],
        ),
        (
            LinearImage::from_pixels(
                gray,
                vec![10.0, 10.0, 10.0, 20.0, 20.0, 20.0, 30.0, 30.0, 30.0],
            ),
            &[(20.0, 10.0)],
        ),
        (
            LinearImage::from_planar_channels(
                rgb,
                [
                    vec![1.0, 3.0, 5.0, 7.0],
                    vec![0.0, 0.0, 100.0, 100.0],
                    vec![1.0, 2.0, 3.0, 4.0],
                ],
            ),
            &[(4.0, 2.0), (50.0, 50.0), (2.5, 1.0)],
        ),
    ];
    for (image, expected) in cases {
        let stats = FrameStats::measure(&image);
        let measured: Vec<(f32, f32)> = stats
            .channels
            .iter()
            .map(|channel| (channel.median, channel.mad))
            .collect();
        assert_eq!(measured, expected);
    }
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

/// A cached frame comes back whole or not at all. Its quality planes return with its channels —
/// reusing the channels without them would put the fill under its nulls into the stack as data on
/// every run after the first — and a frame committed with them is rebuilt when one or both are
/// gone, rather than read as a frame with no nulls. Another key finds nothing.
#[test]
fn a_cached_frame_is_reused_only_whole_and_under_its_key() {
    let directory = TempDir::new("frame_store_cached_quality");
    let dimensions = ImageDimensions::new((2, 2), 1);
    let key = CacheKey::new(
        FileIdentity {
            len: 16,
            mtime_ns: 1,
        },
        DecoderKind::Linear,
    );
    let mut image = LinearImage::from_pixels(dimensions, vec![1.0, 2.0, 3.0, 4.0]);
    image.nulls = NullMask::of_non_finite(dimensions.size(), &[&[1.0, 2.0, f32::NAN, 4.0]]);
    let cache = |name, image: &LinearImage| {
        let spill = FrameSpill::new(directory.path(), name);
        let quality = FrameQuality::for_unwarped(image);
        drop(StoredFrame::cache(&spill, key, image, &quality, FrameStats::measure(image)).unwrap());
        spill
    };

    let spill = cache("masked", &image);
    let reused = StoredFrame::reuse(&spill, key, dimensions)
        .unwrap()
        .unwrap();
    assert_eq!(reused.channels[0].chunk(0, 4), &[1.0, 2.0, 3.0, 4.0]);
    assert_eq!(
        reused.quality.coverage().unwrap().chunk(0, 4),
        &[1.0, 1.0, 0.0, 1.0]
    );
    assert_eq!(
        reused.quality.confidence().unwrap().chunk(0, 4),
        &[1.0, 1.0, 0.0, 1.0]
    );
    drop(reused);

    let other_version = CacheKey {
        decode_version: key.decode_version ^ 1,
        ..key
    };
    let other_decoder = CacheKey {
        decoder: DecoderKind::Cfa,
        ..key
    };
    for other in [other_version, other_decoder] {
        assert!(
            StoredFrame::reuse(&spill, other, dimensions)
                .unwrap()
                .is_none()
        );
    }
    assert!(
        StoredFrame::reuse(&spill, key, ImageDimensions::new((4, 1), 1))
            .unwrap()
            .is_some(),
        "the same sample count is the same plane size"
    );
    assert!(
        StoredFrame::reuse(&spill, key, ImageDimensions::new((3, 2), 1))
            .unwrap()
            .is_none(),
        "planes of another size"
    );

    fs::remove_file(spill.quality_path(FramePlane::Confidence)).unwrap();
    assert!(
        StoredFrame::reuse(&spill, key, dimensions)
            .unwrap()
            .is_none()
    );
    fs::remove_file(spill.quality_path(FramePlane::Coverage)).unwrap();
    assert!(
        StoredFrame::reuse(&spill, key, dimensions)
            .unwrap()
            .is_none()
    );

    let plain = LinearImage::from_pixels(dimensions, vec![1.0, 2.0, 3.0, 4.0]);
    let spill = cache("plain", &plain);
    let reused = StoredFrame::reuse(&spill, key, dimensions)
        .unwrap()
        .unwrap();
    assert!(reused.quality.is_none());
    drop(reused);
    fs::remove_file(spill.channel_path(0)).unwrap();
    assert!(
        StoredFrame::reuse(&spill, key, dimensions)
            .unwrap()
            .is_none()
    );
}

#[test]
fn plane_persistence_roundtrips_pixels() {
    let directory = TempDir::new("frame_store_plane");
    let path = directory.join("plane.bin");
    let pixels: Vec<f32> = (0..12).map(|value| value as f32).collect();
    StoredPlane::write(&path, &pixels).unwrap();

    let mapped = StoredPlane::map(path.clone()).unwrap();
    assert_eq!(mapped.chunk(0, pixels.len()), pixels);

    drop(mapped);
}

/// A cached frame's files are named by its source and decoder: the same pair always finds the same
/// files, and another path or decoder never does. Every file of one frame hangs off one stem.
#[test]
fn spill_names_are_stable_per_source_and_decoder_and_share_one_stem() {
    let path = Path::new("/test/deterministic.fits");
    let cache_dir = Path::new("/cache");
    let hashed = FrameSpill::cached(cache_dir, path, DecoderKind::Linear);
    let channel = hashed.channel_path(0);
    let stem = channel
        .file_name()
        .unwrap()
        .to_str()
        .unwrap()
        .strip_suffix("_c0.bin")
        .unwrap()
        .to_owned();
    assert_eq!(stem.len(), 64);
    assert!(
        stem.bytes()
            .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
    );
    assert_eq!(
        FrameSpill::cached(cache_dir, path, DecoderKind::Linear).channel_path(0),
        channel
    );
    for other in [
        FrameSpill::cached(
            cache_dir,
            Path::new("/test/other.fits"),
            DecoderKind::Linear,
        ),
        FrameSpill::cached(cache_dir, path, DecoderKind::Cfa),
    ] {
        assert_ne!(other.channel_path(0), channel);
    }
    assert_eq!(
        hashed.quality_path(FramePlane::Coverage),
        cache_dir.join(format!("{stem}_coverage.bin"))
    );

    let plain = FrameSpill::new(cache_dir, "frame");
    assert_eq!(plain.channel_path(2), cache_dir.join("frame_c2.bin"));
    assert_eq!(
        plain.quality_path(FramePlane::Confidence),
        cache_dir.join("frame_confidence.bin")
    );
    assert_eq!(plain.nulls_path(), cache_dir.join("frame_nulls.bin"));
}

#[test]
fn channels_on_disk_requires_every_plane_at_the_expected_size() {
    let directory = TempDir::new("frame_store_reuse");
    let dimensions = ImageDimensions::new((4, 3), 3);
    let spill = FrameSpill::new(directory.path(), "reuse");

    // 4×3 f32 = 48 bytes per plane, three planes. Nothing on disk yet.
    assert!(!spill.channels_on_disk(dimensions));

    StoredPlane::write(&spill.channel_path(0), &[0.0f32; 12]).unwrap();
    StoredPlane::write(&spill.channel_path(1), &[0.0f32; 12]).unwrap();
    assert!(
        !spill.channels_on_disk(dimensions),
        "two of three channels present is not reusable"
    );

    StoredPlane::write(&spill.channel_path(2), &[0.0f32; 12]).unwrap();
    assert!(spill.channels_on_disk(dimensions));

    // Same files, geometry that implies 8×3 = 24 pixels = 96 bytes: stale, not reusable.
    assert!(!spill.channels_on_disk(ImageDimensions::new((8, 3), 3)));
}
