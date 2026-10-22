use std::panic::catch_unwind;
use std::path::Path;
use std::sync::Arc;

use arrayvec::ArrayVec;
use common::{FileIdentity, ScratchFile, TempDir, fs};
use imaginarium::Buffer2;

use crate::combine::frame_read::frame_gate::FrameGate;
use crate::decode::decoder_kind::DecoderKind;
use crate::frame::admitted::Admitted;
use crate::frame::frame_quality::FrameQuality;
use crate::frame_store::decode_cache::DecodeCache;
use crate::frame_store::decode_cache::cache_key::CacheKey;
use crate::frame_store::decode_cache::frame_spill::FrameSpill;
use crate::frame_store::decode_cache::kept_frame::KeptFrame;
use crate::frame_store::run_scratch::RunScratch;
use crate::frame_store::stored_frame::StoredFrame;
use crate::frame_store::stored_gain::StoredGain;
use crate::frame_store::stored_image::StoredImage;
use crate::frame_store::stored_plane::StoredPlane;
use crate::image::cfa::cfa_type::CfaType;
use crate::image::cfa::demosaic::xtrans::markesteijn::MarkesteijnPasses;
use crate::image::channel_layout::ChannelLayout;
use crate::image::flat_gain::FlatGain;
use crate::image::image_dimensions::ImageDimensions;
use crate::image::linear_image::LinearImage;
use crate::image::pixel_flags::PixelFlags;
use crate::image::quality_flags::QualityFlags;
use crate::internals::panic_message;
use crate::io::fits::fits_load_config::{FitsHduSelector, FitsLoadConfig};
use crate::io::load_ctx::LoadCtx;

/// A parked image reads back with its pixels, metadata and null mask; one with no nulls reads
/// back with none. Its files have no name while it lives where a scratch file keeps none, and none
/// outlive it anywhere.
#[test]
fn a_parked_image_reads_back_and_leaves_no_file() {
    let dir = TempDir::new("frame_store_image");
    let scratch = RunScratch::create(dir.path()).unwrap();
    let dimensions = ImageDimensions::new((3, 2), ChannelLayout::Mono);
    let mut image = LinearImage::from_interleaved(dimensions, vec![0.1, 0.2, 0.3, 0.4, 0.5, 0.6]);
    image.metadata.header.exposure_time = Some(30.0);

    let stored = StoredImage::spill(&scratch, &image).unwrap();
    if !ScratchFile::KEEPS_NAME {
        assert_eq!(dir.entry_count(), 0, "a parked plane kept its name");
    }
    assert_eq!(
        stored.planes().collect::<Vec<_>>(),
        [&[0.1, 0.2, 0.3, 0.4, 0.5, 0.6][..]]
    );
    assert_eq!(stored.metadata.header.exposure_time, Some(30.0));
    assert!(stored.flags().is_none());

    // Pixels 1 and 5 null: the spill tier warps under the same mask as the RAM tier.
    image.set_flags(PixelFlags::of_non_finite(
        dimensions.size(),
        &[&[0.0, f32::NAN, 0.0, 0.0, 0.0, f32::NAN]],
    ));
    let masked = StoredImage::spill(&scratch, &image).unwrap();
    let nulls = masked.flags().expect("the mask is spilled with the planes");
    assert_eq!(nulls.count(QualityFlags::NO_DATA), 2);
    assert_eq!(
        (0..6)
            .map(|idx| nulls.mask_of(QualityFlags::NO_DATA).get(idx))
            .collect::<Vec<_>>(),
        [false, true, false, false, false, true]
    );
    drop((stored, masked));
    assert_eq!(dir.entry_count(), 0, "a parked plane outlived its frame");
}

/// Two runs scratching in one root — a root the caller also keeps files in — each read back their
/// own planes, never see the other's, and leave the caller's file and nothing else. The decode
/// cache under the same root is one directory for every run, so a later run finds what an earlier
/// one kept.
#[test]
fn runs_scratch_privately_beside_a_shared_decode_cache() {
    let root = TempDir::new("frame_store_runs");
    let sentinel = root.join("user_file.txt");
    fs::write(&sentinel, b"not lumos's").unwrap();
    let first = RunScratch::create(root.path()).unwrap();
    let second = RunScratch::create(root.path()).unwrap();
    let planes: Vec<StoredPlane> = (0..4)
        .map(|i| {
            let scratch = if i % 2 == 0 { &first } else { &second };
            scratch.store(&[i as f32; 6]).unwrap()
        })
        .collect();
    for (i, plane) in planes.iter().enumerate() {
        assert_eq!(plane.chunk(0..6), &[i as f32; 6]);
    }
    if !ScratchFile::KEEPS_NAME {
        assert_eq!(root.entry_count(), 1, "a scratch file is visible by name");
    }
    drop((planes, first, second));
    assert_eq!(root.entry_count(), 1);
    assert!(
        fs::is_file(&sentinel),
        "a file lumos did not write was removed"
    );

    let kept = DecodeCache::open(root.path()).unwrap();
    assert_eq!(kept.path(), DecodeCache::open(root.path()).unwrap().path());
    assert_eq!(
        kept.path().parent(),
        Some(fs::canonicalize(root.path()).unwrap().as_path())
    );
}

