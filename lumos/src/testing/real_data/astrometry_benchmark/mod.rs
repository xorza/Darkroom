//! Astrometry.net benchmark module for star detection validation.
//!
//! This module benchmarks the star detection algorithm against astrometry.net's
//! image2xy source extractor, which serves as ground truth for star positions.
//!
//! # Workflow
//!
//! 1. Load a calibrated sky image from `LUMOS_CALIBRATION_DIR`
//! 2. Split into random rectangular regions (cached)
//! 3. Run image2xy (local astrometry.net) on rectangles to get ground truth
//! 4. Run our star detector on the same rectangles
//! 5. Compare results and compute accuracy metrics
//!
//! # Requirements
//!
//! - Local astrometry.net installation with `image2xy` command
//! - Install on Linux: the `astrometry.net` package
//!
//! # Environment Variables
//!
//! - `LUMOS_CALIBRATION_DIR` - Directory containing source images
//! - `LUMOS_TEST_CACHE_DIR` - Cache directory for rectangles and results
//!
//! # Example Usage
//!
//! ```rust,ignore
//! use lumos::star_detection::astrometry_benchmark::AstrometryBenchmark;
//! use lumos::star_detection::Config;
//! use lumos::testing::calibration_dir;
//!
//! let mut benchmark = AstrometryBenchmark::new()?;
//!
//! let source_image = calibration_dir()?.join("calibrated_light_only_sky.tiff");
//! let rectangles = benchmark.prepare_rectangles(&source_image, 10)?;
//!
//! // Extract stars using local image2xy
//! benchmark.solve_rectangles_local(&rectangles)?;
//!
//! // Run our detector and compare
//! let config = Config::default();
//! let results = benchmark.run_all(&config);
//! AstrometryBenchmark::print_summary(&results);
//! ```
//!
//! # Caching
//!
//! Both rectangles and extraction results are cached:
//!
//! ```text
//! ~/.cache/lumos/astrometry_benchmark/
//! ├── manifest.json           # Rectangle metadata
//! └── rectangles/
//!     ├── rect_abc123.fits    # Cropped image (FITS format for image2xy)
//!     ├── rect_abc123.axy.json # Detected stars from image2xy
//!     └── ...
//! ```

pub mod benchmark;
pub mod local_solver;
pub mod rectangle_cache;

// Re-exports
#[allow(unused_imports)]
pub use benchmark::{AstrometryBenchmark, AstrometryBenchmarkResult};
#[allow(unused_imports)]
pub use local_solver::{AstrometryStar, LocalSolver};
#[allow(unused_imports)]
pub use rectangle_cache::{RectangleCache, RectangleInfo};

// Tests that require real data have been moved to `tests/real_data.rs`.
// Run with: `cargo test --test real_data`
