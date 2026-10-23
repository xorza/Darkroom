use std::convert::Infallible;
use std::io::{Seek, SeekFrom, Write};
use std::path::PathBuf;
use std::thread;
use std::time::{Duration, UNIX_EPOCH};

use common::{TempDir, fs};
use fits_well::image::Image as FitsImage;

use crate::combine::frame_cache::loader::*;
use crate::combine::normalization::Normalization;
use crate::combine::stack_config::StackConfig;
use crate::combine::stack_config::weighting::Weighting;
use crate::frame_store::decode_cache::cached_source::CachedSource;
use crate::frame_store::decode_cache::frame_spill::FrameSpill;
use crate::frame_store::run_scratch::RunScratch;
use crate::frame_store::storage_tier::CombineBudget;
use crate::image::cfa::cfa_pattern::CfaPattern;
use crate::image::cfa::cfa_type::CfaType;
use crate::image::cfa::mosaic::Mosaic;
use crate::image::channel_layout::ChannelLayout;
use crate::image::image_dimensions::ImageDimensions;
use crate::image::linear_image::LinearImage;
use crate::ingest::error::AdmissionError;
use crate::ingest::ingest_config::IngestConfig;
use crate::internals::fits::write_fits;
use crate::io::decoder::cfa_decoder::CfaDecoder;
use crate::io::decoder::linear_decoder::LinearDecoder;
use crate::io::frame_info::{DecoderBytes, FrameInfo};
use crate::io::writer;
use crate::memory::budget::Budget;
use crate::stack_product::quality_planes::QualityPlanes;

/// Frame `idx` of a set of `dimensions`, decoded from `source`.
#[derive(Debug, Clone, Copy)]
struct SetFrame<'a> {
    source: &'a Path,
    dimensions: ImageDimensions,
    idx: usize,
}

impl SetFrame<'_> {
    /// The frame loaded as an `I` through the decode cache under `cache_dir` into a spilled tier.
    fn load<D: Decoder>(self, cache_dir: &Path) -> Result<StoredFrame, StackError> {
        let Self {
            source,
            dimensions,
            idx,
        } = self;
        // A fresh intake per call: these tests are about one frame's cache files, not the set it
        // belongs to, so no frame 0 is admitted before it.
        let run = IngestRun::planned(
            1 << 40,
            &IngestConfig {
                keep_cache: true,
                ..IngestConfig::with_cache_dir(cache_dir.to_path_buf())
            },
        );
        let intake = FrameIntake::new(
            IntakePlan {
                dimensions,
                frame_count: idx + 1,
                decode_slots: 1,
                stage: StackingStage::Loading,
            },
            &run,
        );
        let tier = StorageTier::Spilled {
            scratch: RunScratch::create(cache_dir).unwrap(),
            combine: CombineBudget::new(Budget::of_memory(1 << 30, 1)),
        };
        let paths = vec![source; idx + 1];
        let files = FileSet::<_, D>::new(&paths, &run).unwrap();
        FrameStore {
            files: &files,
            kept: files.storage().kept(),
            step: &None::<Infallible>,
            ctx: &run.ctx,
            tier: &tier,
            intake: &intake,
        }
        .frame(idx)
        .map(|loaded| loaded.frame)
    }
}

/// The directory of the decode cache under `cache_dir`.
fn kept_dir(cache_dir: &Path) -> PathBuf {
    DecodeCache::open(cache_dir).unwrap().path().to_path_buf()
}

/// `path` decoded by `I` under the default context, as the decode cache names and keys it.
fn source_of<D: Decoder>(path: &Path) -> CachedSource {
    CachedSource::of(path, D::KIND, &LoadCtx::default()).unwrap()
}

/// The files `source` is kept in under `kept`.
fn spill_of<'a>(kept: &'a Path, source: &'a CachedSource) -> FrameSpill<'a> {
    FrameSpill::cached(kept, &source.canonical, source.key)
}

/// Overwrite sample `idx` of a spilled plane in place, as anything outside lumos could.
fn poke(path: &Path, idx: usize, value: f32) {
    let mut file = fs::open_write(path).unwrap();
    file.seek(SeekFrom::Start((idx * size_of::<f32>()) as u64))
        .unwrap();
    file.write_all(&value.to_le_bytes()).unwrap();
}

