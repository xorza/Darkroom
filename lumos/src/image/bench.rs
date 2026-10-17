//! The image benches: the entry `crate::bench` runs them by.

use criterion::Criterion;

use crate::image::cfa::demosaic;

pub(crate) fn atomic(c: &mut Criterion) {
    demosaic::bench::atomic(c);
}
