//! `StackConfig::stack_paths` over a set of calibrated and registered mono lights in 32-bit float
//! FITS, end to end under the light preset: the reads, each light's statistics, the normalization
//! and the combine.

use std::time::Duration;

use criterion::{BenchmarkId, Criterion, Throughput};

use crate::bench::files::{Files, LIGHTS};
use crate::bench::fixture::{Kind, Scale};
use crate::bench::scene::{FRAME, Field};
use crate::combine::stack_config::StackConfig;
use crate::ingest::ingest_config::IngestConfig;
use crate::progress::progress_monitor::ProgressMonitor;

pub(crate) fn integration(c: &mut Criterion) {
    let field = Field::Typical;
    // The bench's median iteration on a Ryzen 7 6800U, x86-64-v3.
    let iteration = Duration::from_millis(2110);
    let mut group = Kind::Integration.group(c, "pipeline/stack", Scale::Set { iteration });
    group.throughput(Throughput::Elements((LIGHTS * FRAME.pixel_count()) as u64));
    let set = format!("{LIGHTS}x{}", field.name());
    group.bench_function(BenchmarkId::new("fits_mono", set), |b| {
        let files = Files::aligned_lights(field);
        let ingest = IngestConfig::with_cache_dir(files.dir().join("cache"));
        b.iter(|| {
            StackConfig::light()
                .stack_paths(&files.paths, &ingest, ProgressMonitor::unattended())
                .unwrap()
        });
    });
}
