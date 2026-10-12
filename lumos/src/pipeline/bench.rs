//! The pipeline's stages between a light's decode and its stack, its preparation and its parking
//! on the spill tier; and the pipeline itself, from a session's light files to the stack.

use std::cell::OnceCell;
use std::path::Path;
use std::time::Duration;

use common::{CancelToken, TempDir};
use criterion::{BatchSize, BenchmarkId, Criterion, Throughput};

use crate::bench::files::{Files, LIGHTS};
use crate::bench::fixture::{self, Scale};
use crate::bench::scene::{self, FRAME, Field};
use crate::calibration::calibration_masters::{CalibrationMasters, DEFAULT_SIGMA_THRESHOLD};
use crate::calibration::cosmic_ray::cosmic_ray_config::CosmicRayConfig;
use crate::frame_store::run_scratch::RunScratch;
use crate::frame_store::stored_image::StoredImage;
use crate::image::cfa::cfa_pattern::CfaPattern;
use crate::image::cfa::cfa_type::CfaType;
use crate::ingest::ingest_config::IngestConfig;
use crate::io::load_context::LoadContext;
use crate::pipeline::align_stack_config::AlignStackConfig;
use crate::pipeline::calibrate::CalibrationNotes;
use crate::pipeline::light_source::MosaicPreparation;
use crate::platform::MappedFile;
use crate::progress::progress_callback::ProgressCallback;
use crate::registration::bench::Pair;
use crate::registration::resample::WarpBuffers;
use crate::registration::resample::source_image::{SourceImage, SourcePlane};
use crate::registration::resample::warp_params::WarpParams;
use crate::scratch_pool::{Scratch, ScratchPool};

pub(crate) fn benches(c: &mut Criterion) {
    prepare(c);
    park(c);
}

/// A decoded light prepared as a run of RAW or mosaic lights prepares each, through
/// `MosaicPreparation::prepare`: calibrated by its masters, cleaned of cosmic rays where the
/// configuration asks it, and demosaiced. The default configuration leaves cosmic rays in.
fn prepare(c: &mut Criterion) {
    let field = Field::Typical;
    let cfa_type = CfaType::Bayer(CfaPattern::Rggb);
    let masters = OnceCell::new();
    let mut group = fixture::group(c, "stage/prepare", Scale::Frame);
    group.throughput(Throughput::Elements(FRAME.pixel_count() as u64));
    for (variant, cosmic_ray) in [
        ("light/rggb", AlignStackConfig::default().cosmic_ray),
        ("light_cosmic_rays/rggb", Some(CosmicRayConfig::default())),
    ] {
        group.bench_function(BenchmarkId::new(variant, field.name()), |b| {
            let masters: &CalibrationMasters = masters.get_or_init(|| {
                CalibrationMasters::from_images(
                    scene::masters(FRAME, cfa_type),
                    DEFAULT_SIGMA_THRESHOLD,
                    &CancelToken::never(),
                )
                .unwrap()
            });
            let notes = CalibrationNotes::default();
            let preparation = MosaicPreparation {
                masters,
                cosmic_ray: cosmic_ray.as_ref(),
                notes: &notes,
            };
            let light = field.light(FRAME, cfa_type);
            let context = LoadContext::default();
            b.iter_batched(
                || light.clone(),
                |light| {
                    preparation
                        .prepare(light, Path::new("light.fits"), &context)
                        .unwrap()
                },
                fixture::fresh_copies(size_of_val(light.data.pixels())),
            );
        });
    }
}

/// A calibrated light parked on the spill tier between its detection and its registration, then
/// warped straight from its maps into the warp's buffers, as a run that spills parks every light
/// while it chooses the reference: every page of the light in the page cache, and, where the system
/// can drop a file's pages, every page on the disk, as when the parked lights outgrow the memory.
fn park(c: &mut Criterion) {
    let field = Field::Typical;
    let mut group = fixture::group(c, "stage/park", Scale::Frame);
    group.throughput(Throughput::Elements(FRAME.pixel_count() as u64));
    for (variant, cold) in [("warp_from_map/warm", false), ("warp_from_map/cold", true)] {
        if cold && !MappedFile::EVICTS {
            continue;
        }
        group.bench_function(BenchmarkId::new(variant, field.name()), |b| {
            let dir = TempDir::new("lumos-bench-park");
            let scratch = RunScratch::create(dir.path()).unwrap();
            let light = field.render(FRAME).image;
            let parked = StoredImage::spill(&scratch, &light).unwrap();
            let flags = parked.flags();
            let source = SourceImage {
                dimensions: parked.dimensions,
                metadata: &light.metadata,
                planes: parked
                    .planes()
                    .map(|pixels| SourcePlane {
                        pixels,
                        width: FRAME.width,
                    })
                    .collect(),
                flags: flags.as_ref(),
            };
            let warp = Pair::typical().similarity_warp();
            let mut buffers = WarpBuffers::build(parked.dimensions);
            let rows = ScratchPool::default();
            let mut warp_parked =
                || buffers.warp_into(&source, &warp, WarpParams::default(), &rows);
            if cold {
                b.iter_batched(
                    || assert!(parked.evict(), "the light's pages leave the page cache"),
                    |()| warp_parked(),
                    BatchSize::PerIteration,
                );
            } else {
                b.iter(warp_parked);
            }
        });
    }
}

/// The memory override the spilled run plans against: under the 3.5 GB that twelve demosaiced
/// 24 MP lights alone hold, so the run spills them.
const SPILLED_MACHINE_BYTES: u64 = 4 << 30;

/// `AlignStackConfig::calibrate_align_stack_paths` over a session's raw lights, 16-bit mosaics as
/// the camera writes them, under the default configuration: each light decoded, calibrated by the
/// session's masters, demosaiced and detected, then registered to the reference, warped and
/// combined. Resident, and spilled on the memory of a 4 GiB machine.
pub(crate) fn pipeline(c: &mut Criterion) {
    let field = Field::Typical;
    let cfa_type = CfaType::Bayer(CfaPattern::Rggb);
    let masters = OnceCell::new();
    // The slowest bench's iteration, `rggb/spilled`'s median on a Ryzen 7 6800U, x86-64-v3.
    let iteration = Duration::from_millis(18_340);
    let mut group = fixture::group(
        c,
        "pipeline/calibrate_align_stack",
        Scale::Set { iteration },
    );
    group.throughput(Throughput::Elements((LIGHTS * FRAME.pixel_count()) as u64));
    let set = format!("{LIGHTS}x{}", field.name());
    for (tier, memory_override) in [("resident", None), ("spilled", Some(SPILLED_MACHINE_BYTES))] {
        group.bench_function(BenchmarkId::new(format!("rggb/{tier}"), &set), |b| {
            let masters: &CalibrationMasters = masters.get_or_init(|| {
                CalibrationMasters::from_images(
                    scene::masters(FRAME, cfa_type),
                    DEFAULT_SIGMA_THRESHOLD,
                    &CancelToken::never(),
                )
                .unwrap()
            });
            let files = Files::raw_lights(field, cfa_type);
            let ingest = IngestConfig {
                memory_override,
                ..IngestConfig::with_cache_dir(files.dir().join("cache"))
            };
            b.iter(|| {
                AlignStackConfig::default()
                    .calibrate_align_stack_paths(
                        &files.paths,
                        masters,
                        &ingest,
                        ProgressCallback::default(),
                        CancelToken::never(),
                    )
                    .unwrap()
            });
        });
    }
}
