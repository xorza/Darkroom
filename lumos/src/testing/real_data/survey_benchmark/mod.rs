//! Survey benchmark module for star detection validation.
//!
//! This module provides tools for benchmarking star detection algorithms
//! against real astronomical survey data with known ground truth from
//! professional catalogs.
//!
//! # Supported Surveys
//!
//! - **SDSS** (Sloan Digital Sky Survey): Wide-field imaging survey covering
//!   ~14,000 square degrees of the northern sky.
//! - **Pan-STARRS**: All-sky survey with deep imaging and accurate astrometry.
//! - **Gaia DR3**: ESA mission with the most accurate stellar positions available.
//!
//! # Interpreting Results
//!
//! ## Key Metrics
//!
//! - **Completeness (detection rate)**: Fraction of catalog stars detected.
//!   Expect 10-50% for magnitude-limited catalogs since many catalog stars
//!   are fainter than the detector can reliably find.
//!
//! - **Centroid accuracy**: RMS error between detected and catalog positions.
//!   Sub-pixel accuracy (< 0.5 px) indicates correct detection and centroiding.
//!   This is the primary validation metric.
//!
//! - **Astrometric residuals**: Mean and RMS position offsets. Small mean offsets
//!   (< 0.5 px) indicate correct WCS. Large systematic offsets suggest coordinate
//!   system issues.
//!
//! ## Known Limitations
//!
//! - **SDSS catalog coverage**: The SDSS PhotoObj catalog is merged from multiple
//!   observations. Some catalog objects may not appear in the specific frame being
//!   tested, causing apparent "missed detections" for bright objects. Faint stars
//!   near the detection limit typically match well.
//!
//! - **Extra detections**: The detector may find more stars than the catalog
//!   contains. These are usually real stars fainter than the catalog magnitude
//!   limit, not false positives.
//!
//! - **FWHM and flux errors**: Catalog ground truth uses estimated values
//!   (field typical FWHM, flux from magnitude with assumed zeropoint). High
//!   errors in these metrics are expected and not indicative of algorithm issues.
//!
//! # Usage
//!
//! ```rust,ignore
//! use lumos::star_detection::survey_benchmark::{SurveyBenchmark, sparse_field};
//! use lumos::star_detection::StarDetectionConfig;
//!
//! let benchmark = SurveyBenchmark::new()?;
//! let field = sparse_field();
//! let config = StarDetectionConfig::default();
//!
//! let result = benchmark.run_field(&field, &config)?;
//! result.print_summary();
//! ```
//!
//! # Test Fields
//!
//! The module includes pre-defined test fields with varying characteristics:
//!
//! - `sparse_field()`: Well-separated stars for basic validation
//! - `medium_field()`: Typical extragalactic field density
//! - `dense_field()`: Crowded galactic field for deblending tests
//! - `faint_field()`: Stars near detection limit
//! - `cluster_m67()`: Well-studied open cluster
//! - `standard_sa95()`: Photometric standard field

pub mod benchmark;
pub mod catalog;
pub mod fields;
pub mod image_fetch;
pub mod wcs;

// Re-exports for public API
#[allow(unused_imports)]
pub use benchmark::{BenchmarkResult, SurveyBenchmark};
#[allow(unused_imports)]
pub use catalog::{CatalogClient, CatalogSource, CatalogStar};
#[allow(unused_imports)]
pub use fields::{
    Difficulty, TestField, all_test_fields, cluster_m67, dense_field, faint_field, medium_field,
    sparse_field, standard_sa95,
};
#[allow(unused_imports)]
pub use image_fetch::ImageFetcher;
#[allow(unused_imports)]
pub use wcs::{SkyBounds, WCS};

#[cfg(test)]
mod tests {
    use super::*;
    use crate::star_detection::{BackgroundConfig, StarDetectionConfig};
    use crate::testing::init_tracing;

    #[test]
    #[ignore] // Requires network
    fn test_full_benchmark_sparse() {
        init_tracing();

        let benchmark = SurveyBenchmark::new().expect("Failed to create benchmark");
        let field = sparse_field();

        let config = StarDetectionConfig {
            expected_fwhm: field.expected_fwhm_pixels(0.396),
            min_snr: 20.0, // Only detect brighter stars
            background_config: BackgroundConfig {
                sigma_threshold: 5.0, // Higher threshold to match catalog bright stars
                ..Default::default()
            },
            ..Default::default()
        };

        match benchmark.run_field(&field, &config) {
            Ok(result) => {
                println!("\n=== Benchmark Results ===");
                result.print_summary();

                // Detection rate measures what fraction of catalog stars we found
                // Mean centroid error is the key quality metric
                assert!(
                    result.metrics.mean_centroid_error < 1.0,
                    "Centroid error too high: {:.3}px",
                    result.metrics.mean_centroid_error
                );
            }
            Err(e) => {
                println!("Benchmark failed (may be network issue): {}", e);
            }
        }
    }

    #[test]
    #[ignore] // Requires network
    fn test_benchmark_all_sdss_fields() {
        init_tracing();

        let benchmark = SurveyBenchmark::new().expect("Failed to create benchmark");

        println!("\n=== Running All SDSS Field Benchmarks ===\n");

        for field in fields::sdss_fields() {
            println!("Testing field: {} - {}", field.name, field.description);

            let config = StarDetectionConfig {
                expected_fwhm: field.expected_fwhm_pixels(0.396),
                background_config: BackgroundConfig {
                    sigma_threshold: 3.0,
                    ..Default::default()
                },
                ..Default::default()
            };

            match benchmark.run_field(&field, &config) {
                Ok(result) => {
                    println!(
                        "  Detection: {:.1}% ({}/{}), Centroid: {:.3}px, Time: {}ms",
                        result.metrics.detection_rate * 100.0,
                        result.metrics.true_positives,
                        result.catalog_stars,
                        result.metrics.mean_centroid_error,
                        result.runtime_ms
                    );
                }
                Err(e) => {
                    println!("  FAILED: {}", e);
                }
            }
            println!();
        }
    }

    #[test]
    #[ignore] // Requires network
    fn test_catalog_queries() {
        init_tracing();

        let client = CatalogClient::new();

        // Test SDSS query
        println!("Testing SDSS catalog query...");
        match client.query_region(CatalogSource::Sdss, 180.0, 45.0, 0.1, 20.0) {
            Ok(stars) => println!("  Found {} SDSS stars", stars.len()),
            Err(e) => println!("  SDSS query failed: {}", e),
        }

        // Test Pan-STARRS query
        println!("Testing Pan-STARRS catalog query...");
        match client.query_region(CatalogSource::PanStarrs, 180.0, 45.0, 0.1, 20.0) {
            Ok(stars) => println!("  Found {} Pan-STARRS stars", stars.len()),
            Err(e) => println!("  Pan-STARRS query failed: {}", e),
        }

        // Test Gaia query
        println!("Testing Gaia DR3 catalog query...");
        match client.query_region(CatalogSource::GaiaDr3, 180.0, 45.0, 0.1, 18.0) {
            Ok(stars) => println!("  Found {} Gaia stars", stars.len()),
            Err(e) => println!("  Gaia query failed: {}", e),
        }
    }

    #[test]
    #[ignore] // Requires network
    fn test_image_download() {
        init_tracing();

        let fetcher = ImageFetcher::new().expect("Failed to create fetcher");

        // Test SDSS download
        println!("Testing SDSS image download...");
        match fetcher.fetch_sdss(2505, 3, 150, 'r', 301) {
            Ok(path) => println!("  Downloaded to: {}", path.display()),
            Err(e) => println!("  Download failed: {}", e),
        }
    }
}