#[test]
fn light_frame_keeps_quality_with_its_planes() {
    let dimensions = ImageDimensions::new((2, 2), ChannelLayout::Mono);
    let image = LinearImage::from_interleaved(dimensions, vec![1.0, 2.0, 3.0, 4.0]);
    // The last pixel has no support, so its confidence is zero too — the pairing every consumer of
    // a frame-quality pair relies on.
    let coverage = Buffer2::new(2, 2, vec![1.0, 0.5, 0.25, 0.0]);
    let confidence = Buffer2::new(2, 2, vec![4.0, 3.0, 2.0, 0.0]);
    let admitted = Admitted::measure(&image);
    let frame = StoredFrame::from_memory(
        image,
        FrameQuality::Planes {
            coverage,
            confidence,
        },
        admitted,
    );
    assert_eq!(frame.channels[0].chunk(0..4), &[1.0, 2.0, 3.0, 4.0]);
    assert_eq!(
        frame.quality.coverage().unwrap().chunk(0..4),
        &[1.0, 0.5, 0.25, 0.0]
    );
    assert_eq!(
        frame.quality.confidence(0).unwrap().chunk(0..4),
        &[4.0, 3.0, 2.0, 0.0]
    );
    assert_eq!(frame.source_stats.medians[0], 2.5);
}

#[test]
fn an_unwarped_frames_nulls_become_the_pair_the_combine_gates_on() {
    let dimensions = ImageDimensions::new((2, 2), ChannelLayout::Mono);
    let mut image = LinearImage::from_interleaved(dimensions, vec![1.0, 2.0, 3.0, 4.0]);

    // A frame whose source declared nothing undefined carries no planes at all — the case that
    // keeps this free for every RAW frame and almost every camera FITS.
    assert!(FrameQuality::for_unwarped(&image).is_none());

    // Declaring pixel 2 null makes the frame a mask of its nulls, which the stored frame keeps its
    // flags for: the combine gathers every pixel but 2, each at unit confidence, as nothing was
    // interpolated.
    image.set_flags(PixelFlags::of_non_finite(
        dimensions.size(),
        &[&[1.0, 2.0, f32::NAN, 4.0]],
    ));
    let quality = FrameQuality::for_unwarped(&image);
    assert_eq!(quality.mask(), Some(QualityFlags::NO_DATA));
    let frame = StoredFrame::from_memory(image.clone(), quality, Admitted::measure(&image));
    let gate = FrameGate::of(&frame, 0, 0..4);
    assert_eq!(
        (0..4)
            .map(|idx| gate.sample(idx).map(|sample| sample.confidence))
            .collect::<Vec<_>>(),
        [Some(1.0), Some(1.0), None, Some(1.0)]
    );
}

