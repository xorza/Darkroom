//! The criterion benches, which `benches/lumos.rs` runs: `cargo drive bench`.
//!
//! Two kinds, and every bench is one: `atomic`, one operation on the unit production gives it (a
//! tile, a stamp, a row, a pixel's stack of frames, or the frame an operation takes whole);
//! `integration`, one pipeline stage on a 24 MP frame or a set of them, or a public entry point
//! run end to end from the files of a session (`integration/pipeline/…`).
//!
//! Every bench has one shape. A group is one module at one `common::bench::Scale`, named
//! `<kind>/<area>/<module>`, with `/frame` added for a frame-sized atomic group. A group over a
//! set of frames takes `Scale::Slow` with its slowest bench's measured iteration. A bench is one
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

use crate::{
    bg_mesh, bit_buffer2, calibration, combine, drizzle, frame, frame_store, image, image_ops, io,
    math, noise, pipeline, registration, star_detection,
};

/// Every bench, atomic first, a top-level module at a time.
pub fn run(c: &mut Criterion) {
    math::bench::atomic(c);
    noise::bench::atomic(c);
    io::bench::atomic(c);
    image::bench::atomic(c);
    calibration::bench::atomic(c);
    frame::bench::atomic(c);
    frame_store::bench::atomic(c);
    star_detection::bench::atomic(c);
    bg_mesh::bench::atomic(c);
    bit_buffer2::bench::atomic(c);
    registration::bench::atomic(c);
    combine::bench::atomic(c);
    drizzle::bench::atomic(c);

    star_detection::bench::integration(c);
    registration::bench::integration(c);
    pipeline::bench::integration(c);
    combine::bench::integration(c);
    drizzle::bench::integration(c);
    image_ops::bench::integration(c);
    calibration::bench::integration(c);
}
