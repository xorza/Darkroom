//! Astrometry.net benchmark module for star detection validation.
//!
//! This module benchmarks the star detection algorithm against nova.astrometry.net,
//! which serves as ground truth for star positions.
//!
//! # Workflow
//!
//! 1. Load a calibrated sky image from `LUMOS_CALIBRATION_DIR`
//! 2. Split into random rectangular regions (cached)
//! 3. Upload rectangles to nova.astrometry.net for solving (cached)
//! 4. Run our star detector on the same rectangles
//! 5. Compare results and compute accuracy metrics
//!
//! # Environment Variables
//!
//! - `LUMOS_CALIBRATION_DIR` - Directory containing source images
//! - `LUMOS_TEST_CACHE_DIR` - Cache directory for rectangles and results
//! - `NOVA_API_KEY` - API key for nova.astrometry.net (optional, uses cached results if not set)
//!
//! # Example Usage
//!
//! ```rust,ignore
//! use lumos::star_detection::astrometry_benchmark::{AstrometryBenchmark};
//! use lumos::star_detection::StarDetectionConfig;
//! use lumos::testing::calibration_dir;
//!
//! let api_key = std::env::var("NOVA_API_KEY").ok();
//! let mut benchmark = AstrometryBenchmark::new(api_key)?;
//!
//! let source_image = calibration_dir()?.join("calibrated_light_only_sky.tiff");
//! let rectangles = benchmark.prepare_rectangles(&source_image, 10)?;
//!
//! // Solve through astrometry.net (requires API key)
//! benchmark.solve_rectangles(&rectangles)?;
//!
//! // Run our detector and compare
//! let config = StarDetectionConfig::default();
//! let results = benchmark.run_all(&config);
//! AstrometryBenchmark::print_summary(&results);
//! ```
//!
//! # Caching
//!
//! Both rectangles and astrometry.net results are cached:
//!
//! ```text
//! ~/.cache/lumos/astrometry_benchmark/
//! ├── manifest.json           # Rectangle metadata
//! └── rectangles/
//!     ├── rect_abc123.tiff    # Cropped image
//!     ├── rect_abc123.axy.json # Detected stars from astrometry.net
//!     └── ...
//! ```
//!
//! After first run with API key, subsequent runs can use cached results
//! without network access.

pub mod benchmark;
pub mod local_solver;
pub mod nova_client;
pub mod rectangle_cache;

// Re-exports
#[allow(unused_imports)]
pub use benchmark::{AstrometryBenchmark, AstrometryBenchmarkResult};
#[allow(unused_imports)]
pub use local_solver::LocalSolver;
#[allow(unused_imports)]
pub use nova_client::{AstrometryStar, NovaClient};
#[allow(unused_imports)]
pub use rectangle_cache::{RectangleCache, RectangleInfo};

#[cfg(test)]
mod tests {
    use super::*;
    use crate::star_detection::StarDetectionConfig;
    use crate::testing::{calibration_dir, init_tracing};

    #[test]
    #[ignore] // Requires LUMOS_CALIBRATION_DIR and NOVA_API_KEY
    fn test_full_astrometry_benchmark() {
        init_tracing();

        let api_key = match std::env::var("NOVA_API_KEY") {
            Ok(key) => key,
            Err(_) => {
                eprintln!("NOVA_API_KEY not set, skipping test");
                return;
            }
        };

        let Some(cal_dir) = calibration_dir() else {
            eprintln!("LUMOS_CALIBRATION_DIR not set, skipping test");
            return;
        };

        let source_image = cal_dir.join("calibrated_light_only_sky.tiff");
        if !source_image.exists() {
            eprintln!(
                "calibrated_light_only_sky.tiff not found in {}, skipping test",
                cal_dir.display()
            );
            return;
        }

        println!("\n=== Astrometry.net Benchmark ===\n");
        println!("Source image: {}", source_image.display());

        let mut benchmark =
            AstrometryBenchmark::new(Some(api_key)).expect("Failed to create benchmark");

        // Generate rectangles
        let rectangles = benchmark
            .prepare_rectangles(&source_image, 5)
            .expect("Failed to generate rectangles");

        println!("Generated {} rectangles", rectangles.len());

        // Solve through astrometry.net
        benchmark
            .solve_rectangles(&rectangles)
            .expect("Failed to solve rectangles");

        // Run benchmark
        let config = StarDetectionConfig {
            expected_fwhm: 4.0,
            detection_sigma: 3.0,
            min_snr: 5.0,
            ..Default::default()
        };

        let results = benchmark.run_all(&config);

        AstrometryBenchmark::print_summary(&results);

        // Assertions
        assert!(!results.is_empty(), "Should have benchmark results");

        let avg_detection_rate = results
            .iter()
            .map(|r| r.metrics.detection_rate)
            .sum::<f32>()
            / results.len() as f32;

        println!(
            "\nAverage detection rate: {:.1}%",
            avg_detection_rate * 100.0
        );

        // We should detect at least 50% of the stars astrometry.net found
        assert!(
            avg_detection_rate > 0.50,
            "Detection rate too low: {:.1}%",
            avg_detection_rate * 100.0
        );
    }

