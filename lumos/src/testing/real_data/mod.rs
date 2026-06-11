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

/// Shared scaffolding for the `ml`-gated real-data prototypes (`star_removal`, `ml_denoise`):
/// resolving caller-supplied weights, building the stretched display-domain master, and cropping.
#[cfg(feature = "ml")]
mod ml_support {
    use std::path::PathBuf;

    use common::Vec2us;

    use crate::io::astro_image::{AstroImage, ImageDimensions};
    use crate::testing::calibration_dir;
    use crate::{StretchConfig, neutralize_background, stretch};

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
        neutralize_background(&mut img);
        stretch(&mut img, StretchConfig::auto_stf());
        img
    }

    /// Centre `cw×ch` crop of `image` into a fresh `AstroImage`.
    pub(super) fn center_crop(image: &AstroImage, cw: usize, ch: usize) -> AstroImage {
        let iw = image.width();
        let x0 = (iw - cw) / 2;
        let y0 = (image.height() - ch) / 2;
        let channels: Vec<Vec<f32>> = (0..image.channels())
            .map(|c| {
                let src = image.channel(c).pixels();
                let mut out = Vec::with_capacity(cw * ch);
                for yy in 0..ch {
                    let r = (y0 + yy) * iw + x0;
                    out.extend_from_slice(&src[r..r + cw]);
                }
                out
            })
            .collect();
        AstroImage::from_planar_channels(
            ImageDimensions::new(Vec2us::new(cw, ch), image.channels()),
            channels,
        )
    }
}
