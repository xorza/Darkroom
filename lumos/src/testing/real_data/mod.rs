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

mod background_extraction;
mod color_calibration;
mod denoise;
mod milky_way;
#[cfg(feature = "ml")]
mod ml_denoise;
#[cfg(feature = "ml")]
mod ml_perf;
mod pipeline_bench;
#[cfg(feature = "ml")]
mod star_removal;
mod stretching;

/// Shared scaffolding for the `ml`-gated real-data prototypes (`star_removal`, `ml_denoise`):
/// resolving caller-supplied weights and building the stretched display-domain master.
#[cfg(feature = "ml")]
mod ml_support {
    use std::path::PathBuf;

    use crate::color_calibration::{neutralize_background_planar, scnr_planar};
    use crate::io::astro_image::AstroImage;
    use crate::stretching::stretch_planar;
    use crate::testing::calibration_dir;
    use crate::{ScnrMethod, StretchConfig};

    /// Resolve caller-supplied ONNX weights: the `env_var` override, else `test_data/<default_file>`.
    /// Returns `None` (after a skip message) when absent — lumos ships no models, so the tests skip
    /// rather than fail when the gitignored weights aren't present.
    pub(super) fn onnx_weights(env_var: &str, default_file: &str) -> Option<PathBuf> {
        let path = std::env::var_os(env_var)
            .map(PathBuf::from)
            .unwrap_or_else(|| {
                PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                    .join("test_data")
                    .join(default_file)
            });
        if path.exists() {
            Some(path)
        } else {
            eprintln!(
                "ONNX weights not found at {} (set {env_var} or drop the .onnx there); skipping",
                path.display()
            );
            None
        }
    }

    /// Load the bundled linear master, neutralize its background and apply the default STF stretch —
    /// the display-domain `[0, 1]` input the ML filters (StarNet / DeepSNR) are trained for.
    pub(super) fn stretched_master() -> AstroImage {
        let mut img = AstroImage::from_file(calibration_dir().join("stacked_light.tiff"))
            .expect("load stacked_light.tiff");

        neutralize_background_planar(&mut img);
        stretch_planar(&mut img, StretchConfig::auto_stf());
        scnr_planar(&mut img, ScnrMethod::AverageNeutral);

        img
    }
}
