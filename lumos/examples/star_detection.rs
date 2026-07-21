//! Example: Star Detection
//!
//! This example demonstrates star detection workflows:
//! 1. Detect stars with default configuration
//! 2. Use builder pattern for custom configurations
//! 3. Interpret star properties and diagnostics
//!
//! # Usage
//!
//! ```bash
//! cargo run --release --example star_detection -- /path/to/image.fits
//! ```

use std::env;
use std::path::Path;

use lumos::{LinearImage, LoadContext, StarDetectionConfig, StarDetector};

fn main() {
    // Get image path from command line
    let args: Vec<String> = env::args().collect();
    if args.len() < 2 {
        eprintln!("Usage: {} <image_path>", args[0]);
        eprintln!("Supported formats: linear FITS and floating-point TIFF");
        std::process::exit(1);
    }
    let image_path = Path::new(&args[1]);

    // Load the image
    println!("Loading image: {}", image_path.display());
    let image = LinearImage::from_file(image_path, &LoadContext::default())
        .expect("Failed to load image");
    println!(
        "Image dimensions: {}x{} ({} channel{})",
        image.width(),
        image.height(),
        image.channels(),
        if image.channels() > 1 { "s" } else { "" }
    );

    println!("\n--- Detection with default config ---");
    let mut detector = StarDetector::default();
    let result = detector.detect(&image);

    println!("Stars detected: {}", result.stars.len());
    print_diagnostics(&result.diagnostics);

    println!("\n--- Detection with wide-field preset ---");
    let mut config = StarDetectionConfig::wide_field();
    config.filter.min_snr = 15.0;
    let mut detector = StarDetector::from_config(config).unwrap();
    let result = detector.detect(&image);

    println!("Stars detected: {}", result.stars.len());
    print_diagnostics(&result.diagnostics);

    println!("\n--- Detection with high-resolution preset ---");
    let config = StarDetectionConfig::high_resolution();
    let mut detector = StarDetector::from_config(config).unwrap();
    let result = detector.detect(&image);

    println!("Stars detected: {}", result.stars.len());
    print_diagnostics(&result.diagnostics);

    println!("\n--- Detection with crowded field preset ---");
    let mut config = StarDetectionConfig::crowded_field();
    config.filter.min_snr = 8.0;
    let mut detector = StarDetector::from_config(config).unwrap();
    let result = detector.detect(&image);

    println!("Stars detected: {}", result.stars.len());
    print_diagnostics(&result.diagnostics);

    // Print details for top 10 brightest stars from the last detection
    println!("\n--- Top 10 brightest stars ---");
    println!(
        "{:>5} {:>8} {:>8} {:>8} {:>6} {:>6} {:>6}",
        "#", "X", "Y", "Flux", "FWHM", "SNR", "Ecc"
    );
    println!("{}", "-".repeat(55));

    for (i, star) in result.stars.iter().take(10).enumerate() {
        println!(
            "{:>5} {:>8.2} {:>8.2} {:>8.1} {:>6.2} {:>6.1} {:>6.3}",
            i + 1,
            star.pos.x,
            star.pos.y,
            star.flux,
            star.fwhm,
            star.snr,
            star.eccentricity
        );
    }
}

fn print_diagnostics(diag: &lumos::StarDetectionDiagnostics) {
    println!("  Median FWHM: {:.2} px", diag.median_fwhm);
    println!("  Median SNR: {:.1}", diag.median_snr);
    println!("  Pipeline:");
    println!(
        "    Pixels above threshold: {}",
        diag.pixels_above_threshold
    );
    println!("    Connected components: {}", diag.connected_components);
    println!(
        "    After initial filtering: {}",
        diag.candidates_after_filtering
    );
    println!("    Deblended components: {}", diag.deblended_components);
    println!("    After centroiding: {}", diag.stars_after_centroid);
    println!("  Rejections:");
    println!("    Low SNR: {}", diag.quality_filter.low_snr);
    println!(
        "    High eccentricity: {}",
        diag.quality_filter.high_eccentricity
    );
    println!("    Cosmic rays: {}", diag.quality_filter.cosmic_rays);
    println!("    Saturated: {}", diag.quality_filter.saturated);
    println!("    Roundness: {}", diag.quality_filter.roundness);
    println!("    FWHM outliers: {}", diag.quality_filter.fwhm_outliers);
    println!("    Duplicates: {}", diag.quality_filter.duplicates);
}
