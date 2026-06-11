//! Real-data tests and benchmarks against the bundled `test_data/lumos_data` dataset, gated behind
//! the `real-data` feature.
//!
//! - [`pipeline_bench`] — full master-darks/flats → calibrate → register → stack pipeline benchmark
//!   (`cargo test -p lumos --release bench_full_pipeline -- --ignored --nocapture`).
//! - [`color_calibration`] — background neutralization + SCNR on the bundled stacked light frame.
//! - [`denoise`] — linear-domain wavelet denoising of the bundled stacked light frame.
//! - [`stretching`] — STF/asinh/GHS display stretches of the bundled stacked light frame.
//! - [`milky_way`] — full "best Milky Way" pipeline: green removal + stretch + denoise/HDR/CLAHE.
//! - [`star_removal`] (feature `ml`) — StarNet ONNX star removal on a crop (caller-supplied weights).
//! - [`ml_denoise`] (feature `ml`) — DeepSNR ONNX denoiser on a crop (caller-supplied weights).

mod color_calibration;
mod denoise;
mod milky_way;
#[cfg(feature = "ml")]
mod ml_denoise;
mod pipeline_bench;
#[cfg(feature = "ml")]
mod star_removal;
mod stretching;
