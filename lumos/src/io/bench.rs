//! The decode benches: the entry `crate::bench` runs them by.

use criterion::Criterion;

use crate::io::fits::decode;
use crate::io::raw;

pub(crate) fn atomic(c: &mut Criterion) {
    raw::bench::atomic(c);
    decode::bench::atomic(c);
}
