//! Benchmark runner for comparing star detection against astrometry.net.
//!
//! Orchestrates rectangle generation, local solving, and metrics computation.

use super::local_solver::{AstrometryStar, LocalSolver};
use super::rectangle_cache::{RectangleCache, RectangleInfo};
use crate::{AstroImage, ImageDimensions};

use crate::star_detection::tests::common::output::{
    DetectionMetrics, compute_detection_metrics, save_comparison, save_grayscale, save_metrics,
};
use crate::star_detection::{Star, StarDetectionConfig, StarDetector};
use crate::testing::synthetic::GroundTruthStar;
use anyhow::Result;
use std::path::{Path, PathBuf};
use std::time::Instant;

/// Default rectangle size range.
const DEFAULT_MIN_SIZE: (usize, usize) = (512, 512);
const DEFAULT_MAX_SIZE: (usize, usize) = (1024, 1024);

/// Result of benchmarking a single rectangle.
#[derive(Debug)]
pub struct AstrometryBenchmarkResult {
    /// Rectangle information.
    pub rectangle: RectangleInfo,
    /// Number of stars from astrometry.net.
    pub astrometry_stars: usize,
    /// Number of stars we detected.
    pub detected_stars: usize,
    /// Detection metrics comparing our detector to astrometry.net.
    pub metrics: DetectionMetrics,
    /// Our detection runtime in milliseconds.
    pub runtime_ms: u64,
}

impl AstrometryBenchmarkResult {
    /// Print a summary of the result.
    pub fn print_summary(&self) {
        println!(
            "Rectangle {}: {}x{} at ({}, {})",
            self.rectangle.id,
            self.rectangle.width,
            self.rectangle.height,
            self.rectangle.x,
            self.rectangle.y
        );
        println!(
            "  Astrometry.net stars: {}, Our detections: {}",
            self.astrometry_stars, self.detected_stars
        );
        println!(
            "  Detection rate: {:.1}% ({}/{} astrometry stars matched)",
            self.metrics.detection_rate * 100.0,
            self.metrics.true_positives,
            self.astrometry_stars
        );
        println!(
            "  Mean centroid error: {:.3} pixels",
            self.metrics.mean_centroid_error
        );
        println!("  Runtime: {} ms", self.runtime_ms);
    }
}

/// Benchmark runner for astrometry.net comparison.
#[derive(Debug)]
pub struct AstrometryBenchmark {
    cache: RectangleCache,
    output_dir: PathBuf,
}

impl AstrometryBenchmark {
    /// Create a new benchmark runner.
    pub fn new() -> Result<Self> {
        let cache = RectangleCache::new()?;
        let output_dir = common::test_utils::test_output_path("astrometry_benchmark");

        Ok(Self { cache, output_dir })
    }

    /// Create a benchmark runner with custom directories.
    pub fn with_dirs(cache_dir: PathBuf, output_dir: PathBuf) -> Result<Self> {
        let cache = RectangleCache::with_cache_dir(cache_dir)?;

        std::fs::create_dir_all(&output_dir)?;

        Ok(Self { cache, output_dir })
    }

    /// Get the rectangle cache.
    pub fn cache(&self) -> &RectangleCache {
        &self.cache
    }

    /// Get the rectangle cache mutably.
    #[allow(dead_code)]
    pub fn cache_mut(&mut self) -> &mut RectangleCache {
        &mut self.cache
    }

    /// Generate rectangles from a source image.
    ///
    /// Uses cached rectangles if available.
    pub fn prepare_rectangles(
        &mut self,
        source_image: &Path,
        num_rectangles: usize,
    ) -> Result<Vec<RectangleInfo>> {
        self.prepare_rectangles_with_size(
            source_image,
            num_rectangles,
            DEFAULT_MIN_SIZE,
            DEFAULT_MAX_SIZE,
            Some(42), // Deterministic seed for reproducibility
        )
    }

    /// Generate rectangles with custom size parameters.
    pub fn prepare_rectangles_with_size(
        &mut self,
        source_image: &Path,
        num_rectangles: usize,
        min_size: (usize, usize),
        max_size: (usize, usize),
        seed: Option<u64>,
    ) -> Result<Vec<RectangleInfo>> {
        self.cache
            .generate_rectangles(source_image, num_rectangles, min_size, max_size, seed)
    }

