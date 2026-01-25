//! Benchmark runner for star detection against survey data.
//!
//! Orchestrates image download, catalog queries, detection, and metrics computation.

use super::catalog::{CatalogClient, CatalogStar};
use super::fields::TestField;
use super::image_fetch::ImageFetcher;
use super::wcs::WCS;
use crate::astro_image::AstroImage;
use crate::star_detection::visual_tests::generators::GroundTruthStar;
use crate::star_detection::visual_tests::output::{
    DetectionMetrics, compute_detection_metrics, save_comparison_png, save_grayscale_png,
    save_metrics,
};
use crate::star_detection::{Star, StarDetectionConfig, find_stars};
use anyhow::{Context, Result};
use std::path::{Path, PathBuf};
use std::time::Instant;

/// Result of benchmarking a single field.
#[derive(Debug)]
pub struct BenchmarkResult {
    /// The test field
    pub field_name: String,
    /// Detection metrics
    pub metrics: DetectionMetrics,
    /// Number of catalog stars in the field
    pub catalog_stars: usize,
    /// Number of detected stars
    pub detected_stars: usize,
    /// Detection runtime in milliseconds
    pub runtime_ms: u64,
    /// Image dimensions
    pub image_width: usize,
    pub image_height: usize,
}

impl BenchmarkResult {
    /// Print a summary of the benchmark result.
    pub fn print_summary(&self) {
        println!("Field: {}", self.field_name);
        println!("  Image: {}x{}", self.image_width, self.image_height);
        println!(
            "  Catalog stars: {}, Detected: {}",
            self.catalog_stars, self.detected_stars
        );
        println!(
            "  Detection rate: {:.1}%",
            self.metrics.detection_rate * 100.0
        );
        println!("  Precision: {:.1}%", self.metrics.precision * 100.0);
        println!(
            "  Mean centroid error: {:.3} pixels",
            self.metrics.mean_centroid_error
        );
        println!("  Runtime: {} ms", self.runtime_ms);
    }
}

/// Benchmark runner for survey data.
#[derive(Debug)]
pub struct SurveyBenchmark {
    fetcher: ImageFetcher,
    catalog: CatalogClient,
    output_dir: PathBuf,
}

impl SurveyBenchmark {
    /// Create a new benchmark runner.
    pub fn new() -> Result<Self> {
        let fetcher = ImageFetcher::new()?;
        let catalog = CatalogClient::new();
        let output_dir = common::test_utils::test_output_path("survey_benchmark");

        Ok(Self {
            fetcher,
            catalog,
            output_dir,
        })
    }

    /// Create a benchmark runner with custom output directory.
    #[allow(dead_code)]
    pub fn with_output_dir(output_dir: PathBuf) -> Result<Self> {
        let fetcher = ImageFetcher::new()?;
        let catalog = CatalogClient::new();

        std::fs::create_dir_all(&output_dir)?;

        Ok(Self {
            fetcher,
            catalog,
            output_dir,
        })
    }

    /// Run benchmark on a single test field.
    pub fn run_field(
        &self,
        field: &TestField,
        config: &StarDetectionConfig,
    ) -> Result<BenchmarkResult> {
        tracing::info!("Running benchmark on field: {}", field.name);

        // Step 1: Download image
        let image_path = self.download_image(field)?;

        // Step 2: Load image and extract WCS
        let (image, wcs) = self.load_image_with_wcs(&image_path)?;

        let width = image.dimensions.width;
        let height = image.dimensions.height;

        // Step 3: Query catalog for the image region
        let bounds = wcs.image_bounds_sky(width, height);
        let catalog_stars = self
            .catalog
            .query_box(field.source, &bounds, field.mag_limit)?;

        tracing::info!(
            "Found {} catalog stars in field bounds",
            catalog_stars.len()
        );

        // Step 4: Convert catalog to ground truth (pixel coordinates)
        let ground_truth = catalog_to_ground_truth(&catalog_stars, &wcs, width, height, field);

        tracing::info!("{} catalog stars within image bounds", ground_truth.len());

        // Step 5: Run star detection
        let start = Instant::now();
        let result = find_stars(&image.pixels, width, height, config);
        let runtime_ms = start.elapsed().as_millis() as u64;

        tracing::info!("Detected {} stars in {} ms", result.stars.len(), runtime_ms);

        // Step 6: Compute metrics
        let match_radius = config.expected_fwhm * 2.0; // 2 FWHM matching radius
        let metrics = compute_detection_metrics(&ground_truth, &result.stars, match_radius);

        // Step 7: Save output images
        self.save_outputs(field, &image, &ground_truth, &result.stars, &metrics)?;

        Ok(BenchmarkResult {
            field_name: field.name.to_string(),
            metrics,
            catalog_stars: ground_truth.len(),
            detected_stars: result.stars.len(),
            runtime_ms,
            image_width: width,
            image_height: height,
        })
    }

    /// Run benchmark on all test fields.
    #[allow(dead_code)]
    pub fn run_all(&self, config: &StarDetectionConfig) -> Vec<Result<BenchmarkResult>> {
        super::fields::all_test_fields()
            .iter()
            .map(|field| self.run_field(field, config))
            .collect()
    }

    /// Download the image for a test field.
    fn download_image(&self, field: &TestField) -> Result<PathBuf> {
        if let Some(sdss) = &field.sdss {
            self.fetcher
                .fetch_sdss(sdss.run, sdss.camcol, sdss.field, 'r', sdss.rerun)
        } else {
            // Fall back to Pan-STARRS cutout
            self.fetcher.fetch_panstarrs(field.ra, field.dec, 10.0, 'r')
        }
    }

