use crate::memory::run_memory::RunMemory;
use std::fs::OpenOptions;
use std::io::{Seek, SeekFrom, Write};
use std::time::{Duration, UNIX_EPOCH};

use common::CancelToken;

use crate::combine::cache::loader::*;
use crate::error::FrameDimensionMismatch;
use crate::frame_store::cache_key::DecoderKind;
use crate::frame_store::frame_spill::Carries;
use crate::io::image::cfa::CfaImage;
use crate::io::image::image_dimensions::ImageDimensions;
use crate::io::image::linear::LinearImage;
use common::TempDir;

fn cache_test_frame<I: StackableImage>(
    cache_dir: &Path,
    source: &Path,
    dimensions: ImageDimensions,
    index: usize,
) -> Result<StoredFrame, Error> {
    // No frame 0 admitted: these tests are about one frame's cache files, not the set it belongs to.
    let cancel = CancelToken::never();
    FrameDiskCache::<I> {
        scratch: &RunScratch::create(cache_dir).unwrap(),
        kept: Some(&DecodeCache::open(cache_dir).unwrap()),
        admission: &FrameAdmission::new(dimensions, &cancel),
        context: &LoadContext::new(CancelToken::never(), u64::MAX),
        step: None,
    }
    .frame(source, index, None)
}

/// The directory of the decode cache under `cache_dir`.
fn kept_directory(cache_dir: &Path) -> PathBuf {
    DecodeCache::open(cache_dir).unwrap().path().to_path_buf()
}

fn spill_of<'a, I: StackableImage>(kept: &'a Path, source: &Path) -> FrameSpill<'a> {
    FrameSpill::cached(kept, &fs::canonicalize(source).unwrap(), I::DECODER)
}

/// Overwrite sample `index` of a spilled plane in place, as anything outside lumos could.
fn poke(path: &Path, index: usize, value: f32) {
    let mut file = OpenOptions::new().write(true).open(path).unwrap();
    file.seek(SeekFrom::Start((index * size_of::<f32>()) as u64))
        .unwrap();
    file.write_all(&value.to_le_bytes()).unwrap();
}

fn set_mtime(path: &Path, nanos_past_epoch: u64) {
    OpenOptions::new()
        .write(true)
        .open(path)
        .unwrap()
        .set_modified(UNIX_EPOCH + Duration::from_nanos(nanos_past_epoch))
        .unwrap();
}

/// A first call decodes and commits; a second maps what the first committed. A rewrite of the
/// source that keeps its length but moves its mtime by 100 ns decodes again.
#[test]
fn cache_frame_reuses_a_committed_frame_until_its_source_changes() {
    let temp_dir = TempDir::new("lumos_cache_frame_reuse");
    let dims = ImageDimensions::new((4, 3), 1);
    // [0, 1, …, 11]: median 5.5, absolute deviations 0.5, 0.5, 1.5, 1.5, …, 5.5, 5.5 → MAD 3.0.
    let pixels: Vec<f32> = (0..12).map(|i| i as f32).collect();
    let source = temp_dir.join("source.tiff");
    LinearImage::from_pixels(dims, pixels.clone())
        .save(&source)
        .unwrap();
    set_mtime(&source, 1_700_000_000_000_000_100);

    let first = cache_test_frame::<LinearImage>(temp_dir.path(), &source, dims, 0).unwrap();
    assert_eq!(first.channels[0].chunk(0, 12), pixels);
    assert_eq!(first.source_stats.channels[0].median, 5.5);
    assert_eq!(first.source_stats.channels[0].mad, 3.0);
    drop(first);

    // A sample and the statistics changed on disk come back as they are: the frame was mapped and
    // its statistics read, not measured again.
    let kept = kept_directory(temp_dir.path());
    let spill = spill_of::<LinearImage>(&kept, &source);
    poke(&spill.channel_path(0), 2, 102.0);
    let key = CacheKey::new(
        CachedSource::of(&source).unwrap().identity,
        DecoderKind::Linear,
    );
    let mut sentinel = spill.committed(key).unwrap().stats;
    sentinel.channels[0].median = 99.0;
    spill
        .commit(
            key,
            Carries {
                quality: false,
                flags: false,
            },
            &sentinel,
        )
        .unwrap();
    let reused = cache_test_frame::<LinearImage>(temp_dir.path(), &source, dims, 0).unwrap();
    assert_eq!(reused.channels[0].chunk(0, 3), &[0.0, 1.0, 102.0]);
    assert_eq!(reused.source_stats.channels[0].median, 99.0);
    assert_eq!(reused.source_stats.channels[0].mad, 3.0);
    drop(reused);

    let rewritten: Vec<f32> = (200..212).map(|i| i as f32).collect();
    let original_len = fs::metadata(&source).unwrap().len();
    LinearImage::from_pixels(dims, rewritten.clone())
        .save(&source)
        .unwrap();
    assert_eq!(
        fs::metadata(&source).unwrap().len(),
        original_len,
        "the timestamp, not the length, tells this rewrite apart"
    );
    set_mtime(&source, 1_700_000_000_000_000_200);
    let decoded = cache_test_frame::<LinearImage>(temp_dir.path(), &source, dims, 0).unwrap();
    assert_eq!(decoded.channels[0].chunk(0, 12), rewritten);
}

