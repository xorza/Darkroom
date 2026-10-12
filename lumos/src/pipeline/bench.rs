//! The pipeline's stages between a light's decode and its stack: its preparation, and its
//! parking on the spill tier.

use std::cell::OnceCell;

use common::{CancelToken, TempDir};
use criterion::{BenchmarkId, Criterion, Throughput};

use crate::bench::fixture::{self, Scale};
use crate::bench::scene::{self, FRAME, Field};
use crate::calibration_masters::cosmic_ray::config::CosmicRayConfig;
use crate::calibration_masters::cosmic_ray::reject_cosmic_rays;
use crate::calibration_masters::{CalibrationMasters, DEFAULT_SIGMA_THRESHOLD};
use crate::frame_store::run_scratch::RunScratch;
use crate::frame_store::stored_image::StoredImage;
use crate::io::image::cfa::CfaType;
use crate::io::image::cfa::cfa_pattern::CfaPattern;
use crate::io::image::load_context::LoadContext;
use crate::pipeline::config::AlignStackConfig;
use crate::registration::bench::Pair;
use crate::registration::registration_config::WarpParams;
use crate::registration::resample::WarpBuffers;
use crate::registration::resample::source_image::{SourceImage, SourcePlane};

pub(crate) fn benches(c: &mut Criterion) {
    prepare(c);
    park(c);
}

/// A decoded light prepared as a run of RAW or mosaic lights prepares each: calibrated by its
/// masters, cleaned of cosmic rays where the configuration asks it, and demosaiced. The default
/// configuration leaves cosmic rays in.
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
            let light = field.light(FRAME, cfa_type);
            let passes = LoadContext::default().xtrans_passes;
            b.iter_batched(
                || light.clone(),
                |mut light| {
                    masters.calibrate(&mut light).unwrap();
                    if let Some(config) = &cosmic_ray {
                        reject_cosmic_rays(&mut light, config).unwrap();
                    }
                    light.demosaic(passes, &CancelToken::never()).unwrap()
                },
                fixture::in_place(size_of_val(light.data.pixels())),
            );
        });
    }
}

/// A calibrated light parked on the spill tier between its detection and its registration, then
/// warped straight from its maps into the warp's buffers, as a run that spills parks every light
/// while it chooses the reference: every page of the light in the page cache.
fn park(c: &mut Criterion) {
    let field = Field::Typical;
    let mut group = fixture::group(c, "stage/park", Scale::Frame);
    group.throughput(Throughput::Elements(FRAME.pixel_count() as u64));
    group.bench_function(BenchmarkId::new("warp_from_map/warm", field.name()), |b| {
        let dir = TempDir::new("lumos-bench-park");
        let scratch = RunScratch::create(dir.path()).unwrap();
        let light = field.render(FRAME).image;
        let parked = StoredImage::spill(&scratch, &light).unwrap();
        let flags = parked.flags();
        let source = SourceImage {
            dimensions: parked.dimensions,
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
        let mut buffers = WarpBuffers::new(parked.dimensions);
        b.iter(|| buffers.warp_into(&source, &warp, WarpParams::default()));
    });
}
