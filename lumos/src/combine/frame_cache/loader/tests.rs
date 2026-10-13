use std::fs::{self, OpenOptions};
use std::io::{Seek, SeekFrom, Write};
use std::path::PathBuf;
use std::thread;
use std::time::{Duration, UNIX_EPOCH};

use common::TempDir;
use fits_well::image::Image as FitsImage;

use crate::combine::frame_cache::loader::*;
use crate::combine::stack_config::{Normalization, Weighting};
use crate::frame_store::decode_cache::frame_spill::{Carries, Decoded, FrameSpill};
use crate::frame_store::run_scratch::RunScratch;
use crate::frame_store::tier::CombineBudget;
use crate::image::cfa::cfa_image::CfaImage;
use crate::image::image_dimensions::ImageDimensions;
use crate::image::linear_image::LinearImage;
use crate::ingest::error::AdmissionError;
use crate::ingest::frame_step::NoStep;
use crate::ingest::ingest_config::IngestConfig;
use crate::internals::fits::write_fits;
use crate::memory::run_memory::RunMemory;

/// Frame `index` of a set of `dimensions`, decoded from `source`.
#[derive(Debug, Clone, Copy)]
struct SetFrame<'a> {
    source: &'a Path,
    dimensions: ImageDimensions,
    index: usize,
}

impl SetFrame<'_> {
    /// The frame loaded as an `I` through the decode cache under `cache_dir` into a spilled tier.
    fn load<I: StackableImage>(self, cache_dir: &Path) -> Result<StoredFrame, StackError> {
        let Self {
            source,
            dimensions,
            index,
        } = self;
        // A fresh intake per call: these tests are about one frame's cache files, not the set it
        // belongs to, so no frame 0 is admitted before it.
        let run = IngestRun::planned(
            RunMemory::new(1 << 40, None),
            &IngestConfig::with_cache_dir(cache_dir.to_path_buf()),
        );
        let intake = FrameIntake::new(
            IntakePlan {
                dimensions,
                frame_count: index + 1,
                decode_slots: 1,
                stage: StackingStage::Loading,
            },
            &run,
        );
        let tier = Tier::Spilled {
            scratch: RunScratch::create(cache_dir).unwrap(),
            combine: CombineBudget::new(RunMemory::new(1 << 30, None).budget(1)),
        };
        let paths = vec![source; index + 1];
        FrameLoad::<I, NoStep> {
            paths: &paths,
            tier: &tier,
            kept: Some(&DecodeCache::open(cache_dir).unwrap()),
            intake: &intake,
            step: &NoStep,
            image: PhantomData,
        }
        .frame(index, None)
        .map(|loaded| loaded.frame)
    }
}

/// The directory of the decode cache under `cache_dir`.
fn kept_directory(cache_dir: &Path) -> PathBuf {
    DecodeCache::open(cache_dir).unwrap().path().to_path_buf()
}

/// `path` decoded by `I` under the default context, as the decode cache names and keys it.
fn source_of<I: StackableImage>(path: &Path) -> CachedSource {
    CachedSource::of(path, I::DECODER, &LoadContext::default()).unwrap()
}