/// A reused frame is held to the same checks as a decoded one: a non-finite sample, and quality
/// planes whose coverage and confidence disagree on where there is support, each name the pixel.
#[test]
fn cache_frame_validates_a_reused_frame() {
    let temp_dir = TempDir::new("lumos_cache_frame_validates");
    let dims = ImageDimensions::new((4, 3), 1);
    let source = temp_dir.join("source.tiff");
    LinearImage::from_pixels(dims, (0..12).map(|i| i as f32).collect())
        .save(&source)
        .unwrap();
    drop(cache_test_frame::<LinearImage>(temp_dir.path(), &source, dims, 1).unwrap());
    let kept = kept_directory(temp_dir.path());
    let spill = spill_of::<LinearImage>(&kept, &source);

    poke(&spill.channel_path(0), 2, f32::INFINITY);
    let error = cache_test_frame::<LinearImage>(temp_dir.path(), &source, dims, 1).unwrap_err();
    assert!(matches!(
        error,
        Error::NonFiniteImageSample {
            index: 1,
            channel: 0,
            pixel: 2,
            value: f32::INFINITY,
        }
    ));
    poke(&spill.channel_path(0), 2, 2.0);

    // Committed as carrying quality planes, then given a pair with zero confidence at pixel 5 under
    // full coverage.
    let image = LinearImage::from_pixels(dims, (0..12).map(|i| i as f32).collect());
    let mut confidence = vec![1.0f32; 12];
    confidence[5] = 0.0;
    let quality = FrameQuality::Planes {
        coverage: Buffer2::new(4, 3, vec![1.0; 12]),
        confidence: Buffer2::new(4, 3, confidence),
    };
    let key = CacheKey::new(
        CachedSource::of(&source).unwrap().identity,
        DecoderKind::Linear,
    );
    let stats = FrameStats::measure(&image);
    drop(StoredFrame::cache(&spill, key, &image, &quality, stats).unwrap());
    let error = cache_test_frame::<LinearImage>(temp_dir.path(), &source, dims, 1).unwrap_err();
    assert!(
        matches!(
            error,
            Error::FrameQualityPairMismatch {
                index: 1,
                pixel: 5,
                ..
            }
        ),
        "{error:?}"
    );
}

