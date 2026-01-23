//! Demosaicing module for CFA (Color Filter Array) sensors.
//!
//! This module provides demosaicing algorithms for different sensor types:
//! - Bayer CFA patterns (RGGB, BGGR, GRBG, GBRG)
//! - X-Trans 6x6 patterns (Fujifilm sensors)
//!
//! Re-exports the main types and functions from the submodules.

mod bayer;
pub(crate) mod xtrans;

pub use bayer::{BayerImage, CfaPattern, demosaic_bilinear};

#[cfg(feature = "bench")]
pub mod bench {
    pub use super::bayer::bench as bayer;
    pub use super::xtrans::bench as xtrans;

    use criterion::Criterion;
    use std::path::Path;

    /// Register all demosaic benchmarks (both Bayer and X-Trans if applicable).
    pub fn benchmarks(c: &mut Criterion, raw_file_path: &Path) {
        // Always run Bayer benchmarks
        bayer::benchmarks(c, raw_file_path);

        // Try to run X-Trans benchmarks if the file is an X-Trans sensor
        // We check by attempting to load - if it fails the assertion, we skip
        // For now, we only run X-Trans benchmarks if explicitly requested
    }
}