fn set_mtime(path: &Path, nanos_past_epoch: u64) {
    fs::open_write(path)
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
    let dims = ImageDimensions::new((4, 3), ChannelLayout::Mono);
    // [0, 1, …, 11]: median 5.5, absolute deviations 0.5, 0.5, 1.5, 1.5, …, 5.5, 5.5 → MAD 3.0.
    let pixels: Vec<f32> = (0..12).map(|i| i as f32).collect();
    let source = temp_dir.join("source.tiff");
    writer::raster(
        &LinearImage::from_interleaved(dims, pixels.clone()),
        &source,
    )
    .unwrap();
    set_mtime(&source, 1_700_000_000_000_000_100);
    let frame = SetFrame {
        source: &source,
        dimensions: dims,
        idx: 0,
    };

    let first = frame.load::<LinearDecoder>(temp_dir.path()).unwrap();
    assert_eq!(first.channels[0].chunk(0..12), pixels);
    assert_eq!(first.source_stats.medians[0], 5.5);
    drop(first);

    // A sample and the statistics changed on disk and sealed again come back as they are: the
    // frame was mapped and its statistics read, not measured again.
    let kept = kept_dir(temp_dir.path());
    let plane = spill_of(&kept, &source_of::<LinearDecoder>(&source)).channel_path(0);
    let edit = |value: f32| {
        let kept_source = source_of::<LinearDecoder>(&source);
        let spill = spill_of(&kept, &kept_source);
        poke(&plane, 2, value);
        spill.reseal();
        let committed = spill.committed().unwrap();
        assert_eq!(committed.decoded.flags, None);
        let mut sentinel = committed.admitted;
        sentinel.stats.medians[0] = 99.0;
        spill
            .commit(&committed.decoded, committed.digests, &sentinel)
            .unwrap();
    };
    edit(102.0);
    let reused = frame.load::<LinearDecoder>(temp_dir.path()).unwrap();
    assert_eq!(reused.channels[0].chunk(0..3), &[0.0, 1.0, 102.0]);
    assert_eq!(reused.source_stats.medians[0], 99.0);
    drop(reused);

    fs::write(&plane, [0u8; 48]).unwrap();
    let decoded = frame.load::<LinearDecoder>(temp_dir.path()).unwrap();
    assert_eq!(decoded.channels[0].chunk(0..12), pixels, "a zeroed plane");
    assert_eq!(decoded.source_stats.medians[0], 5.5);
    drop(decoded);

    let settle = || thread::sleep(Duration::from_millis(20));
    edit(102.0);
    settle();
    let mut permissions = fs::metadata(&source).unwrap().permissions();
    permissions.set_readonly(true);
    fs::set_permissions(&source, permissions.clone()).unwrap();
    let decoded = frame.load::<LinearDecoder>(temp_dir.path()).unwrap();
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
    writer::raster(
        &LinearImage::from_interleaved(dims, restored.clone()),
        &source,
    )
    .unwrap();
    set_mtime(&source, 1_700_000_000_000_000_100);
    let decoded = frame.load::<LinearDecoder>(temp_dir.path()).unwrap();
    assert_eq!(
        decoded.channels[0].chunk(0..12),
        restored,
        "a rewrite with the mtime set back"
    );
    drop(decoded);

    let rewritten: Vec<f32> = (200..212).map(|i| i as f32).collect();
    let original_len = fs::metadata(&source).unwrap().len();
    writer::raster(
        &LinearImage::from_interleaved(dims, rewritten.clone()),
        &source,
    )
    .unwrap();
    assert_eq!(
        fs::metadata(&source).unwrap().len(),
        original_len,
        "the timestamp, not the length, tells this rewrite apart"
    );
    set_mtime(&source, 1_700_000_000_000_000_200);
    let decoded = frame.load::<LinearDecoder>(temp_dir.path()).unwrap();
    assert_eq!(decoded.channels[0].chunk(0..12), rewritten);
}