/// The files `source` is kept in under `kept`.
fn spill_of<'a>(kept: &'a Path, source: &'a CachedSource) -> FrameSpill<'a> {
    FrameSpill::cached(kept, &source.canonical, source.key)
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

/// A first call decodes and commits; a second maps what the first committed. Each of these decodes
/// again instead: a plane zeroed behind its commit, as a power loss can leave one; a source made
/// read-only, which moves its change time alone; a rewrite of the source that keeps its length and
/// moves its mtime by 100 ns; and one that keeps both, its mtime set back. The kernel stamps the
/// change time from a clock as coarse as a few milliseconds, so each change of the source waits
/// 20 ms past the last.
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
    let frame = SetFrame {
        source: &source,
        dimensions: dims,
        index: 0,
    };

    let first = frame.load::<LinearImage>(temp_dir.path()).unwrap();
    assert_eq!(first.channels[0].chunk(0..12), pixels);
    assert_eq!(first.source_stats.medians[0], 5.5);
    drop(first);

    // A sample and the statistics changed on disk and sealed again come back as they are: the
    // frame was mapped and its statistics read, not measured again.
    let kept = kept_directory(temp_dir.path());
    let plane = spill_of(&kept, &source_of::<LinearImage>(&source)).channel_path(0);
    let edit = |value: f32| {
        let kept_source = source_of::<LinearImage>(&source);
        let spill = spill_of(&kept, &kept_source);
        poke(&plane, 2, value);
        spill.reseal(1);
        let committed = spill.committed().unwrap();
        let mut sentinel = committed.stats;
        sentinel.medians[0] = 99.0;
        spill
            .commit(
                Decoded {
                    carries: Carries::Channels,
                    ..committed.decoded
                },
                committed.digests,
                &sentinel,
            )
            .unwrap();
    };
    edit(102.0);
    let reused = frame.load::<LinearImage>(temp_dir.path()).unwrap();
    assert_eq!(reused.channels[0].chunk(0..3), &[0.0, 1.0, 102.0]);
    assert_eq!(reused.source_stats.medians[0], 99.0);
    drop(reused);

    fs::write(&plane, [0u8; 48]).unwrap();
    let decoded = frame.load::<LinearImage>(temp_dir.path()).unwrap();
    assert_eq!(decoded.channels[0].chunk(0..12), pixels, "a zeroed plane");
    assert_eq!(decoded.source_stats.medians[0], 5.5);
    drop(decoded);

    let settle = || thread::sleep(Duration::from_millis(20));
    edit(102.0);
    settle();
    let mut permissions = fs::metadata(&source).unwrap().permissions();
    permissions.set_readonly(true);
    fs::set_permissions(&source, permissions.clone()).unwrap();
    let decoded = frame.load::<LinearImage>(temp_dir.path()).unwrap();
    assert_eq!(
        decoded.channels[0].chunk(0..12),
        pixels,
        "a read-only source"
    );
    drop(decoded);
    #[expect(
        clippy::permissions_set_readonly_false,
        reason = "the file is the test's own, made writable again for the rewrites below"
    )]
    permissions.set_readonly(false);
    fs::set_permissions(&source, permissions).unwrap();

    settle();
    let restored: Vec<f32> = (100..112).map(|i| i as f32).collect();
    LinearImage::from_pixels(dims, restored.clone())
        .save(&source)
        .unwrap();
    set_mtime(&source, 1_700_000_000_000_000_100);
    let decoded = frame.load::<LinearImage>(temp_dir.path()).unwrap();
    assert_eq!(
        decoded.channels[0].chunk(0..12),
        restored,
        "a rewrite with the mtime set back"
    );
    drop(decoded);

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
    let decoded = frame.load::<LinearImage>(temp_dir.path()).unwrap();
    assert_eq!(decoded.channels[0].chunk(0..12), rewritten);
}

/// A reused frame is held to the same checks as a decoded one: a non-finite sample names its pixel.
#[test]
fn cache_frame_validates_a_reused_frame() {
    let temp_dir = TempDir::new("lumos_cache_frame_validates");
    let dims = ImageDimensions::new((4, 3), 1);
    let source = temp_dir.join("source.tiff");
    LinearImage::from_pixels(dims, (0..12).map(|i| i as f32).collect())
        .save(&source)
        .unwrap();
    let frame = SetFrame {
        source: &source,
        dimensions: dims,
        index: 1,
    };
    drop(frame.load::<LinearImage>(temp_dir.path()).unwrap());
    let kept = kept_directory(temp_dir.path());
    let kept_source = source_of::<LinearImage>(&source);
    let spill = spill_of(&kept, &kept_source);

    // Sealed again, so the digests pass and the checks behind them are what refuses it.
    poke(&spill.channel_path(0), 2, f32::INFINITY);
    spill.reseal(1);
    let error = frame.load::<LinearImage>(temp_dir.path()).unwrap_err();
    assert!(matches!(
        error,
        StackError::Admission(AdmissionError::NonFiniteImageSample {
            index: 1,
            channel: 0,
            pixel: 2,
            value: f32::INFINITY,
        })
    ));
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
    let frame = SetFrame {
        source: &source,
        dimensions: dims,
        index: 0,
    };

    drop(frame.load::<LinearImage>(temp_dir.path()).unwrap());
    let kept = kept_directory(temp_dir.path());
    assert_ne!(
        spill_of(&kept, &source_of::<LinearImage>(&source)).channel_path(0),
        spill_of(&kept, &source_of::<CfaImage>(&source)).channel_path(0)
    );
    let error = frame.load::<CfaImage>(temp_dir.path()).unwrap_err();
    assert!(matches!(error, StackError::ImageLoad(_)), "{error:?}");
}

