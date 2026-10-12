//! The criterion benches, one entry for each tier, which `benches/lumos.rs` runs.
//!
//! Three tiers, and every bench is in one: `kernel`, one operation on the unit production gives
//! it (a tile, a stamp, a row, a pixel's stack of frames); `stage`, one pipeline stage on a 24 MP
//! frame or a set of them; `pipeline`, a public entry point run end to end from files.
//!
//! Every bench has one shape. A group is one module at one `fixture::Scale`, named
//! `<tier>/<area>/<module>`, with `/frame` added for a frame-sized group of the kernel tier. A
//! bench is one operation, its id `<operation>/<variant>` and its parameter the input (a length, a
//! side, a count, a matrix shape or a scene), and its throughput the elements one iteration
//! processes. A bench builds its input inside its `bench_function` closure, so a filter that skips
//! it skips its setup too. Nothing but the operation is timed: one that changes its input in place
//! takes a fresh copy through `iter_batched_ref`, one that consumes it a fresh copy through
//! `iter_batched`, and one that returns a frame drops it through `iter_with_large_drop`.

pub(crate) mod fixture;
pub(crate) mod scene;

use criterion::Criterion;

use crate::calibration_masters::{self, cosmic_ray, defect_map, prepared_flat};
use crate::combine::cache::sample;
use crate::combine::{normalization, rejection};
use crate::frame_store::{self, frame_stats};
use crate::io::image::fits::decode as fits_decode;
use crate::io::raw::{self, demosaic};
use crate::math::{
    dmat3, floor_i32, lanczos, linear_system, lm_controller, lstsq, noise, pixel_quadrature,
    statistics, sum, wavelet,
};
use crate::{background_mesh, bit_buffer2, registration, star_detection};

/// The kernel tier: one operation on the unit production gives it.
pub fn kernel(c: &mut Criterion) {
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
    raw::bench::benches(c);
    demosaic::bench::benches(c);
    fits_decode::bench::benches(c);
    calibration_masters::bench::benches(c);
    defect_map::bench::benches(c);
    cosmic_ray::bench::benches(c);
    prepared_flat::bench::benches(c);
    frame_stats::bench::benches(c);
    frame_store::bench::benches(c);
    star_detection::bench::benches(c);
    background_mesh::bench::benches(c);
    bit_buffer2::bench::benches(c);
    registration::bench::benches(c);
    rejection::bench::benches(c);
    sample::bench::benches(c);
    normalization::bench::benches(c);
}