    #[test]
    #[ignore] // Requires cached results in LUMOS_TEST_CACHE_DIR
    fn test_cached_benchmark() {
        init_tracing();

        let benchmark = match AstrometryBenchmark::new(None) {
            Ok(b) => b,
            Err(e) => {
                eprintln!("Failed to create benchmark: {}", e);
                return;
            }
        };

        let solved = benchmark.cache().solved_rectangles();
        if solved.is_empty() {
            eprintln!("No cached rectangles available, skipping test");
            eprintln!("Run the full benchmark first with NOVA_API_KEY set");
            return;
        }

        println!("\n=== Running Benchmark with Cached Data ===\n");
        println!("Found {} solved rectangles in cache", solved.len());

        let config = StarDetectionConfig {
            expected_fwhm: 4.0,
            detection_sigma: 3.0,
            min_snr: 5.0,
            ..Default::default()
        };

        let results = benchmark.run_all(&config);
        AstrometryBenchmark::print_summary(&results);
    }

    #[test]
    #[ignore] // Requires LUMOS_CALIBRATION_DIR and local astrometry.net installation
    fn test_local_astrometry_benchmark() {
        init_tracing();

        if !local_solver::LocalSolver::is_available() {
            eprintln!("solve-field not found, skipping test");
            eprintln!("Install with: yay -S astrometry.net");
            return;
        }

        let Some(cal_dir) = calibration_dir() else {
            eprintln!("LUMOS_CALIBRATION_DIR not set, skipping test");
            return;
        };

        let source_image = cal_dir.join("calibrated_light_only_sky.tiff");
        if !source_image.exists() {
            eprintln!(
                "calibrated_light_only_sky.tiff not found in {}, skipping test",
                cal_dir.display()
            );
            return;
        }

        println!("\n=== Local Astrometry Benchmark ===\n");
        println!("Source image: {}", source_image.display());

        let mut benchmark = AstrometryBenchmark::new(None).expect("Failed to create benchmark");

        // Generate rectangles
        let rectangles = benchmark
            .prepare_rectangles(&source_image, 5)
            .expect("Failed to generate rectangles");

        println!("Generated {} rectangles", rectangles.len());

        // Solve locally using solve-field
        benchmark
            .solve_rectangles_local(&rectangles)
            .expect("Failed to solve rectangles locally");

        // Run benchmark
        let config = StarDetectionConfig {
            expected_fwhm: 4.0,
            detection_sigma: 3.0,
            min_snr: 5.0,
            ..Default::default()
        };

        let results = benchmark.run_all(&config);

        AstrometryBenchmark::print_summary(&results);

        // Assertions
        assert!(!results.is_empty(), "Should have benchmark results");

        let avg_detection_rate = results
            .iter()
            .map(|r| r.metrics.detection_rate)
            .sum::<f32>()
            / results.len() as f32;

        println!(
            "\nAverage detection rate: {:.1}%",
            avg_detection_rate * 100.0
        );

        // We should detect at least 10% of the stars astrometry.net found
        assert!(
            avg_detection_rate > 0.10,
            "Detection rate too low: {:.1}%",
            avg_detection_rate * 100.0
        );
    }

    #[test]
    #[ignore] // Requires LUMOS_CALIBRATION_DIR
    fn test_rectangle_generation_only() {
        init_tracing();

        let Some(cal_dir) = calibration_dir() else {
            eprintln!("LUMOS_CALIBRATION_DIR not set, skipping test");
            return;
        };

        let source_image = cal_dir.join("calibrated_light_only_sky.tiff");
        if !source_image.exists() {
            eprintln!("calibrated_light_only_sky.tiff not found, skipping test");
            return;
        }

        let mut benchmark = AstrometryBenchmark::new(None).expect("Failed to create benchmark");

        let rectangles = benchmark
            .prepare_rectangles_with_size(&source_image, 3, (512, 512), (1024, 1024), Some(42))
            .expect("Failed to generate rectangles");

        println!("\nGenerated {} rectangles:", rectangles.len());
        for rect in &rectangles {
            println!(
                "  {} - {}x{} at ({}, {})",
                rect.id, rect.width, rect.height, rect.x, rect.y
            );

            let path = benchmark.cache().rectangle_image_path(rect);
            assert!(
                path.exists(),
                "Rectangle file not found: {}",
                path.display()
            );
        }
    }
}