/// A cache written by one decoder is invisible to the other. A float TIFF is a `LinearImage` input
/// and no `CfaImage` one, so a `CfaImage` load of it must decode and be refused — with the cache
/// keyed on the path alone it would map the `LinearImage` planes, which have its size exactly, as a
/// sensor plane.
#[test]
fn a_cache_written_by_one_decoder_is_not_reused_by_another() {
    let temp_dir = TempDir::new("lumos_cache_frame_decoders");
    let dims = ImageDimensions::new((4, 3), 1);
    let source = temp_dir.join("light.tiff");
    LinearImage::from_pixels(dims, (0..12).map(|i| i as f32 / 16.0).collect())
        .save(&source)
        .unwrap();

    drop(cache_test_frame::<LinearImage>(temp_dir.path(), &source, dims, 0).unwrap());
    let kept = kept_directory(temp_dir.path());
    assert_ne!(
        spill_of::<LinearImage>(&kept, &source).channel_path(0),
        spill_of::<CfaImage>(&kept, &source).channel_path(0)
    );
    let error = cache_test_frame::<CfaImage>(temp_dir.path(), &source, dims, 0).unwrap_err();
    assert!(matches!(error, Error::ImageLoad(_)), "{error:?}");
}

/// A kept cache serves a second run: frame 1's planes come back from the first run's files, and
/// frame 0 — always decoded, for the stack's metadata — is committed like the rest, so a run in
/// which it is not first can reuse it.
#[test]
fn a_kept_disk_cache_is_reused_by_the_next_run() {
    let temp_dir = TempDir::new("lumos_cache_kept_run");
    let dims = ImageDimensions::new((4, 3), 1);
    let paths: Vec<PathBuf> = (0..2)
        .map(|index| {
            let path = temp_dir.join(format!("light_{index}.tiff"));
            LinearImage::from_pixels(dims, vec![0.25 + 0.25 * index as f32; 12])
                .save(&path)
                .unwrap();
            path
        })
        .collect();
    let config = StackConfig {
        ingest: IngestConfig {
            cache_dir: temp_dir.join("cache"),
            keep_cache: true,
            ..IngestConfig::default()
        },
        ..StackConfig::default()
    };
    let run = || {
        load_tiered::<LinearImage, _>(
            &paths,
            &config,
            IngestRun::planned(RunMemory::new(1 << 30, Some(1))),
            None,
            ProgressCallback::default(),
        )
        .unwrap()
    };

    let first = run();
    assert!(first.core.tier.spills(), "a one-byte budget spills");
    drop(first);
    let directory = kept_directory(&config.ingest.cache_dir);
    let key = |path: &Path| {
        CacheKey::new(
            CachedSource::of(path).unwrap().identity,
            DecoderKind::Linear,
        )
    };
    for path in &paths {
        assert!(
            spill_of::<LinearImage>(&directory, path)
                .committed(key(path))
                .is_some(),
            "{} was not committed",
            path.display()
        );
    }
    for path in &paths {
        poke(
            &spill_of::<LinearImage>(&directory, path).channel_path(0),
            0,
            0.125,
        );
    }

    let second = run();
    assert_eq!(second.frames[0].channels[0].chunk(0, 1), &[0.25]);
    assert_eq!(second.frames[1].channels[0].chunk(0, 1), &[0.125]);
}

#[test]
fn cache_frame_dimension_mismatch() {
    let temp_dir = TempDir::new("lumos_load_cache_mismatch_test");

    // Create image with different dimensions than expected
    let actual_dims = ImageDimensions::new((4, 3), 1);
    let pixels: Vec<f32> = (0..12).map(|i| i as f32).collect();
    let image = LinearImage::from_pixels(actual_dims, pixels);

    let source_path = temp_dir.join("source.tiff");
    image.save(&source_path).unwrap();

    // Try to load with wrong expected dimensions
    let expected_dims = ImageDimensions::new((8, 6), 1);
    let result = cache_test_frame::<LinearImage>(temp_dir.path(), &source_path, expected_dims, 5);

    assert!(matches!(
        result.unwrap_err(),
        Error::DimensionMismatch(FrameDimensionMismatch {
            index: 5,
            expected,
            actual,
        }) if expected == expected_dims && actual == actual_dims
    ));
}
