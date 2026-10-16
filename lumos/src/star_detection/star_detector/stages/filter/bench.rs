//! The removal of duplicate detections from a star list sorted by flux, the detector's last filter,
//! on a typical field's 3000 stars and a cluster's 20 000, at the separation each field's
//! configuration sets.

use criterion::{BenchmarkId, Criterion, Throughput};

use crate::bench::fixture::{self, Kind, Scale};
use crate::bench::scene::{FRAME, Field};
use crate::star_detection::bench;
use crate::star_detection::duplicates::Duplicates;
use crate::star_detection::star::Star;

pub(crate) fn atomic(c: &mut Criterion) {
    let mut group = Kind::Atomic.group(c, "detection/filter", Scale::Sample);
    for field in [Field::Typical, Field::Dense] {
        group.throughput(Throughput::Elements(field.stars() as u64));
        group.bench_function(BenchmarkId::new("remove_duplicates", field.name()), |b| {
            let mut stars: Vec<Star> = field
                .scene(FRAME)
                .sources
                .iter()
                .map(|source| Star::at(source.pos).with_flux(source.flux))
                .collect();
            stars.sort_by(|a, b| b.flux.total_cmp(&a.flux));
            let min_separation = bench::config(field).filter.duplicate_min_separation;
            let mut scratch = Duplicates::default();
            b.iter_batched_ref(
                || stars.clone(),
                |stars| scratch.remove(stars, min_separation),
                fixture::fresh_copies(size_of_val(stars.as_slice())),
            );
        });
    }
}