/// A cached frame comes back whole or not at all, in the shape its keep gave it. Its mask returns
/// with its channels — reusing the channels without it would put the fill under its nulls into the
/// stack as data on every run after the first — and a frame committed with one is rebuilt when its
/// flags are gone, rather than read as a frame with no nulls, and one whose plane is shorter than
/// the shape it was committed with is rebuilt too. A frame of no mask keeps its stratified samples
/// and one a flat divided its gain's nodes, as planes of their own that a reuse maps and needs;
/// read back whole it is the image it was decoded to, flat gain and all. Another key finds
/// nothing: another decode version, decoder, X-Trans pass count or FITS HDU.
#[test]
fn a_cached_frame_is_reused_only_whole_and_under_its_key() {
    let dir = TempDir::new("frame_store_cached_quality");
    let dimensions = ImageDimensions::new((2, 2), ChannelLayout::Mono);
    let identity = FileIdentity {
        len: 16,
        mtime_ns: 1,
        file_id: None,
        changed_ns: None,
    };
    let key = CacheKey::new(identity, DecoderKind::Linear, &LoadCtx::default());
    let mut image = LinearImage::from_interleaved(dimensions, vec![1.0, 2.0, 3.0, 4.0]);
    image.set_flags(PixelFlags::of_non_finite(
        dimensions.size(),
        &[&[1.0, 2.0, f32::NAN, 4.0]],
    ));
    let cache = |name, image: &LinearImage| {
        let spill = FrameSpill::named(dir.path(), name, key);
        let kept = KeptFrame::keep(&spill, image, Admitted::measure(image)).unwrap();
        (spill, kept.into_frame())
    };

    let (spill, written) = cache("masked", &image);
    assert!(written.samples.is_none());
    drop(written);
    let reused = KeptFrame::reuse(&spill).unwrap();
    assert_eq!(reused.dimensions(), dimensions);
    let frame = reused.into_frame();
    assert_eq!(frame.channels[0].chunk(0..4), &[1.0, 2.0, 3.0, 4.0]);
    assert_eq!(frame.quality.mask(), Some(QualityFlags::NO_DATA));
    assert!(frame.samples.is_none());
    assert_eq!(
        frame.flags.as_ref().unwrap().chunk(0..4),
        &[0, 0, QualityFlags::NO_DATA.byte(), 0]
    );
    drop(frame);
    // Read back whole, the frame is the image it was decoded to, flags and metadata with it.
    let restored: LinearImage = KeptFrame::reuse(&spill).unwrap().into_image();
    assert_eq!(restored.channel(0).pixels(), image.channel(0).pixels());
    assert_eq!(
        restored.flags().unwrap().bytes(),
        image.flags().unwrap().bytes()
    );
    assert_eq!(
        format!("{:?}", restored.metadata),
        format!("{:?}", image.metadata)
    );

    let other_version = CacheKey {
        decode_version: key.decode_version ^ 1,
        ..key
    };
    let other_decoder = CacheKey {
        decoder: DecoderKind::Cfa,
        ..key
    };
    let three_passes = LoadCtx {
        xtrans_passes: MarkesteijnPasses::Three,
        ..LoadCtx::default()
    };
    let second_hdu = LoadCtx {
        fits: FitsLoadConfig {
            hdu: FitsHduSelector::Idx(1),
            ..FitsLoadConfig::default()
        },
        ..LoadCtx::default()
    };
    let other_options =
        [three_passes, second_hdu].map(|ctx| CacheKey::new(identity, DecoderKind::Linear, &ctx));
    for other in [other_version, other_decoder]
        .into_iter()
        .chain(other_options)
    {
        let other = FrameSpill::named(dir.path(), "masked", other);
        assert!(KeptFrame::reuse(&other).is_none());
    }

    fs::remove_file(spill.flags_path()).unwrap();
    assert!(
        KeptFrame::reuse(&spill).is_none(),
        "a mask without its flags"
    );

    // A frame of no mask keeps one slot of samples, on the first run as on a reuse.
    let plain = LinearImage::from_interleaved(dimensions, vec![1.0, 2.0, 3.0, 4.0]);
    let (spill, written) = cache("plain", &plain);
    assert_eq!(written.samples.as_ref().map(ArrayVec::len), Some(1));
    drop(written);
    let frame = KeptFrame::reuse(&spill).unwrap().into_frame();
    assert!(frame.quality.is_none());
    assert!(frame.flat_gain.is_none());
    assert_eq!(frame.samples.as_ref().map(ArrayVec::len), Some(1));
    drop(frame);
    fs::remove_file(spill.samples_path(0)).unwrap();
    assert!(KeptFrame::reuse(&spill).is_none(), "samples gone");

    // A frame a flat divided keeps its gain's nodes as a plane, mapped on reuse — the stored gain
    // the combine reads, one channel of it — and back in RAM, the same grid, when read whole.
    let gain = Arc::new(FlatGain::of_divisor(
        &Buffer2::new(2, 2, vec![0.5, 0.25, 1.0, 0.5]),
        &CfaType::Mono,
        |_| false,
    ));
    let mut flattened = plain.clone();
    flattened.set_flat_gain(Some(Arc::clone(&gain)));
    let (spill, written) = cache("flattened", &flattened);
    drop(written);
    let frame = KeptFrame::reuse(&spill).unwrap().into_frame();
    let stored = frame.flat_gain.as_ref().unwrap();
    assert!(matches!(stored, StoredGain::Mapped { .. }));
    assert_eq!(stored.channels(), 1);
    drop(frame);
    let restored: LinearImage = KeptFrame::reuse(&spill).unwrap().into_image();
    assert!(restored.flat_gain().unwrap().planes().eq(gain.planes()));
    let nodes = fs::read(spill.gain_path(0)).unwrap();
    fs::write(spill.gain_path(0), &nodes[..4]).unwrap();
    assert!(KeptFrame::reuse(&spill).is_none(), "a gain plane cut short");
    fs::write(spill.gain_path(0), &nodes).unwrap();

    let channel = fs::read(spill.channel_path(0)).unwrap();
    fs::write(spill.channel_path(0), &channel[..12]).unwrap();
    assert!(
        KeptFrame::reuse(&spill).is_none(),
        "a plane shorter than the shape it was committed with"
    );
    fs::write(spill.channel_path(0), &channel).unwrap();
    assert!(
        KeptFrame::reuse(&spill).is_some(),
        "the planes as committed"
    );
    fs::remove_file(spill.channel_path(0)).unwrap();
    assert!(KeptFrame::reuse(&spill).is_none());
}

