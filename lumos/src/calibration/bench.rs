//! The calibration of one light by a session's masters: the bias, the dark and the flat removed
//! and the defects repaired, as every light of the session is; and the masters built from a
//! session's frames, end to end.

use std::time::Duration;

use common::{CancelToken, TempDir};
use criterion::{BenchmarkId, Criterion, Throughput};

use crate::bench::files::{LIGHTS, MasterFiles};
use crate::bench::fixture::{self, Kind, Scale};
use crate::bench::scene::{self, FRAME, Field};
use crate::calibration::calibration_masters::{CalibrationMasters, MasterBuild};
use crate::calibration::calibration_set::CalibrationSet;
use crate::calibration::defect_map::sigma_threshold::SigmaThreshold;
use crate::calibration::master_cache::NoMasterCache;
use crate::calibration::{cosmic_ray, defect_map, prepared_flat};
use crate::image::cfa::cfa_pattern::CfaPattern;
use crate::image::cfa::cfa_type::CfaType;
use crate::ingest::ingest_config::IngestConfig;
use crate::progress::progress_monitor::ProgressMonitor;

pub(crate) fn atomic(c: &mut Criterion) {
    apply(c);
    defect_map::bench::atomic(c);
    cosmic_ray::bench::atomic(c);
    prepared_flat::bench::atomic(c);
}

fn apply(c: &mut Criterion) {
    let field = Field::Typical;
    let cfa_type = CfaType::Bayer(CfaPattern::Rggb);
    let mut group = Kind::Atomic.group(c, "calibration/apply/frame", Scale::Frame);
    group.throughput(Throughput::Elements(FRAME.pixel_count() as u64));
    group.bench_function(BenchmarkId::new("light/rggb", field.name()), |b| {
        let masters = CalibrationMasters::from_images(
            scene::masters(FRAME, cfa_type),
            SigmaThreshold::DEFAULT,
            &CancelToken::never(),
        )
        .unwrap();
        let light = field.light(FRAME, cfa_type);
        b.iter_batched_ref(
            || light.clone(),
            |light| masters.calibrate(light).unwrap(),
            fixture::fresh_copies(size_of_val(light.data().pixels())),
        );
    });
}

/// `CalibrationMasters::build` over a session's bias, dark and flat frames, 16-bit mosaics as the
/// camera writes them, with no master cache: each role stacked, the bias out of the dark and the
/// flat, the defects detected and the flat prepared.
pub(crate) fn integration(c: &mut Criterion) {
    let cfa_type = CfaType::Bayer(CfaPattern::Rggb);
    // The bench's median iteration on a Ryzen 7 6800U, x86-64-v3.
    let iteration = Duration::from_millis(6620);
    let mut group = Kind::Integration.group(c, "pipeline/masters", Scale::Set { iteration });
    group.throughput(Throughput::Elements(
        (3 * LIGHTS * FRAME.pixel_count()) as u64,
    ));
    group.bench_function(BenchmarkId::new("build/rggb", format!("{LIGHTS}x3")), |b| {
        let files = MasterFiles::of(cfa_type);
        let dir = TempDir::new("lumos-bench-masters-cache");
        let ingest = IngestConfig::with_cache_dir(dir.path().to_path_buf());
        let frames = CalibrationSet {
            dark: files.frames.dark.as_slice(),
            flat: files.frames.flat.as_slice(),
            bias: files.frames.bias.as_slice(),
            flat_dark: &[][..],
        };
        b.iter(|| {
            CalibrationMasters::build(
                MasterBuild {
                    frames,
                    sigma_threshold: SigmaThreshold::DEFAULT,
                    cache: &NoMasterCache,
                },
                &ingest,
                ProgressMonitor::unattended(),
            )
            .unwrap()
        });
    });
}
