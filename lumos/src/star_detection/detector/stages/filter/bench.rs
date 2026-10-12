//! The removal of duplicate detections from a star list sorted by flux, the detector's last filter,
//! on a typical field's 3000 stars and a cluster's 20 000, at the separation each field's
//! configuration sets.

use criterion::{BenchmarkId, Criterion, Throughput};

use crate::bench::fixture::{self, Scale};
use crate::bench::scene::{FRAME, Field};
use crate::star_detection::bench::Stages;
use crate::star_detection::detector::stages::filter::DuplicateScratch;
use crate::star_detection::star::Star;

pub(crate) fn benches(c: &mut Criterion) {
    let mut group = fixture::group(c, "kernel/detection/filter", Scale::Sample);
    for field in [Field::Typical, Field::Dense] {
        let sources = field.scene(FRAME).sources;
        group.throughput(Throughput::Elements(sources.len() as u64));
        group.bench_function(BenchmarkId::new("remove_duplicates", field.name()), |b| {
            let mut stars: Vec<Star> = sources
                .iter()
                .map(|source| Star::at(source.pos).with_flux(source.flux))
                .collect();
            stars.sort_by(|a, b| b.flux.total_cmp(&a.flux));
            let min_separation = Stages::config(field).filter.duplicate_min_separation;
            let mut scratch = DuplicateScratch::default();
            b.iter_batched_ref(
                || stars.clone(),
                |stars| scratch.remove_duplicates(stars, min_separation),
                fixture::in_place(size_of_val(stars.as_slice())),
            );
        });
    }
}
