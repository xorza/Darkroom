//! Benchmark runner for star detection against survey data.
//!
//! Orchestrates image download, catalog queries, detection, and metrics computation.

use super::catalog::{CatalogClient, CatalogStar};
use super::fields::TestField;
use super::image_fetch::ImageFetcher;
use super::wcs::WCS;
use crate::astro_image::AstroImage;
use crate::star_detection::visual_tests::output::{
    DetectionMetrics, compute_detection_metrics, save_comparison_png, save_grayscale_png,
    save_metrics,
};
use crate::star_detection::{Star, StarDetectionConfig, find_stars};
use crate::testing::synthetic::GroundTruthStar;
use anyhow::{Context, Result};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::Instant;

/// Detection statistics for a magnitude bin.
#[derive(Debug, Clone, Default)]
pub struct MagnitudeBinStats {
    /// Magnitude bin center
    pub mag_center: f32,
    /// Number of catalog stars in this bin
    pub catalog_count: usize,
    /// Number of detected (matched) stars
    pub detected_count: usize,
    /// Detection rate for this bin
    pub detection_rate: f32,
    /// Mean centroid error for matched stars (pixels)
    pub mean_centroid_error: f32,
}

/// Extended benchmark result with detailed analysis.
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
    /// Detection rate by magnitude bin
    pub magnitude_bins: Vec<MagnitudeBinStats>,
    /// Astrometric residuals (detected - catalog) in pixels
    pub astrometric_residuals: AstrometricResiduals,
}