    /// Solve rectangles using local image2xy command.
    ///
    /// This is much faster than the web API and doesn't require an API key.
    /// Requires astrometry.net to be installed locally with index files.
    pub fn solve_rectangles_local(&mut self, rectangles: &[RectangleInfo]) -> Result<()> {
        if !LocalSolver::is_available() {
            anyhow::bail!("solve-field not found - install astrometry.net package");
        }

        let solver = LocalSolver::new();

        let unsolved: Vec<_> = rectangles.iter().filter(|r| r.axy_path.is_none()).collect();

        if unsolved.is_empty() {
            tracing::info!("All {} rectangles already solved", rectangles.len());
            return Ok(());
        }

        tracing::info!(
            "Solving {} unsolved rectangles locally (out of {})",
            unsolved.len(),
            rectangles.len()
        );

        for (i, rect) in unsolved.iter().enumerate() {
            tracing::info!(
                "Solving rectangle {}/{}: {} ({}x{})",
                i + 1,
                unsolved.len(),
                rect.id,
                rect.width,
                rect.height
            );

            let image_path = self.cache.rectangle_image_path(rect);

            match solver.solve_and_get_stars(&image_path) {
                Ok(stars) => {
                    self.cache.save_axy_result(&rect.id, 0, &stars)?;
                    tracing::info!("Solved rectangle {}: {} stars", rect.id, stars.len());
                }
                Err(e) => {
                    tracing::error!("Failed to solve rectangle {}: {}", rect.id, e);
                    // Continue with other rectangles
                }
            }
        }

        Ok(())
    }

    /// Run benchmark on a single rectangle.
    pub fn run_rectangle(
        &self,
        rect: &RectangleInfo,
        config: &StarDetectionConfig,
    ) -> Result<AstrometryBenchmarkResult> {
        // Load astrometry.net stars as ground truth
        let astrometry_stars = self.cache.load_axy_result(rect)?;

        // Load rectangle image
        let (pixels, width, height) = self.cache.load_rectangle_image(rect)?;

        // Convert astrometry stars to ground truth format
        let ground_truth = astrometry_to_ground_truth(&astrometry_stars);

        // Run our detector
        let start = Instant::now();
        let image = AstroImage::from_pixels(ImageDimensions::new(width, height, 1), pixels.clone());
        let mut detector = StarDetector::from_config(config.clone());
        let result = detector.detect(&image);
        let runtime_ms = start.elapsed().as_millis() as u64;

        // Compute metrics
        let match_radius = config.psf.expected_fwhm.max(4.0) * 2.0;
        let metrics = compute_detection_metrics(&ground_truth, &result.stars, match_radius);

        // Save output images
        self.save_outputs(
            rect,
            &pixels,
            width,
            height,
            &ground_truth,
            &result.stars,
            &metrics,
        )?;

        Ok(AstrometryBenchmarkResult {
            rectangle: rect.clone(),
            astrometry_stars: astrometry_stars.len(),
            detected_stars: result.stars.len(),
            metrics,
            runtime_ms,
        })
    }

    /// Run benchmark on all solved rectangles.
    pub fn run_all(&self, config: &StarDetectionConfig) -> Vec<AstrometryBenchmarkResult> {
        let solved = self.cache.solved_rectangles();

        if solved.is_empty() {
            tracing::warn!("No solved rectangles to benchmark");
            return Vec::new();
        }

        tracing::info!("Running benchmark on {} solved rectangles", solved.len());

        solved
            .into_iter()
            .filter_map(|rect| match self.run_rectangle(rect, config) {
                Ok(result) => Some(result),
                Err(e) => {
                    tracing::error!("Failed to benchmark rectangle {}: {}", rect.id, e);
                    None
                }
            })
            .collect()
    }

    /// Print summary of benchmark results.
    pub fn print_summary(results: &[AstrometryBenchmarkResult]) {
        if results.is_empty() {
            println!("No benchmark results");
            return;
        }

        println!("\n=== Astrometry.net Benchmark Summary ===\n");

        let total_astrometry = results.iter().map(|r| r.astrometry_stars).sum::<usize>();
        let total_detected = results.iter().map(|r| r.detected_stars).sum::<usize>();
        let total_matched = results
            .iter()
            .map(|r| r.metrics.true_positives)
            .sum::<usize>();

        let avg_detection_rate = results
            .iter()
            .map(|r| r.metrics.detection_rate)
            .sum::<f32>()
            / results.len() as f32;
        let avg_centroid_error = results
            .iter()
            .filter(|r| r.metrics.mean_centroid_error > 0.0)
            .map(|r| r.metrics.mean_centroid_error)
            .sum::<f32>()
            / results
                .iter()
                .filter(|r| r.metrics.mean_centroid_error > 0.0)
                .count()
                .max(1) as f32;

        println!("Rectangles tested: {}", results.len());
        println!(
            "Total astrometry.net stars: {}, Our detections: {}",
            total_astrometry, total_detected
        );
        println!(
            "Total matched: {} ({:.1}%)",
            total_matched,
            if total_astrometry > 0 {
                total_matched as f32 / total_astrometry as f32 * 100.0
            } else {
                0.0
            }
        );
        println!("Average detection rate: {:.1}%", avg_detection_rate * 100.0);
        println!("Average centroid error: {:.3} pixels", avg_centroid_error);

        println!("\n--- Per-Rectangle Results ---\n");
        for result in results {
            result.print_summary();
            println!();
        }
    }

