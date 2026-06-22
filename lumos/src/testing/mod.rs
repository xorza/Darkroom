//! Testing utilities for lumos.

pub mod real_data;
pub mod synthetic;

// ============================================================================
// Deterministic test RNG
// ============================================================================

/// Deterministic LCG random number generator for reproducible test data.
///
/// Uses the Knuth LCG: `state = state * 6364136223846793005 + 1`.
/// All synthetic data generators should use this instead of inline LCG closures.
#[derive(Debug, Clone)]
pub struct TestRng {
    state: u64,
}

impl TestRng {
    /// Create a new RNG with the given seed.
    pub fn new(seed: u64) -> Self {
        Self { state: seed }
    }

    /// Advance state and return raw u64.
    #[inline]
    pub fn next_u64(&mut self) -> u64 {
        self.state = self.state.wrapping_mul(6364136223846793005).wrapping_add(1);
        self.state
    }

    /// Return a random f32 in [0, 1).
    #[inline]
    pub fn next_f32(&mut self) -> f32 {
        (self.next_u64() >> 33) as f32 / (1u64 << 31) as f32
    }

    /// Return a random f64 in [0, 1) with full 53-bit mantissa precision.
    #[inline]
    pub fn next_f64(&mut self) -> f64 {
        (self.next_u64() >> 11) as f64 / (1u64 << 53) as f64
    }

    /// Return a Gaussian-distributed f32 with mean 0 and standard deviation 1.
    ///
    /// Uses the Box-Muller transform. Consumes two uniform samples per call.
    #[inline]
    pub fn next_gaussian_f32(&mut self) -> f32 {
        let u1 = self.next_f32().max(1e-10);
        let u2 = self.next_f32();
        (-2.0 * u1.ln()).sqrt() * (2.0 * std::f32::consts::PI * u2).cos()
    }
}

use std::path::PathBuf;

use crate::AstroImage;
use crate::io::astro_image::AstroImageMetadata;
use crate::io::astro_image::cfa::{CfaImage, CfaType};
use crate::stacking::star_detection::background::estimate::BackgroundEstimate;
use crate::stacking::star_detection::buffer_pool::BufferPool;
use crate::stacking::star_detection::config::Config;
use imaginarium::Buffer2;
use common::test_utils::test_output_path;
use imaginarium::{ColorFormat, Image};

/// Convenience function to estimate background for tests.
///
/// Returns a `BackgroundEstimate` with background and noise estimates.
/// Creates a temporary buffer pool internally.
pub(crate) fn estimate_background(pixels: &Buffer2<f32>, config: &Config) -> BackgroundEstimate {
    let mut pool = BufferPool::new(pixels.width(), pixels.height());
    crate::stacking::star_detection::background::estimate_background(pixels, config, &mut pool)
}

/// Create a CfaImage from raw pixel data and CFA type.
pub fn make_cfa(width: usize, height: usize, pixels: Vec<f32>, cfa_type: CfaType) -> CfaImage {
    CfaImage {
        data: Buffer2::new(width, height, pixels),
        metadata: AstroImageMetadata {
            cfa_type: Some(cfa_type),
            ..Default::default()
        },
    }
}

/// Create a CfaImage filled with a constant value.
pub fn constant_cfa(width: usize, height: usize, value: f32, cfa_type: CfaType) -> CfaImage {
    CfaImage {
        data: Buffer2::new_filled(width, height, value),
        metadata: AstroImageMetadata {
            cfa_type: Some(cfa_type),
            ..Default::default()
        },
    }
}

/// Returns the bundled real-data directory (`test_data/lumos_data`). Real-data tests are
/// gated behind the `real-data` feature and require the dataset, so this panics if it's
/// missing rather than making every caller handle an `Option`.
pub fn calibration_dir() -> PathBuf {
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("test_data/lumos_data");
    assert!(
        dir.is_dir(),
        "real-data dataset missing at {} (required by the `real-data` feature)",
        dir.display()
    );
    dir
}

/// Returns the calibration_masters subdirectory within the calibration directory.
/// Prints a message if not found.
pub fn calibration_masters_dir() -> Option<PathBuf> {
    let cal_dir = calibration_dir();
    let masters_dir = cal_dir.join("calibration_masters");
    if masters_dir.exists() {
        Some(masters_dir)
    } else {
        eprintln!("calibration_masters subdirectory not found");
        None
    }
}

/// Initialize tracing subscriber for tests.
/// Safe to call multiple times - will only initialize once.
/// Respects RUST_LOG env var, defaults to "info".
pub fn init_tracing() {
    use tracing_subscriber::EnvFilter;
    // `ort` (ONNX Runtime) logs its arena allocations at INFO — far too chatty; quiet it by default.
    let filter =
        EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info,ort=warn"));
    let _ = tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_test_writer()
        .try_init();
}

/// Save an `AstroImage` as an 8-bit RGB file under `test_output/<name>`, creating the directory.
/// 8-bit/JPEG output needs `RGB_U8`, so the (often `[0,1]`-float) image is converted first. Used by
/// real-data tests to write viewable results for inspection.
pub(crate) fn save_png(image: &AstroImage, name: &str) {
    let path = test_output_path(name);
    std::fs::create_dir_all(path.parent().unwrap()).expect("create test_output dir");
    Image::from(image)
        .convert(ColorFormat::RGB_U8)
        .expect("convert to RGB_U8")
        .save_file(&path)
        .expect("save png");
    eprintln!("wrote {}", path.display());
}

/// Loads all images from a subdirectory of the calibration directory.
/// Returns None if the subdirectory does not exist.
pub fn load_calibration_images(subdir: &str) -> Option<Vec<AstroImage>> {
    let cal_dir = calibration_dir();
    let dir = cal_dir.join(subdir);

    if !dir.exists() {
        return None;
    }

    let images = common::file_utils::astro_image_files(&dir)
        .iter()
        .map(|path| AstroImage::from_file(path).expect("Failed to load image"))
        .collect();
    Some(images)
}

/// Returns the first RAW file from the Lights subdirectory.
/// Returns None if no RAW files are found.
pub fn first_raw_file() -> Option<PathBuf> {
    let cal_dir = calibration_dir();
    let lights = cal_dir.join("Lights");
    if !lights.exists() {
        return None;
    }
    common::file_utils::astro_image_files(&lights)
        .first()
        .cloned()
}

/// Returns paths to all images in a subdirectory of the calibration directory.
/// Returns None if the subdirectory does not exist.
pub fn calibration_image_paths(subdir: &str) -> Option<Vec<PathBuf>> {
    let cal_dir = calibration_dir();
    let dir = cal_dir.join(subdir);

    if !dir.exists() {
        return None;
    }

    Some(common::file_utils::astro_image_files(&dir))
}
