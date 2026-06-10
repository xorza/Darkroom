//! Real-data tests and benchmarks against the bundled `test_data/lumos_data` dataset, gated behind
//! the `real-data` feature.
//!
//! - [`pipeline_bench`] — full master-darks/flats → calibrate → register → stack pipeline benchmark
//!   (`cargo test -p lumos --release bench_full_pipeline -- --ignored --nocapture`).
//! - [`color_calibration`] — background neutralization + SCNR on the bundled stacked light frame.

mod color_calibration;
mod pipeline_bench;