    /// Save benchmark output images.
    #[allow(clippy::too_many_arguments)]
    fn save_outputs(
        &self,
        rect: &RectangleInfo,
        pixels: &[f32],
        width: usize,
        height: usize,
        ground_truth: &[GroundTruthStar],
        detected: &[Star],
        metrics: &DetectionMetrics,
    ) -> Result<()> {
        let rect_dir = self.output_dir.join(&rect.id);
        std::fs::create_dir_all(&rect_dir)?;

        // Save input image
        save_grayscale(pixels, width, height, &rect_dir.join("input.png"));

        // Save comparison image
        let match_radius = 8.0;
        save_comparison(
            pixels,
            width,
            height,
            ground_truth,
            detected,
            match_radius,
            &rect_dir.join("comparison.png"),
        );

        // Save metrics
        save_metrics(metrics, &rect_dir.join("metrics.txt"));

        Ok(())
    }
}

/// Convert astrometry.net stars to ground truth format.
fn astrometry_to_ground_truth(stars: &[AstrometryStar]) -> Vec<GroundTruthStar> {
    stars
        .iter()
        .map(|s| GroundTruthStar {
            x: s.x,
            y: s.y,
            flux: s.flux,
            fwhm: 4.0, // Estimate - astrometry.net doesn't provide FWHM
            eccentricity: 0.0,
            is_saturated: false,
            angle: 0.0,
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::calibration_dir;

    #[test]
    #[ignore] // Benchmark test - run with --ignored
    fn test_benchmark_creation() {
        let temp_dir = std::env::temp_dir().join(format!("lumos_bench_{}", std::process::id()));
        let output_dir = temp_dir.join("output");
        let cache_dir = temp_dir.join("cache");

        let benchmark = AstrometryBenchmark::with_dirs(cache_dir, output_dir);
        assert!(benchmark.is_ok());

        // Cleanup
        let _ = std::fs::remove_dir_all(&temp_dir);
    }

    #[test]
    #[ignore] // Requires LUMOS_CALIBRATION_DIR
    fn test_rectangle_generation() {
        let Some(cal_dir) = calibration_dir() else {
            eprintln!("LUMOS_CALIBRATION_DIR not set, skipping test");
            return;
        };

        let source_image = cal_dir.join("calibrated_light_only_sky.tiff");
        if !source_image.exists() {
            eprintln!("calibrated_light_only_sky.tiff not found, skipping test");
            return;
        }

        let temp_dir = std::env::temp_dir().join(format!("lumos_rect_{}", std::process::id()));
        let cache_dir = temp_dir.join("cache");
        let output_dir = temp_dir.join("output");

        let mut benchmark = AstrometryBenchmark::with_dirs(cache_dir, output_dir).unwrap();

        let rectangles = benchmark.prepare_rectangles(&source_image, 3).unwrap();

        assert_eq!(rectangles.len(), 3);
        for rect in &rectangles {
            assert!(rect.width >= 512 && rect.width <= 1024);
            assert!(rect.height >= 512 && rect.height <= 1024);
            let path = benchmark.cache().rectangle_image_path(rect);
            assert!(
                path.exists(),
                "Rectangle image not created: {}",
                path.display()
            );
        }

        // Cleanup
        let _ = std::fs::remove_dir_all(&temp_dir);
    }

    #[test]
    #[ignore] // Requires LUMOS_CALIBRATION_DIR and local astrometry.net
    fn test_full_benchmark_local() {
        crate::testing::init_tracing();

        let Some(cal_dir) = calibration_dir() else {
            eprintln!("LUMOS_CALIBRATION_DIR not set, skipping test");
            return;
        };

        let source_image = cal_dir.join("calibrated_light_only_sky.tiff");
        if !source_image.exists() {
            eprintln!("calibrated_light_only_sky.tiff not found, skipping test");
            return;
        }

        if !LocalSolver::is_available() {
            eprintln!("Local astrometry.net not installed, skipping test");
            return;
        }

        let mut benchmark = AstrometryBenchmark::new().unwrap();

        // Generate 2 rectangles for testing
        let rectangles = benchmark.prepare_rectangles(&source_image, 2).unwrap();

        // Solve using local solver
        benchmark.solve_rectangles_local(&rectangles).unwrap();

        // Run benchmark
        let config = StarDetectionConfig::default();
        let results = benchmark.run_all(&config);

        AstrometryBenchmark::print_summary(&results);

        // Basic assertions
        assert!(!results.is_empty());
        for result in &results {
            assert!(result.astrometry_stars > 0);
            assert!(result.detected_stars > 0);
        }
    }

    #[test]
    #[ignore] // Requires cached results
    fn test_benchmark_cached() {
        crate::testing::init_tracing();

        let benchmark = match AstrometryBenchmark::new() {
            Ok(b) => b,
            Err(e) => {
                eprintln!("Failed to create benchmark: {}", e);
                return;
            }
        };

        let solved = benchmark.cache().solved_rectangles();
        if solved.is_empty() {
            eprintln!("No cached rectangles, skipping test");
            return;
        }

        let config = StarDetectionConfig::default();
        let results = benchmark.run_all(&config);

        AstrometryBenchmark::print_summary(&results);
    }
}
