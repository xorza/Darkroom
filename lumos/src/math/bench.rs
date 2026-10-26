//! The math benches: the entry `crate::bench` runs them by.

use criterion::Criterion;

use crate::math::{
    dmat3, lanczos, linear_system, lm_controller, lstsq, pixel_quadrature, statistics, sum, wavelet,
};

pub(crate) fn atomic(c: &mut Criterion) {
    sum::bench::atomic(c);
    statistics::bench::atomic(c);
    wavelet::bench::atomic(c);
    lanczos::bench::atomic(c);
    dmat3::bench::atomic(c);
    pixel_quadrature::bench::atomic(c);
    linear_system::bench::atomic(c);
    lm_controller::bench::atomic(c);
    lstsq::bench::atomic(c);
}
