//! Testing utilities for lumos.

use std::f32::consts::PI;
use std::ops::Deref;
use std::path::{Path, PathBuf};

use common::file_utils;
use imaginarium::Buffer2;

use crate::io::image::cfa::{CfaImage, CfaType};
use crate::io::image::image_metadata::ImageMetadata;
use crate::io::raw::RAW_EXTENSIONS;
use crate::math::size2us::Size2us;

pub(crate) mod assertions;
pub(crate) mod images;
pub(crate) mod mem_probe;
pub(crate) mod prelude;
#[cfg(feature = "real-data")]
mod real_data;
pub(crate) mod synthetic;
pub(crate) mod visual;

#[derive(Debug)]
pub(crate) struct ScratchDirectory(PathBuf);

impl ScratchDirectory {
    pub(crate) fn new(name: &str) -> Self {
        let path = std::env::current_dir()
            .unwrap()
            .join(".tmp")
            .join(format!("{name}_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&path);
        std::fs::create_dir_all(&path).unwrap();
        Self(path)
    }
}

impl Deref for ScratchDirectory {
    type Target = Path;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl AsRef<Path> for ScratchDirectory {
    fn as_ref(&self) -> &Path {
        &self.0
    }
}

impl Drop for ScratchDirectory {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// Deterministic LCG random number generator for reproducible test data.
///
/// Uses the Knuth LCG: `state = state * 6364136223846793005 + 1`.
/// All synthetic data generators should use this instead of inline LCG closures.
#[derive(Debug, Clone)]
pub(crate) struct TestRng {
    state: u64,
}

impl TestRng {
    /// Create a new RNG with the given seed.
    pub(crate) fn new(seed: u64) -> Self {
        Self { state: seed }
    }

    /// Advance state and return raw u64.
    #[inline]
    pub(crate) fn next_u64(&mut self) -> u64 {
        self.state = self.state.wrapping_mul(6364136223846793005).wrapping_add(1);
        self.state
    }

    /// Return a random f32 in [0, 1).
    #[inline]
    pub(crate) fn next_f32(&mut self) -> f32 {
        (self.next_u64() >> 33) as f32 / (1u64 << 31) as f32
    }

    /// Return a random f64 in [0, 1) with full 53-bit mantissa precision.
    #[inline]
    pub(crate) fn next_f64(&mut self) -> f64 {
        (self.next_u64() >> 11) as f64 / (1u64 << 53) as f64
    }

    /// Return a Gaussian-distributed f32 with mean 0 and standard deviation 1.
    ///
    /// Uses the Box-Muller transform. Consumes two uniform samples per call.
    #[inline]
    pub(crate) fn next_gaussian_f32(&mut self) -> f32 {
        let u1 = self.next_f32().max(1e-10);
        let u2 = self.next_f32();
        (-2.0 * u1.ln()).sqrt() * (2.0 * PI * u2).cos()
    }
}

/// Create a CfaImage from raw pixel data and CFA type.
pub(crate) fn make_cfa(size: Size2us, pixels: Vec<f32>, cfa_type: CfaType) -> CfaImage {
    CfaImage {
        data: Buffer2::new(size.width, size.height, pixels),
        metadata: ImageMetadata {
            cfa_type: Some(cfa_type),
            ..Default::default()
        },
        quantization_sigma: None,
    }
}

/// Create a CfaImage filled with a constant value.
pub(crate) fn constant_cfa(size: Size2us, value: f32, cfa_type: CfaType) -> CfaImage {
    CfaImage {
        data: Buffer2::new_filled(size.width, size.height, value),
        metadata: ImageMetadata {
            cfa_type: Some(cfa_type),
            ..Default::default()
        },
        quantization_sigma: None,
    }
}

/// Returns the bundled real-data directory (`test_data/lumos_data`). Real-data tests are
/// gated behind the `real-data` feature and require the dataset, so this panics if it's
/// missing rather than making every caller handle an `Option`.
pub(crate) fn calibration_dir() -> PathBuf {
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("test_data/lumos_data");
    assert!(
        dir.is_dir(),
        "real-data dataset missing at {} (required by the `real-data` feature)",
        dir.display()
    );
    dir
}

/// Initialize tracing subscriber for tests.
/// Safe to call multiple times - will only initialize once.
/// Respects RUST_LOG env var, defaults to "info".
pub(crate) fn init_tracing() {
    use tracing_subscriber::EnvFilter;
    // `ort` (ONNX Runtime) logs its arena allocations at INFO — far too chatty; quiet it by default.
    let filter =
        EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info,ort=warn"));
    let _ = tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_test_writer()
        .try_init();
}

/// Save an image as an 8-bit RGB file under `test_output/<name>`, creating the directory.
/// 8-bit/JPEG output needs `RGB_U8`, so the (often `[0,1]`-float) image is converted first. Used by
/// real-data tests to write viewable results for inspection.
#[cfg(feature = "real-data")]
/// Returns the first RAW file from the Lights subdirectory.
/// Returns None if no RAW files are found.
#[cfg(feature = "real-data")]
pub(crate) fn first_raw_file() -> Option<PathBuf> {
    let cal_dir = calibration_dir();
    let lights = cal_dir.join("Lights");
    if !lights.exists() {
        return None;
    }
    file_utils::files_with_extensions(&lights, RAW_EXTENSIONS)
        .expect("scan RAW lights directory")
        .first()
        .cloned()
}

/// Returns paths to all camera-RAW images in a calibration subdirectory.
/// Returns None if the subdirectory does not exist.
pub(crate) fn calibration_image_paths(subdir: &str) -> Option<Vec<PathBuf>> {
    let cal_dir = calibration_dir();
    let dir = cal_dir.join(subdir);

    if !dir.exists() {
        return None;
    }

    Some(
        file_utils::files_with_extensions(&dir, RAW_EXTENSIONS)
            .expect("scan RAW calibration directory"),
    )
}

#[cfg(test)]
mod tests {
    use crate::testing::ScratchDirectory;

    #[test]
    fn scratch_directory_cleans_up_on_drop() {
        let path;
        {
            let directory = ScratchDirectory::new("scratch_directory_cleanup");
            path = directory.to_path_buf();
            std::fs::write(directory.join("probe"), b"test").unwrap();
            assert!(path.join("probe").is_file());
        }
        assert!(!path.exists());
    }
}