    /// Load a FITS image and extract its WCS.
    fn load_image_with_wcs(&self, path: &Path) -> Result<(AstroImage, WCS)> {
        let image = AstroImage::from_file(path).context("Failed to load FITS image")?;

        // Extract WCS from FITS header
        let wcs = extract_wcs_from_fits(path).context("Failed to extract WCS from FITS")?;

        Ok((image, wcs))
    }

    /// Save benchmark output images and metrics.
    fn save_outputs(
        &self,
        field: &TestField,
        image: &AstroImage,
        ground_truth: &[GroundTruthStar],
        detected: &[Star],
        metrics: &DetectionMetrics,
    ) -> Result<()> {
        let field_dir = self.output_dir.join(field.name);
        std::fs::create_dir_all(&field_dir)?;

        let width = image.dimensions.width;
        let height = image.dimensions.height;

        // Normalize image to 0-1 range for display
        let pixels_normalized = normalize_pixels(&image.pixels);

        // Save input image
        save_grayscale_png(
            &pixels_normalized,
            width,
            height,
            &field_dir.join("input.png"),
        );

        // Save comparison image (use 2*FWHM as match radius)
        let match_radius = 8.0; // Typical 2*FWHM for survey images
        save_comparison_png(
            &pixels_normalized,
            width,
            height,
            ground_truth,
            detected,
            match_radius,
            &field_dir.join("comparison.png"),
        );

        // Save metrics
        save_metrics(metrics, &field_dir.join("metrics.txt"));

        Ok(())
    }
}

/// Extract WCS from a FITS file header.
fn extract_wcs_from_fits(path: &Path) -> Result<WCS> {
    use fitsio::FitsFile;

    let mut fptr = FitsFile::open(path).context("Failed to open FITS file")?;
    let hdu = fptr.primary_hdu().context("Failed to get primary HDU")?;

    // Helper to read keyword as f64
    let get_keyword = |key: &str| -> Option<f64> { hdu.read_key::<f64>(&mut fptr, key).ok() };

    WCS::from_header(get_keyword).context("Failed to parse WCS from header")
}

/// Convert catalog stars to ground truth in pixel coordinates.
fn catalog_to_ground_truth(
    catalog: &[CatalogStar],
    wcs: &WCS,
    width: usize,
    height: usize,
    field: &TestField,
) -> Vec<GroundTruthStar> {
    let margin = 10.0; // Edge margin in pixels

    catalog
        .iter()
        .filter_map(|star| {
            let (x, y) = wcs.sky_to_pixel(star.ra, star.dec);

            // Check if within image bounds (with margin)
            if x < margin || x >= width as f64 - margin || y < margin || y >= height as f64 - margin
            {
                return None;
            }

            // Estimate FWHM from plate scale
            let plate_scale = wcs.pixel_scale_arcsec() as f32;
            let fwhm = field.typical_fwhm_arcsec / plate_scale;

            // Estimate flux from magnitude (rough approximation)
            // Using zeropoint of 25 for typical SDSS images
            let flux = 10.0_f32.powf((25.0 - star.mag) / 2.5);

            Some(GroundTruthStar {
                x: x as f32,
                y: y as f32,
                flux,
                fwhm,
                eccentricity: 0.0,
                is_saturated: star.mag < 14.0, // Rough saturation estimate
                angle: 0.0,
            })
        })
        .collect()
}

/// Normalize pixel values to 0-1 range with asinh stretch.
fn normalize_pixels(pixels: &[f32]) -> Vec<f32> {
    if pixels.is_empty() {
        return Vec::new();
    }

    // Compute robust statistics
    let mut sorted: Vec<f32> = pixels.iter().copied().filter(|v| v.is_finite()).collect();
    if sorted.is_empty() {
        return vec![0.5; pixels.len()];
    }
    sorted.sort_by(|a, b| a.partial_cmp(b).unwrap());

    let low = sorted[sorted.len() / 100]; // 1st percentile
    let high = sorted[sorted.len() * 99 / 100]; // 99th percentile

    let range = (high - low).max(1.0);

    // Apply asinh stretch for better dynamic range display
    pixels
        .iter()
        .map(|&v| {
            let normalized = (v - low) / range;
            let stretched = (normalized * 10.0).asinh() / 10.0_f32.asinh();
            stretched.clamp(0.0, 1.0)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_normalize_pixels() {
        let pixels = vec![0.0, 100.0, 200.0, 300.0, 400.0];
        let normalized = normalize_pixels(&pixels);

        assert!(normalized.iter().all(|&v| (0.0..=1.0).contains(&v)));
    }

    #[test]
    #[ignore] // Requires network
    fn test_benchmark_sparse_field() {
        let benchmark = SurveyBenchmark::new().unwrap();
        let field = super::super::fields::sparse_field();

        let config = StarDetectionConfig {
            expected_fwhm: field.expected_fwhm_pixels(0.396), // SDSS plate scale
            ..Default::default()
        };

        let result = benchmark.run_field(&field, &config);

        match result {
            Ok(r) => {
                r.print_summary();
                assert!(r.metrics.detection_rate > 0.5);
            }
            Err(e) => {
                println!("Benchmark failed: {}", e);
            }
        }
    }
}
