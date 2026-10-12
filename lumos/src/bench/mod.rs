//! The criterion benches, which `benches/lumos.rs` runs: `./x.py bench`.
//!
//! Two kinds, and every bench is one: `atomic`, one operation on the unit production gives it (a
//! tile, a stamp, a row, a pixel's stack of frames, or the frame an operation takes whole);
//! `integration`, one pipeline stage on a 24 MP frame or a set of them, or a public entry point
//! run end to end from the files of a session (`integration/pipeline/…`).
//!
//! Every bench has one shape. A group is one module at one `fixture::Scale`, named
//! `<kind>/<area>/<module>`, with `/frame` added for a frame-sized atomic group. A group over a
//! set of frames takes `Scale::Set` with its slowest bench's measured iteration. A bench is one
//! operation, its id `<operation>/<variant>` and its parameter the input (a length, a side, a
//! count, a matrix shape or a scene), and its throughput the elements one iteration processes. A
//! bench builds its input inside its `bench_function` closure, so a filter that skips it skips its
//! setup too. Nothing but the operation is timed: one that changes its input in place takes a
//! fresh copy through `iter_batched_ref`, one that consumes it a fresh copy through
//! `iter_batched`, and one that returns a frame drops it through `iter_with_large_drop`.

pub(crate) mod files;
pub(crate) mod fixture;
pub(crate) mod scene;

use criterion::Criterion;

use crate::calibration::{self, cosmic_ray, defect_map, prepared_flat};
use crate::combine::frame_cache::sample;
use crate::combine::stack;
use crate::combine::{normalization, rejection};
use crate::drizzle::accumulator;
use crate::frame::frame_stats;
use crate::frame_store;
use crate::image::cfa::demosaic;
use crate::io::fits::decode as fits_decode;
use crate::io::raw;
use crate::math::{
    dmat3, floor_i32, lanczos, linear_system, lm_controller, lstsq, noise, pixel_quadrature,
    statistics, sum, wavelet,
};
use crate::registration::resample;
use crate::star_detection::star_detector;
use crate::{
    background_mesh, bit_buffer2, combine, drizzle, image_ops, pipeline, registration,
    star_detection,
};

/// Every bench, atomic first.
pub fn run(c: &mut Criterion) {
    sum::bench::atomic(c);
    statistics::bench::atomic(c);
    noise::bench::atomic(c);
    wavelet::bench::atomic(c);
    lanczos::bench::atomic(c);
    dmat3::bench::atomic(c);
    floor_i32::bench::atomic(c);
    pixel_quadrature::bench::atomic(c);
    linear_system::bench::atomic(c);
    lm_controller::bench::atomic(c);
    lstsq::bench::atomic(c);
    raw::bench::atomic(c);
    demosaic::bench::atomic(c);
    fits_decode::bench::atomic(c);
    calibration::bench::atomic(c);
    defect_map::bench::atomic(c);
    cosmic_ray::bench::atomic(c);
    prepared_flat::bench::atomic(c);
    frame_stats::bench::atomic(c);
    frame_store::bench::atomic(c);
    star_detection::bench::atomic(c);
    background_mesh::bench::atomic(c);
    bit_buffer2::bench::atomic(c);
    registration::bench::atomic(c);
    rejection::bench::atomic(c);
    sample::bench::atomic(c);
    normalization::bench::atomic(c);
    accumulator::bench::atomic(c);

    star_detector::bench::integration(c);
    registration::bench::integration(c);
    resample::bench::integration(c);
    pipeline::bench::integration(c);
    combine::bench::integration(c);
    drizzle::bench::integration(c);
    image_ops::bench::integration(c);
    stack::bench::integration(c);
    calibration::bench::integration(c);
}
