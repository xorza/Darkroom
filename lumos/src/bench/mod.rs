//! The criterion benches, one entry for each area, which `benches/lumos.rs` runs.
//!
//! Every bench has one shape. A group is one module at one `fixture::Scale`, named
//! `<area>/<module>`, with `/frame` added for the frame-sized one. A bench is one operation,
//! its id `<operation>/<variant>` and its parameter the input size (a length, a side, a count or
//! a matrix shape), and its throughput the elements one iteration processes. A bench builds its input inside its `bench_function`
//! closure, so a filter that skips it skips its setup too, and an operation that changes its
//! input in place takes a fresh copy through `iter_batched_ref`, outside the timing.

pub(crate) mod fixture;

use criterion::Criterion;

use crate::math::{
    dmat3, floor_i32, lanczos, linear_system, lm_controller, lstsq, noise, pixel_quadrature,
    statistics, sum, wavelet,
};

/// Every hot path of `math`.
pub fn math(c: &mut Criterion) {
    sum::bench::benches(c);
    statistics::bench::benches(c);
    noise::bench::benches(c);
    wavelet::bench::benches(c);
    lanczos::bench::benches(c);
    dmat3::bench::benches(c);
    floor_i32::bench::benches(c);
    pixel_quadrature::bench::benches(c);
    linear_system::bench::benches(c);
    lm_controller::bench::benches(c);
    lstsq::bench::benches(c);
}