/// Astrometric residual statistics.
#[derive(Debug, Clone, Default)]
pub struct AstrometricResiduals {
    /// Mean residual in X (pixels)
    pub mean_dx: f32,
    /// Mean residual in Y (pixels)
    pub mean_dy: f32,
    /// RMS residual in X
    pub rms_dx: f32,
    /// RMS residual in Y
    pub rms_dy: f32,
    /// Total RMS residual (sqrt(dx² + dy²))
    pub rms_total: f32,
    /// Number of matched stars used
    pub n_matched: usize,
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
            "  Completeness: {:.1}% ({} of {} catalog stars detected)",
            self.metrics.detection_rate * 100.0,
            self.metrics.true_positives,
            self.catalog_stars
        );
        println!(
            "  Mean centroid error: {:.3} pixels",
            self.metrics.mean_centroid_error
        );
        // Note: "false positives" include real stars fainter than catalog limit
        let extra_detections = self.detected_stars.saturating_sub(self.catalog_stars);
        if extra_detections > 0 {
            println!(
                "  Extra detections: {} (likely fainter than catalog mag limit)",
                extra_detections
            );
        }
        println!("  Runtime: {} ms", self.runtime_ms);
    }

    /// Print detailed analysis including magnitude bins and astrometry.
    pub fn print_detailed(&self) {
        self.print_summary();

        // Magnitude-binned detection rates
        if !self.magnitude_bins.is_empty() {
            println!("\n  Detection by magnitude:");
            println!("    Mag    | Count | Detected | Rate   | Centroid Err");
            println!("    -------|-------|----------|--------|-------------");
            for bin in &self.magnitude_bins {
                if bin.catalog_count > 0 {
                    println!(
                        "    {:5.1}  | {:5} | {:8} | {:5.1}% | {:.3} px",
                        bin.mag_center,
                        bin.catalog_count,
                        bin.detected_count,
                        bin.detection_rate * 100.0,
                        bin.mean_centroid_error
                    );
                }
            }
        }

        // Astrometric residuals
        let res = &self.astrometric_residuals;
        if res.n_matched > 0 {
            println!("\n  Astrometric residuals ({} stars):", res.n_matched);
            println!(
                "    Mean:  dX = {:+.3} px, dY = {:+.3} px",
                res.mean_dx, res.mean_dy
            );
            println!(
                "    RMS:   dX = {:.3} px, dY = {:.3} px, Total = {:.3} px",
                res.rms_dx, res.rms_dy, res.rms_total
            );
        }
    }

    /// Export results to JSON format.
    pub fn to_json(&self) -> String {
        let mag_bins: Vec<_> = self
            .magnitude_bins
            .iter()
            .filter(|b| b.catalog_count > 0)
            .map(|b| {
                format!(
                    r#"{{"mag":{:.1},"catalog":{},"detected":{},"rate":{:.4},"centroid_err":{:.4}}}"#,
                    b.mag_center, b.catalog_count, b.detected_count, b.detection_rate, b.mean_centroid_error
                )
            })
            .collect();

        format!(
            r#"{{"field":"{}","image_width":{},"image_height":{},"catalog_stars":{},"detected_stars":{},"detection_rate":{:.4},"precision":{:.4},"f1_score":{:.4},"mean_centroid_error":{:.4},"median_centroid_error":{:.4},"runtime_ms":{},"astrometry":{{"mean_dx":{:.4},"mean_dy":{:.4},"rms_dx":{:.4},"rms_dy":{:.4},"rms_total":{:.4},"n_matched":{}}},"magnitude_bins":[{}]}}"#,
            self.field_name,
            self.image_width,
            self.image_height,
            self.catalog_stars,
            self.detected_stars,
            self.metrics.detection_rate,
            self.metrics.precision,
            self.metrics.f1_score,
            self.metrics.mean_centroid_error,
            self.metrics.median_centroid_error,
            self.runtime_ms,
            self.astrometric_residuals.mean_dx,
            self.astrometric_residuals.mean_dy,
            self.astrometric_residuals.rms_dx,
            self.astrometric_residuals.rms_dy,
            self.astrometric_residuals.rms_total,
            self.astrometric_residuals.n_matched,
            mag_bins.join(",")
        )
    }

    /// Export results to CSV row format.
    #[allow(dead_code)]
    pub fn to_csv_row(&self) -> String {
        format!(
            "{},{},{},{},{},{:.4},{:.4},{:.4},{:.4},{:.4},{},{:.4},{:.4},{:.4},{:.4},{:.4},{}",
            self.field_name,
            self.image_width,
            self.image_height,
            self.catalog_stars,
            self.detected_stars,
            self.metrics.detection_rate,
            self.metrics.precision,
            self.metrics.f1_score,
            self.metrics.mean_centroid_error,
            self.metrics.median_centroid_error,
            self.runtime_ms,
            self.astrometric_residuals.mean_dx,
            self.astrometric_residuals.mean_dy,
            self.astrometric_residuals.rms_dx,
            self.astrometric_residuals.rms_dy,
            self.astrometric_residuals.rms_total,
            self.astrometric_residuals.n_matched,
        )
    }

    /// CSV header for the row format.
    #[allow(dead_code)]
    pub fn csv_header() -> &'static str {
        "field,image_width,image_height,catalog_stars,detected_stars,detection_rate,precision,f1_score,mean_centroid_error,median_centroid_error,runtime_ms,mean_dx,mean_dy,rms_dx,rms_dy,rms_total,n_matched"
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

        let width = image.width();
        let height = image.height();

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
        let ground_truth_with_mag =
            catalog_to_ground_truth_with_mag(&catalog_stars, &wcs, width, height, field);
        let ground_truth: Vec<GroundTruthStar> = ground_truth_with_mag
            .iter()
            .map(|(gt, _)| gt.clone())
            .collect();

        tracing::info!("{} catalog stars within image bounds", ground_truth.len());

        // Step 5: Run star detection
        let start = Instant::now();
        let result = find_stars(&image, config);
        let runtime_ms = start.elapsed().as_millis() as u64;

        tracing::info!("Detected {} stars in {} ms", result.stars.len(), runtime_ms);

        // Step 6: Compute metrics
        let match_radius = config.expected_fwhm * 2.0; // 2 FWHM matching radius
        let metrics = compute_detection_metrics(&ground_truth, &result.stars, match_radius);

        // Step 7: Compute magnitude-binned statistics
        let magnitude_bins = compute_magnitude_bins(
            &ground_truth_with_mag,
            &result.stars,
            &metrics,
            match_radius,
        );

        // Step 8: Compute astrometric residuals
        let astrometric_residuals =
            compute_astrometric_residuals(&ground_truth, &result.stars, &metrics);

        // Step 9: Save output images and detailed results
        self.save_outputs(field, &image, &ground_truth, &result.stars, &metrics)?;

        let benchmark_result = BenchmarkResult {
            field_name: field.name.to_string(),
            metrics,
            catalog_stars: ground_truth.len(),
            detected_stars: result.stars.len(),
            runtime_ms,
            image_width: width,
            image_height: height,
            magnitude_bins,
            astrometric_residuals,
        };

        // Save JSON output
        self.save_json_output(field, &benchmark_result)?;

        Ok(benchmark_result)
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

        let width = image.width();
        let height = image.height();

        // Normalize image to 0-1 range for display
        let pixels_normalized = normalize_pixels(image.pixels());

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

    /// Save JSON output for a benchmark result.
    fn save_json_output(&self, field: &TestField, result: &BenchmarkResult) -> Result<()> {
        let field_dir = self.output_dir.join(field.name);
        std::fs::create_dir_all(&field_dir)?;

        let json_path = field_dir.join("results.json");
        let mut file = std::fs::File::create(&json_path)?;
        writeln!(file, "{}", result.to_json())?;

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

/// Convert catalog stars to ground truth in pixel coordinates, preserving magnitude.
fn catalog_to_ground_truth_with_mag(
    catalog: &[CatalogStar],
    wcs: &WCS,
    width: usize,
    height: usize,
    field: &TestField,
) -> Vec<(GroundTruthStar, f32)> {
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

            let gt = GroundTruthStar {
                x: x as f32,
                y: y as f32,
                flux,
                fwhm,
                eccentricity: 0.0,
                is_saturated: star.mag < 14.0, // Rough saturation estimate
                angle: 0.0,
            };

            Some((gt, star.mag))
        })
        .collect()
}

