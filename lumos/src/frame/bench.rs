//! The frame benches: the entry `crate::bench` runs them by.

use criterion::Criterion;

use crate::frame::frame_stats;

pub(crate) fn atomic(c: &mut Criterion) {
    frame_stats::bench::atomic(c);
}