/// A published plane maps back as written, from the handle that wrote it: a plane published under
/// the same name after it replaces the name, and leaves the first map reading its own samples.
#[test]
fn a_published_plane_maps_back_what_its_writer_wrote() {
    let dir = TempDir::new("frame_store_plane");
    let path = dir.join("plane.bin");
    let pixels: Vec<f32> = (0..12).map(|value| value as f32).collect();
    let first = StoredPlane::publish(&path, &pixels).unwrap();
    let second = StoredPlane::publish(&path, &[7.0f32; 12]).unwrap();
    assert_eq!(first.chunk(0..12), pixels);
    assert_eq!(second.chunk(0..12), &[7.0; 12]);
    let named = StoredPlane::<f32>::map(&path).unwrap();
    assert_eq!(named.chunk(0..12), &[7.0; 12]);
}

/// A cached frame's files are named by its source and its whole key: the same pair always finds
/// the same files, and another path, decoder, decode version or options never does. Every file of
/// one frame hangs off one stem.
#[test]
fn spill_names_are_stable_per_source_and_key_and_share_one_stem() {
    let path = Path::new("/test/deterministic.fits");
    let cache_dir = Path::new("/cache");
    let key = |decoder| {
        CacheKey::new(
            FileIdentity {
                len: 16,
                mtime_ns: 1,
                file_id: None,
                changed_ns: None,
            },
            decoder,
            &LoadCtx::default(),
        )
    };
    let linear = key(DecoderKind::Linear);
    let hashed = FrameSpill::cached(cache_dir, path, linear);
    let channel = hashed.channel_path(0);
    let stem = channel
        .file_name()
        .unwrap()
        .to_str()
        .unwrap()
        .strip_suffix("_c0.bin")
        .unwrap()
        .to_owned();
    assert_eq!(stem.len(), 16);
    assert!(
        stem.bytes()
            .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
    );
    assert_eq!(
        FrameSpill::cached(cache_dir, path, linear).channel_path(0),
        channel
    );
    for other in [
        FrameSpill::cached(cache_dir, Path::new("/test/other.fits"), linear),
        FrameSpill::cached(cache_dir, path, key(DecoderKind::Cfa)),
        FrameSpill::cached(
            cache_dir,
            path,
            CacheKey {
                decode_version: linear.decode_version ^ 1,
                ..linear
            },
        ),
        FrameSpill::cached(
            cache_dir,
            path,
            CacheKey {
                options: linear.options ^ 1,
                ..linear
            },
        ),
    ] {
        assert_ne!(other.channel_path(0), channel);
    }
    assert_eq!(
        hashed.flags_path(),
        cache_dir.join(format!("{stem}_flags.bin"))
    );
    assert_eq!(
        hashed.samples_path(1),
        cache_dir.join(format!("{stem}_s1.bin"))
    );
    assert_eq!(
        hashed.gain_path(2),
        cache_dir.join(format!("{stem}_g2.bin"))
    );

    let plain = FrameSpill::named(cache_dir, "frame", linear);
    assert_eq!(plain.channel_path(2), cache_dir.join("frame_c2.bin"));
    assert_eq!(plain.flags_path(), cache_dir.join("frame_flags.bin"));
}

/// A flat gain covers the image that holds it; one set on an image of another size, or of another
/// channel count, is refused where it is set, rather than read past its grid in the combine. A
/// grid over the right pixels and channels is taken.
#[test]
fn a_flat_gain_of_another_image_is_refused() {
    let store = |size: (usize, usize), channels: usize| {
        let gain = Arc::new(FlatGain::of_divisor(
            &Buffer2::new(size.0, size.1, vec![0.5; size.0 * size.1]),
            &CfaType::Mono,
            |_| false,
        ));
        catch_unwind(move || {
            let mut image = LinearImage::from_interleaved(
                ImageDimensions::new((8, 8), ChannelLayout::try_from(channels).unwrap()),
                vec![0.5; 64 * channels],
            );
            image.set_flat_gain(Some(gain));
            let stats = Admitted::measure(&image);
            StoredFrame::from_memory(image, FrameQuality::None, stats);
        })
        .map_err(|payload| panic_message(payload.as_ref()))
    };
    assert!(store((8, 8), 1).is_ok());
    for (size, channels) in [((4, 4), 1), ((8, 8), 3)] {
        let message = store(size, channels).unwrap_err();
        assert!(
            message.contains("an image's flat gain covers another image's pixels"),
            "{size:?} × {channels}: {message}"
        );
    }
}