/// Compute magnitude-binned detection statistics.
fn compute_magnitude_bins(
    ground_truth_with_mag: &[(GroundTruthStar, f32)],
    detected: &[Star],
    metrics: &DetectionMetrics,
    match_radius: f32,
) -> Vec<MagnitudeBinStats> {
    // Define magnitude bins (typically 1 mag wide)
    let mag_min = ground_truth_with_mag
        .iter()
        .map(|(_, m)| *m)
        .fold(f32::INFINITY, f32::min);
    let mag_max = ground_truth_with_mag
        .iter()
        .map(|(_, m)| *m)
        .fold(f32::NEG_INFINITY, f32::max);

    if mag_min > mag_max {
        return Vec::new();
    }

    let bin_width = 1.0;
    let bin_start = (mag_min / bin_width).floor() * bin_width;
    let bin_end = (mag_max / bin_width).ceil() * bin_width;

    let mut bins: Vec<MagnitudeBinStats> = Vec::new();
    let mut mag = bin_start;

    while mag < bin_end {
        let mag_center = mag + bin_width / 2.0;

        // Count catalog stars in this bin
        let stars_in_bin: Vec<_> = ground_truth_with_mag
            .iter()
            .enumerate()
            .filter(|(_, (_, m))| *m >= mag && *m < mag + bin_width)
            .collect();

        let catalog_count = stars_in_bin.len();

        // Count detected stars in this bin (using match result if available)
        let mut detected_count = 0;
        let mut centroid_errors = Vec::new();

        if let Some(ref match_result) = metrics.match_result {
            for (idx, (_gt, _mag)) in stars_in_bin {
                // Check if this ground truth star was matched
                if match_result.matched_truth.contains(&idx) {
                    detected_count += 1;

                    // Find the corresponding detected star and compute error
                    for &(ti, di, dist) in &match_result.pairs {
                        if ti == idx {
                            let _ = di; // Suppress unused warning
                            centroid_errors.push(dist);
                            break;
                        }
                    }
                }
            }
        } else {
            // Fallback: manual matching for this bin
            let match_radius_sq = match_radius * match_radius;
            for (_, (gt, _)) in &stars_in_bin {
                for det in detected {
                    let dx = det.x - gt.x;
                    let dy = det.y - gt.y;
                    if dx * dx + dy * dy < match_radius_sq {
                        detected_count += 1;
                        centroid_errors.push((dx * dx + dy * dy).sqrt());
                        break;
                    }
                }
            }
        }

        let detection_rate = if catalog_count > 0 {
            detected_count as f32 / catalog_count as f32
        } else {
            0.0
        };

        let mean_centroid_error = if !centroid_errors.is_empty() {
            centroid_errors.iter().sum::<f32>() / centroid_errors.len() as f32
        } else {
            0.0
        };

        bins.push(MagnitudeBinStats {
            mag_center,
            catalog_count,
            detected_count,
            detection_rate,
            mean_centroid_error,
        });

        mag += bin_width;
    }

    bins
}

