//! `lumos::stack` over a set of calibrated and registered mono lights in 32-bit float FITS, end to
//! end under the light preset: the reads, each light's statistics, the normalization and the
//! combine.

use common::CancelToken;
use criterion::{BenchmarkId, Criterion, Throughput};

use crate::bench::files::{Files, LIGHTS};
use crate::bench::fixture::{self, Scale};
use crate::bench::scene::{FRAME, Field};
use crate::combine::config::StackConfig;
use crate::combine::stack::stack;
use crate::ingest::ingest_config::IngestConfig;
use crate::progress::progress_callback::ProgressCallback;

pub(crate) fn pipeline(c: &mut Criterion) {
    let field = Field::Typical;
    let mut group = fixture::group(c, "pipeline/stack", Scale::Pipeline);
    group.throughput(Throughput::Elements((LIGHTS * FRAME.pixel_count()) as u64));
    let set = format!("{LIGHTS}x{}", field.name());
    group.bench_function(BenchmarkId::new("fits_mono", set), |b| {
        let files = Files::aligned_lights(field);
        let ingest = IngestConfig::with_cache_dir(files.dir().join("cache"));
        b.iter(|| {
            stack(
                &files.paths,
                &StackConfig::light(),
                &ingest,
                ProgressCallback::default(),
                CancelToken::never(),
            )
            .unwrap()
        });
    });
}