/// A kept cache serves a second run, resident or spilled, whether frame 0 had to be decoded for
/// the plan (a float TIFF, which has no header to peek) or not (a FITS file): frame 1's planes come
/// back from the first run's files, and so do frame 0's when no decode was spent on it, the stack's
/// metadata with them. A frame 0 decoded for the plan is taken from that decode and committed
/// again.
#[test]
fn a_kept_cache_is_reused_by_the_next_run() {
    let temp_dir = TempDir::new("lumos_cache_kept_run");
    let dims = ImageDimensions::new((4, 3), 1);
    let config = StackConfig {
        weighting: Weighting::Equal,
        normalization: Normalization::None,
        ..StackConfig::light()
    };
    for (format, memory_override, spills) in [
        ("tiff", Some(1), true),
        ("tiff", None, false),
        ("fits", Some(1), true),
        ("fits", None, false),
    ] {
        let label = format!("{format}, spilled {spills}");
        let directory = temp_dir.join(format!("{format}_{spills}"));
        fs::create_dir_all(&directory).unwrap();
        let paths: Vec<PathBuf> = (0..2)
            .map(|index| {
                let path = directory.join(format!("light_{index}.{format}"));
                let pixels = vec![0.25 + 0.25 * index as f32; 12];
                if format == "fits" {
                    write_fits(&path, &FitsImage::new([4, 3], pixels).unwrap(), None);
                } else {
                    LinearImage::from_pixels(dims, pixels).save(&path).unwrap();
                }
                path
            })
            .collect();
        let ingest = IngestConfig {
            cache_dir: directory.join("cache"),
            keep_cache: true,
            ..IngestConfig::default()
        };
        let path_refs: Vec<&Path> = paths.iter().map(PathBuf::as_path).collect();
        let run = || {
            load_tiered::<LinearImage>(
                &path_refs,
                &config,
                IngestRun::planned(RunMemory::new(1 << 30, memory_override), &ingest),
                &NoStep,
            )
            .unwrap()
        };

        let first = run();
        assert_eq!(first.core.tier.spills(), spills, "{label}");
        drop(first);
        let kept = kept_directory(&ingest.cache_dir);
        for path in &paths {
            let kept_source = source_of::<LinearImage>(path);
            let spill = spill_of(&kept, &kept_source);
            assert!(
                spill.committed().is_some(),
                "{label}: {} was not committed",
                path.display()
            );
            poke(&spill.channel_path(0), 0, 0.125);
            spill.reseal(1);
        }

        let second = run();
        let first_sample = if format == "fits" { 0.125 } else { 0.25 };
        assert_eq!(
            second.frames[0].channels[0].chunk(0..1),
            &[first_sample],
            "{label}"
        );
        assert_eq!(
            second.frames[1].channels[0].chunk(0..1),
            &[0.125],
            "{label}"
        );
        assert!(second.core.metadata.provenance.is_some(), "{label}");
    }
}

/// A frame of 4 × 3 in a set of 8 × 6 is refused by its index, with both shapes.
#[test]
fn cache_frame_dimension_mismatch() {
    let temp_dir = TempDir::new("lumos_load_cache_mismatch_test");
    let actual_dims = ImageDimensions::new((4, 3), 1);
    let pixels: Vec<f32> = (0..12).map(|i| i as f32).collect();
    let image = LinearImage::from_pixels(actual_dims, pixels);

    let source_path = temp_dir.join("source.tiff");
    image.save(&source_path).unwrap();
    let expected_dims = ImageDimensions::new((8, 6), 1);
    let result = SetFrame {
        source: &source_path,
        dimensions: expected_dims,
        index: 5,
    }
    .load::<LinearImage>(temp_dir.path());

    assert!(matches!(
        result.unwrap_err(),
        StackError::Admission(AdmissionError::DimensionMismatch(FrameDimensionMismatch {
            index: 5,
            expected,
            actual,
        })) if expected == expected_dims && actual == actual_dims
    ));
}
