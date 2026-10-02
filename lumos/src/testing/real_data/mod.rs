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

use std::path::{Path, PathBuf};

use common::CancelToken;

use crate::io::image::linear::LinearImage;
use crate::io::image::load_context::LoadContext;
use crate::io::raw::load_raw_cfa;

use crate::io::raw::raw_files;

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
    let frames =
        raw_files::raw_files(&dataset_path(subdir)).expect("scan a real-data RAW directory");
    assert!(
        !frames.is_empty(),
        "real-data {subdir}/ holds no RAW frames"
    );
    frames
}

/// A RAW light, demosaiced without calibration: registration and detection need its stars, not
/// its noise floor.
pub(crate) fn raw_light(path: &Path) -> LinearImage {
    load_raw_cfa(path, &LoadContext::default())
        .expect("load a RAW light")
        .demosaic(&CancelToken::never())
        .expect("demosaic a RAW light")
}

/// Two RAW lights of one field.
#[derive(Debug)]
pub(crate) struct LightPair {
    pub(crate) first: LinearImage,
    pub(crate) last: LinearImage,
}

/// The first and last RAW lights of the dataset, demosaiced: two frames of one field, offset by
/// the drift of a night's sequence.
pub(crate) fn first_and_last_lights() -> LightPair {
    let lights = raw_frames("Lights");
    assert!(lights.len() >= 2, "real-data Lights/ needs two frames");
    LightPair {
        first: raw_light(&lights[0]),
        last: raw_light(&lights[lights.len() - 1]),
    }
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
