//! Shared scaffolding for real-data tests, plus the two that span more than one subsystem.
//!
//! Everything here runs against the bundled `test_data/lumos_data` dataset behind the `real-data`
//! feature. A real-data test that exercises *one* subsystem now lives with that subsystem, in its
//! `tests/real_data.rs` — this module previously held the `image_ops` ones, which made it look
//! like shared infrastructure when it was really one subsystem's test directory.
//!
//! What is left is genuinely shared or genuinely cross-cutting:
//!
//! - [`pipeline_bench`] — full master-darks/flats → calibrate → register → stack benchmark
//!   (`cargo test -p lumos --release bench_full_pipeline -- --ignored --nocapture`).
//! - [`milky_way`] — the "best Milky Way" chain: green removal, stretch, denoise, HDR and CLAHE
//!   together, so it belongs to no single image op.
//! - [`ml_support`] (feature `ml`) — weight resolution and the stretched master the `ml`
//!   prototypes in `image_ops/ml/tests/` share.

mod milky_way;
mod pipeline_bench;

/// Shared scaffolding for the `ml`-gated real-data prototypes (`star_removal`, `ml_denoise`):
/// resolving caller-supplied weights and building the stretched display-domain master.
#[cfg(feature = "ml")]
pub(crate) mod ml_support {
    use std::path::PathBuf;

    use crate::io::image::linear::LinearImage;
    use crate::io::image::load_context::LoadContext;
    use crate::testing::calibration_dir;
    use crate::{NeutralizeBackground, Scnr, Stretch};

    /// Resolve caller-supplied ONNX weights: the `env_var` override, else `test_data/<default_file>`.
    /// Returns `None` (after a skip message) when absent — lumos ships no models, so the tests skip
    /// rather than fail when the gitignored weights aren't present.
    pub(crate) fn onnx_weights(env_var: &str, default_file: &str) -> Option<PathBuf> {
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
    pub(crate) fn stretched_master() -> LinearImage {
        let mut img = LinearImage::from_file(
            calibration_dir().join("stacked_light.tiff"),
            &LoadContext::default(),
        )
        .expect("load stacked_light.tiff");

        NeutralizeBackground.apply(&mut img).unwrap();
        Stretch::auto_stf().apply(&mut img).unwrap();
        Scnr::average_neutral().apply(&mut img).unwrap();

        img
    }
}