/// Compute astrometric residuals from matched stars.
fn compute_astrometric_residuals(
    ground_truth: &[GroundTruthStar],
    detected: &[Star],
    metrics: &DetectionMetrics,
) -> AstrometricResiduals {
    let Some(ref match_result) = metrics.match_result else {
        return AstrometricResiduals::default();
    };

    if match_result.pairs.is_empty() {
        return AstrometricResiduals::default();
    }

    let mut dx_values = Vec::with_capacity(match_result.pairs.len());
    let mut dy_values = Vec::with_capacity(match_result.pairs.len());

    for &(ti, di, _) in &match_result.pairs {
        let gt = &ground_truth[ti];
        let det = &detected[di];

        let dx = det.x - gt.x;
        let dy = det.y - gt.y;

        dx_values.push(dx);
        dy_values.push(dy);
    }

    let n = dx_values.len() as f32;
    let mean_dx = dx_values.iter().sum::<f32>() / n;
    let mean_dy = dy_values.iter().sum::<f32>() / n;

    let rms_dx = (dx_values.iter().map(|x| x * x).sum::<f32>() / n).sqrt();
    let rms_dy = (dy_values.iter().map(|y| y * y).sum::<f32>() / n).sqrt();
    let rms_total = (rms_dx * rms_dx + rms_dy * rms_dy).sqrt();

    AstrometricResiduals {
        mean_dx,
        mean_dy,
        rms_dx,
        rms_dy,
        rms_total,
        n_matched: dx_values.len(),
    }
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
    #[ignore] // Benchmark test - run with --ignored
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
                r.print_detailed();

                // Detection rate depends on catalog mag limit vs actual detection limit
                // 10% is a reasonable lower bound for sparse fields
                assert!(r.metrics.detection_rate > 0.10);

                // Centroid accuracy should be sub-pixel
                assert!(
                    r.metrics.mean_centroid_error < 1.0,
                    "Centroid error too high: {:.3}",
                    r.metrics.mean_centroid_error
                );

                // Astrometric residuals should show no systematic offset
                assert!(
                    r.astrometric_residuals.mean_dx.abs() < 0.5,
                    "Systematic X offset: {:.3}",
                    r.astrometric_residuals.mean_dx
                );
                assert!(
                    r.astrometric_residuals.mean_dy.abs() < 0.5,
                    "Systematic Y offset: {:.3}",
                    r.astrometric_residuals.mean_dy
                );
            }
            Err(e) => {
                println!("Benchmark failed: {}", e);
            }
        }
    }

    #[test]
    #[ignore] // Requires network
    fn test_benchmark_detailed_analysis() {
        crate::testing::init_tracing();

        let benchmark = SurveyBenchmark::new().unwrap();

        println!("\n=== Detailed Benchmark Analysis ===\n");

        for field in super::super::fields::sdss_fields().into_iter().take(3) {
            println!("Testing field: {}", field.name);

            let config = StarDetectionConfig {
                expected_fwhm: field.expected_fwhm_pixels(0.396),
                detection_sigma: 3.0,
                ..Default::default()
            };

            match benchmark.run_field(&field, &config) {
                Ok(result) => {
                    result.print_detailed();

                    // Validate magnitude-dependent detection
                    let bright_bins: Vec<_> = result
                        .magnitude_bins
                        .iter()
                        .filter(|b| b.mag_center < 18.0 && b.catalog_count > 0)
                        .collect();

                    if !bright_bins.is_empty() {
                        let bright_detection_rate: f32 = bright_bins
                            .iter()
                            .map(|b| b.detection_rate * b.catalog_count as f32)
                            .sum::<f32>()
                            / bright_bins
                                .iter()
                                .map(|b| b.catalog_count as f32)
                                .sum::<f32>();

                        println!(
                            "  Bright stars (mag<18) detection rate: {:.1}%",
                            bright_detection_rate * 100.0
                        );

                        // Bright stars should have higher detection rate
                        assert!(
                            bright_detection_rate > result.metrics.detection_rate,
                            "Bright stars should be detected more often"
                        );
                    }

                    // Validate JSON output
                    let json = result.to_json();
                    assert!(json.contains(&result.field_name));
                    assert!(json.contains("detection_rate"));
                    assert!(json.contains("magnitude_bins"));

                    println!();
                }
                Err(e) => {
                    println!("  FAILED: {}\n", e);
                }
            }
        }
    }

    #[test]
    #[ignore] // Requires network
    fn test_sensitivity_to_detection_threshold() {
        crate::testing::init_tracing();

        let benchmark = SurveyBenchmark::new().unwrap();
        let field = super::super::fields::sparse_field();

        println!("\n=== Detection Threshold Sensitivity ===\n");
        println!("Sigma | Detection | Precision | Centroid Err | Stars");
        println!("------|-----------|-----------|--------------|------");

        for sigma in [2.0, 3.0, 4.0, 5.0, 7.0, 10.0] {
            let config = StarDetectionConfig {
                expected_fwhm: field.expected_fwhm_pixels(0.396),
                detection_sigma: sigma,
                ..Default::default()
            };

            match benchmark.run_field(&field, &config) {
                Ok(r) => {
                    println!(
                        "{:5.1} | {:8.1}% | {:8.1}% | {:11.3} | {}",
                        sigma,
                        r.metrics.detection_rate * 100.0,
                        r.metrics.precision * 100.0,
                        r.metrics.mean_centroid_error,
                        r.detected_stars
                    );
                }
                Err(e) => println!("{:5.1} | FAILED: {}", sigma, e),
            }
        }

        println!();
    }

    #[test]
    #[ignore] // Benchmark test - run with --ignored
    fn test_magnitude_bin_computation() {
        // Test with synthetic data
        let ground_truth_with_mag = vec![
            (
                GroundTruthStar {
                    x: 100.0,
                    y: 100.0,
                    flux: 1000.0,
                    fwhm: 3.0,
                    eccentricity: 0.0,
                    is_saturated: false,
                    angle: 0.0,
                },
                15.0,
            ), // Bright
            (
                GroundTruthStar {
                    x: 200.0,
                    y: 200.0,
                    flux: 100.0,
                    fwhm: 3.0,
                    eccentricity: 0.0,
                    is_saturated: false,
                    angle: 0.0,
                },
                18.0,
            ), // Medium
            (
                GroundTruthStar {
                    x: 300.0,
                    y: 300.0,
                    flux: 10.0,
                    fwhm: 3.0,
                    eccentricity: 0.0,
                    is_saturated: false,
                    angle: 0.0,
                },
                20.0,
            ), // Faint
        ];

        let detected = vec![
            Star {
                x: 100.1,
                y: 100.1,
                flux: 1000.0,
                fwhm: 3.0,
                eccentricity: 0.0,
                snr: 100.0,
                peak: 0.5,
                sharpness: 0.3,
                roundness1: 0.0,
                roundness2: 0.0,
                laplacian_snr: 0.0,
            }, // Matches bright
        ];

        let ground_truth: Vec<_> = ground_truth_with_mag
            .iter()
            .map(|(gt, _)| gt.clone())
            .collect();
        let metrics = compute_detection_metrics(&ground_truth, &detected, 5.0);

        let bins = compute_magnitude_bins(&ground_truth_with_mag, &detected, &metrics, 5.0);

        // Should have bins covering mag 15-20
        assert!(!bins.is_empty());

        // Find the bin containing mag 15
        let bright_bin = bins
            .iter()
            .find(|b| b.mag_center >= 15.0 && b.mag_center < 16.0);
        assert!(bright_bin.is_some());
        let bright_bin = bright_bin.unwrap();
        assert_eq!(bright_bin.catalog_count, 1);
        assert_eq!(bright_bin.detected_count, 1);
        assert!((bright_bin.detection_rate - 1.0).abs() < 0.01);
    }

    #[test]
    #[ignore] // Benchmark test - run with --ignored
    fn test_astrometric_residuals_computation() {
        let ground_truth = vec![
            GroundTruthStar {
                x: 100.0,
                y: 100.0,
                flux: 1000.0,
                fwhm: 3.0,
                eccentricity: 0.0,
                is_saturated: false,
                angle: 0.0,
            },
            GroundTruthStar {
                x: 200.0,
                y: 200.0,
                flux: 1000.0,
                fwhm: 3.0,
                eccentricity: 0.0,
                is_saturated: false,
                angle: 0.0,
            },
        ];

        // Detected with small systematic offset
        let detected = vec![
            Star {
                x: 100.1,
                y: 100.2,
                flux: 1000.0,
                fwhm: 3.0,
                eccentricity: 0.0,
                snr: 100.0,
                peak: 0.5,
                sharpness: 0.3,
                roundness1: 0.0,
                roundness2: 0.0,
                laplacian_snr: 0.0,
            },
            Star {
                x: 200.1,
                y: 200.2,
                flux: 1000.0,
                fwhm: 3.0,
                eccentricity: 0.0,
                snr: 100.0,
                peak: 0.5,
                sharpness: 0.3,
                roundness1: 0.0,
                roundness2: 0.0,
                laplacian_snr: 0.0,
            },
        ];

        let metrics = compute_detection_metrics(&ground_truth, &detected, 5.0);
        let residuals = compute_astrometric_residuals(&ground_truth, &detected, &metrics);

        assert_eq!(residuals.n_matched, 2);
        assert!((residuals.mean_dx - 0.1).abs() < 0.01);
        assert!((residuals.mean_dy - 0.2).abs() < 0.01);
        assert!(residuals.rms_total > 0.0);
    }

    #[test]
    #[ignore] // Requires network
    fn debug_bright_star_matching() {
        crate::testing::init_tracing();

        let benchmark = SurveyBenchmark::new().unwrap();
        let field = super::super::fields::sparse_field();

        let sdss = field.sdss.as_ref().unwrap();
        let image_path = benchmark
            .fetcher
            .fetch_sdss(sdss.run, sdss.camcol, sdss.field, 'r', sdss.rerun)
            .unwrap();

        let (image, wcs) = benchmark.load_image_with_wcs(&image_path).unwrap();
        let width = image.width();
        let height = image.height();

        println!("\n=== WCS Info ===");
        println!(
            "CRPIX: ({:.1}, {:.1}), CRVAL: ({:.6}, {:.6})",
            wcs.crpix1, wcs.crpix2, wcs.crval1, wcs.crval2
        );
        println!(
            "CD matrix: [{:.2e}, {:.2e}; {:.2e}, {:.2e}]",
            wcs.cd1_1, wcs.cd1_2, wcs.cd2_1, wcs.cd2_2
        );
        println!("Pixel scale: {:.3} arcsec/pix", wcs.pixel_scale_arcsec());

        let bounds = wcs.image_bounds_sky(width, height);
        println!(
            "Image bounds: RA [{:.4}, {:.4}], Dec [{:.4}, {:.4}]",
            bounds.ra_min, bounds.ra_max, bounds.dec_min, bounds.dec_max
        );

        let catalog_stars = benchmark
            .catalog
            .query_box(field.source, &bounds, field.mag_limit)
            .unwrap();

        println!("\n=== First few catalog stars (raw) ===");
        for star in catalog_stars.iter().take(5) {
            let (px, py) = wcs.sky_to_pixel(star.ra, star.dec);
            println!(
                "RA={:.6}, Dec={:.6}, mag={:.1} -> pixel ({:.1}, {:.1})",
                star.ra, star.dec, star.mag, px, py
            );
        }

        let ground_truth_with_mag =
            catalog_to_ground_truth_with_mag(&catalog_stars, &wcs, width, height, &field);

        let config = StarDetectionConfig {
            expected_fwhm: field.expected_fwhm_pixels(0.396),
            ..Default::default()
        };
        let result = find_stars(&image, &config);
        let match_radius = config.expected_fwhm * 2.0;

        println!("\n=== Bright Catalog Stars (mag < 18) ===");
        println!("Match radius: {:.1} pixels", match_radius);

        let bright_stars: Vec<_> = ground_truth_with_mag
            .iter()
            .filter(|(_, m)| *m < 18.0)
            .collect();

        for (gt, mag) in bright_stars.iter().take(10) {
            // Find nearest detected star
            let mut nearest_dist = f32::MAX;
            let mut nearest_det: Option<&Star> = None;
            for det in &result.stars {
                let dx = det.x - gt.x;
                let dy = det.y - gt.y;
                let dist = (dx * dx + dy * dy).sqrt();
                if dist < nearest_dist {
                    nearest_dist = dist;
                    nearest_det = Some(det);
                }
            }

            print!("Catalog: ({:7.1}, {:7.1}) mag={:.1}", gt.x, gt.y, mag);

            if let Some(det) = nearest_det {
                let matched = nearest_dist < match_radius;
                println!(
                    " -> nearest: ({:7.1}, {:7.1}) dist={:5.1}px {}",
                    det.x,
                    det.y,
                    nearest_dist,
                    if matched { "MATCH" } else { "NO MATCH" }
                );
            } else {
                println!(" -> no detections found");
            }

            // Check pixel value at catalog position
            let px = gt.x as usize;
            let py = gt.y as usize;
            if px < width && py < height {
                let idx = py * width + px;
                println!(
                    "    Pixel value: {:.1}, saturated flag: {}",
                    image.pixels()[idx],
                    gt.is_saturated
                );
            }
        }

        println!("\nTotal bright stars in catalog: {}", bright_stars.len());
        println!("Total detections: {}", result.stars.len());

        // Compute systematic offset
        let mut dx_sum = 0.0f32;
        let mut dy_sum = 0.0f32;
        let mut count = 0;

        for (gt, _) in &bright_stars {
            let mut nearest_dist = f32::MAX;
            let mut nearest_dx = 0.0f32;
            let mut nearest_dy = 0.0f32;
            for det in &result.stars {
                let dx = det.x - gt.x;
                let dy = det.y - gt.y;
                let dist = (dx * dx + dy * dy).sqrt();
                if dist < nearest_dist {
                    nearest_dist = dist;
                    nearest_dx = dx;
                    nearest_dy = dy;
                }
            }
            if nearest_dist < 100.0 {
                dx_sum += nearest_dx;
                dy_sum += nearest_dy;
                count += 1;
            }
        }

        if count > 0 {
            println!(
                "\nSystematic offset (mean of {} stars): dX = {:.1}, dY = {:.1}",
                count,
                dx_sum / count as f32,
                dy_sum / count as f32
            );
        }
    }
}