/// A reused frame is held to the same checks as a decoded one: a non-finite sample names its pixel.
#[test]
fn cache_frame_validates_a_reused_frame() {
    let temp_dir = TempDir::new("lumos_cache_frame_validates");
    let dims = ImageDimensions::new((4, 3), ChannelLayout::Mono);
    let source = temp_dir.join("source.tiff");
    writer::raster(
        &LinearImage::from_interleaved(dims, (0..12).map(|i| i as f32).collect()),
        &source,
    )
    .unwrap();
    let frame = SetFrame {
        source: &source,
        dimensions: dims,
        idx: 1,
    };
    drop(frame.load::<LinearDecoder>(temp_dir.path()).unwrap());
    let kept = kept_dir(temp_dir.path());
    let kept_source = source_of::<LinearDecoder>(&source);
    let spill = spill_of(&kept, &kept_source);

    // Sealed again, so the digests pass and the checks behind them are what refuses it.
    poke(&spill.channel_path(0), 2, f32::INFINITY);
    spill.reseal();
    let error = frame.load::<LinearDecoder>(temp_dir.path()).unwrap_err();
    assert!(matches!(
        error,
        StackError::Admission(AdmissionError::NonFiniteImageSample {
            idx: 1,
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
    let dims = ImageDimensions::new((4, 3), ChannelLayout::Mono);
    let source = temp_dir.join("light.tiff");
    writer::raster(
        &LinearImage::from_interleaved(dims, (0..12).map(|i| i as f32 / 16.0).collect()),
        &source,
    )
    .unwrap();
    let frame = SetFrame {
        source: &source,
        dimensions: dims,
        idx: 0,
    };

    drop(frame.load::<LinearDecoder>(temp_dir.path()).unwrap());
    let kept = kept_dir(temp_dir.path());
    assert_ne!(
        spill_of(&kept, &source_of::<LinearDecoder>(&source)).channel_path(0),
        spill_of(&kept, &source_of::<CfaDecoder>(&source)).channel_path(0)
    );
    let error = frame.load::<CfaDecoder>(temp_dir.path()).unwrap_err();
    assert!(matches!(error, StackError::ImageLoad(_)), "{error:?}");
}

/// Adds one to every sample: a step that changes every frame it takes.
#[derive(Debug)]
struct AddOne;

impl FrameStep<LinearImage> for AddOne {
    type Error = Infallible;

    fn apply(&self, _idx: usize, image: &mut LinearImage) -> Result<(), Infallible> {
        for plane in image.planes_mut() {
            for sample in plane.pixels_mut() {
                *sample += 1.0;
            }
        }
        Ok(())
    }
}

/// A kept cache serves a second run, resident or spilled, whether the plan needs frame 0 itself (a
/// float TIFF, which has no header to peek) or not (a FITS file): every frame's planes come back
/// from the first run's files, frame 0's for the plan too, so no frame is decoded or written
/// again, and the stack's metadata comes with them.
///
/// A run through a step reads and writes the cache too, but its decodes alone. The first run here
/// adds one to each frame, 0.25 + 1 and 0.5 + 1 at pixel 1, and still commits the frames as they
/// decode, 0.25 and 0.5 there. Pixel 0 of each kept frame is then edited to 0.125: the run with no
/// step reads 0.125, and a run through the step reads it too, plus one, 1.125.
#[test]
fn a_kept_cache_is_reused_by_the_next_run() {
    let temp_dir = TempDir::new("lumos_cache_kept_run");
    let dims = ImageDimensions::new((4, 3), ChannelLayout::Mono);
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
        let dir = temp_dir.join(format!("{format}_{spills}"));
        fs::create_dir_all(&dir).unwrap();
        let paths: Vec<PathBuf> = (0..2)
            .map(|idx| {
                let path = dir.join(format!("light_{idx}.{format}"));
                let pixels = vec![0.25 + 0.25 * idx as f32; 12];
                if format == "fits" {
                    write_fits(&path, FitsImage::new([4, 3], pixels).unwrap().view(), None);
                } else {
                    writer::raster(&LinearImage::from_interleaved(dims, pixels), &path).unwrap();
                }
                path
            })
            .collect();
        let ingest = IngestConfig {
            cache_dir: dir.join("cache"),
            keep_cache: true,
            ..IngestConfig::default()
        };
        let path_refs: Vec<&Path> = paths.iter().map(PathBuf::as_path).collect();
        let run = |step: &Option<AddOne>| {
            load_tiered::<LinearDecoder, _, StackError>(
                SteppedPaths {
                    paths: &path_refs,
                    step,
                },
                PlannedCombine {
                    normalization: config.normalization,
                    quality: config.quality,
                },
                IngestRun::planned(memory_override.unwrap_or(1 << 30), &ingest),
            )
            .unwrap()
        };

        let first = run(&Some(AddOne));
        assert_eq!(first.core.tier.spills(), spills, "{label}");
        assert_eq!(first.frames[0].channels[0].chunk(1..2), &[1.25], "{label}");
        assert_eq!(first.frames[1].channels[0].chunk(1..2), &[1.5], "{label}");
        drop(first);
        let kept = kept_dir(&ingest.cache_dir);
        for path in &paths {
            let kept_source = source_of::<LinearDecoder>(path);
            let spill = spill_of(&kept, &kept_source);
            assert!(
                spill.committed().is_some(),
                "{label}: {} was not committed",
                path.display()
            );
            poke(&spill.channel_path(0), 0, 0.125);
            spill.reseal();
        }

        let second = run(&None);
        assert_eq!(
            second.frames[0].channels[0].chunk(0..2),
            &[0.125, 0.25],
            "{label}"
        );
        assert_eq!(
            second.frames[1].channels[0].chunk(0..2),
            &[0.125, 0.5],
            "{label}"
        );
        assert!(
            second.core.metadata.derived.provenance().is_some(),
            "{label}"
        );
        drop(second);

        let stepped = run(&Some(AddOne));
        assert_eq!(
            stepped.frames[0].channels[0].chunk(0..2),
            &[1.125, 1.25],
            "{label}"
        );
        assert_eq!(
            stepped.frames[1].channels[0].chunk(0..2),
            &[1.125, 1.5],
            "{label}"
        );
    }
}

/// A frame of 4 × 3 in a set of 8 × 6 is refused by its index, with both shapes.
#[test]
fn cache_frame_dimension_mismatch() {
    let temp_dir = TempDir::new("lumos_load_cache_mismatch_test");
    let actual_dims = ImageDimensions::new((4, 3), ChannelLayout::Mono);
    let pixels: Vec<f32> = (0..12).map(|i| i as f32).collect();
    let image = LinearImage::from_interleaved(actual_dims, pixels);

    let source_path = temp_dir.join("source.tiff");
    writer::raster(&image, &source_path).unwrap();
    let expected_dims = ImageDimensions::new((8, 6), ChannelLayout::Mono);
    let result = SetFrame {
        source: &source_path,
        dimensions: expected_dims,
        idx: 5,
    }
    .load::<LinearDecoder>(temp_dir.path());

    assert!(matches!(
        result.unwrap_err(),
        StackError::Admission(AdmissionError::DimensionMismatch(FrameDimensionMismatch {
            idx: 5,
            expected,
            actual,
        })) if expected == expected_dims && actual == actual_dims
    ));
}

/// Three 100 × 100 RGGB mosaics stacked to the image alone, with no normalization. A frame resides
/// as 40 000 B of samples and 10 000 B of flags, and its statistics hold its pairs, 98 a row over
/// 100 rows at 4 bytes and a bin byte, 49 000 B: the decode keeps 50 000 B a frame and holds 49 000
/// more in work, beside the 30 000 B file the peek read for its frame's decode; the normalization
/// holds nothing; the combine holds the image and its flag byte, 5 B a pixel, 50 000 B, and a row
/// copies three frames of 5 B a pixel over 100 pixels into each of its two chunk copies, 3000 B.
/// The frames share a flat gain of the three colors, 26 × 26 nodes of 4 B each, 8112 B, which every
/// stage holds beside it. A combine scratch for three frames on its one worker is 266 B: a value, a
/// weight, three noise columns and a flag byte a frame, 21 B, the sorted keys, values, positions
/// and kept marks, 17 B, the weights and the clamped copy, 8 B, four GESD critical values of 8 B,
/// and the normal scores, a row of three for each of the three counts and their counts, 72 + 24 B:
/// 3 × 46 + 32 + 96.
#[test]
fn stages_charge_the_decode_the_normalization_and_the_combine() {
    let peek = SetPeek {
        idx: 0,
        info: FrameInfo {
            dimensions: ImageDimensions::new((100, 100), ChannelLayout::Mono),
            cfa_type: CfaType::Mosaic(Mosaic::Bayer(CfaPattern::Rggb)),
            decoder: DecoderBytes::default(),
            gain_bytes: 8112,
        },
        held: 30_000,
        gains: 8112,
    };
    let combine = PlannedCombine {
        normalization: Normalization::None,
        quality: QualityPlanes::IMAGE_ONLY,
    };
    let kept = 50_000;
    assert_eq!(
        stages(combine, peek, 3, 1),
        [
            Stage {
                kept,
                held: 38_112,
                in_flight: 49_000,
                units: Units::Frames,
            },
            Stage {
                kept,
                held: 8112,
                in_flight: 0,
                units: Units::Whole,
            },
            Stage {
                kept,
                held: 58_112 + 266,
                in_flight: 3000,
                units: Units::Rows(100),
            },
        ]
    );
}
