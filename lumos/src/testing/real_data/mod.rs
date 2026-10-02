//! Shared scaffolding for real-data tests, plus the two that span more than one subsystem.
//!
//! Everything here runs against the bundled `test_data/lumos_data` dataset behind the `real-data`
//! feature. A real-data test that exercises *one* subsystem now lives with that subsystem, in its
//! `tests/real_data.rs`, so this module is not one subsystem's test directory wearing the name
//! of shared infrastructure.
//!
//! What is left is genuinely shared or genuinely cross-cutting:
//!
//! - [`pipeline_bench`] — full master-darks/flats → calibrate → register → stack benchmark
//!   (`cargo test -p lumos --release bench_full_pipeline -- --ignored --nocapture`).
//! - [`milky_way`] — the "best Milky Way" chain: green removal, stretch, denoise, HDR and CLAHE
//!   together, so it belongs to no single image op.
//! - [`ml_support`] (feature `ml`) — weight resolution and the stretched master the `ml`
//!   prototypes in `image_ops/ml/tests/` share.

use std::path::PathBuf;

use common::file_utils;

use crate::io::raw::RAW_EXTENSIONS;

mod milky_way;
mod pipeline_bench;

/// The bundled dataset, `test_data/lumos_data`. The `real-data` feature states that it is
/// present, so a missing dataset or entry is a setup error: every accessor here panics and
/// names what is missing, and none hands back an `Option` to skip on.
pub(crate) fn dataset_dir() -> PathBuf {
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("test_data/lumos_data");
    assert!(
        dir.is_dir(),
        "real-data dataset missing at {} (fetch it with scripts/fetch-test-data.sh)",
        dir.display()
    );
    dir
}

/// A file or directory of the dataset.
pub(crate) fn dataset_path(name: &str) -> PathBuf {
    let path = dataset_dir().join(name);
    assert!(path.exists(), "real-data dataset has no {}", path.display());
    path
}

/// The camera-RAW frames of a dataset subdirectory (`Lights`, `Darks`, `Flats`, `Bias`), at
/// least one, in name order.
pub(crate) fn raw_frames(subdir: &str) -> Vec<PathBuf> {
    let frames = file_utils::files_with_extensions(&dataset_path(subdir), RAW_EXTENSIONS)
        .expect("scan a real-data RAW directory");
    assert!(
        !frames.is_empty(),
        "real-data {subdir}/ holds no RAW frames"
    );
    frames
}

/// Shared scaffolding for the `ml`-gated real-data prototypes (`star_removal`, `ml_denoise`):
/// resolving caller-supplied weights and building the stretched display-domain master.
#[cfg(feature = "ml")]
pub(crate) mod ml_support {
    use std::env;
    use std::path::PathBuf;

    use crate::io::image::linear::LinearImage;
    use crate::io::image::load_context::LoadContext;
    use crate::testing::real_data::dataset_path;
    use crate::{NeutralizeBackground, Scnr, Stretch};

    /// Resolve caller-supplied ONNX weights: the `env_var` override, else `test_data/<default_file>`.
    /// Returns `None` (after a skip message) when absent — lumos ships no models, so the tests skip
    /// rather than fail when the gitignored weights aren't present.
    pub(crate) fn onnx_weights(env_var: &str, default_file: &str) -> Option<PathBuf> {
        let path = env::var_os(env_var).map_or_else(
            || {
                PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                    .join("test_data")
                    .join(default_file)
            },
            PathBuf::from,
        );
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
        let mut img =
            LinearImage::from_file(dataset_path("stacked_light.tiff"), &LoadContext::default())
                .expect("load stacked_light.tiff");

        NeutralizeBackground.apply(&mut img).unwrap();
        Stretch::auto_stf().apply(&mut img).unwrap();
        Scnr::average_neutral().apply(&mut img).unwrap();

        img
    }
}
