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

use lumos::{AstroImage, StarDetectionConfig, find_stars};

fn main() {
    // Get image path from command line
    let args: Vec<String> = env::args().collect();
    if args.len() < 2 {
        eprintln!("Usage: {} <image_path>", args[0]);
        eprintln!("Supported formats: FITS, TIFF, PNG, RAW (RAF, CR2, CR3, NEF, ARW, DNG)");
        std::process::exit(1);
    }
    let image_path = Path::new(&args[1]);

    // Load the image
    println!("Loading image: {}", image_path.display());
    let image = AstroImage::from_file(image_path).expect("Failed to load image");
    println!(
        "Image dimensions: {}x{} ({} channel{})",
        image.width(),
        image.height(),
        image.channels(),
        if image.channels() > 1 { "s" } else { "" }
    );

    // === Detection with default configuration ===
    println!("\n--- Detection with default config ---");
    let default_config = StarDetectionConfig::default();
    let result = find_stars(&image, &default_config);

    println!("Stars detected: {}", result.stars.len());
    print_diagnostics(&result.diagnostics);

    // === Detection with wide-field preset ===
    println!("\n--- Detection with wide-field preset ---");
    let wide_field_config = StarDetectionConfig::builder()
        .for_wide_field()
        .with_min_snr(15.0)
        .build();
    let result = find_stars(&image, &wide_field_config);

    println!("Stars detected: {}", result.stars.len());
    print_diagnostics(&result.diagnostics);

    // === Detection with high-resolution preset ===
    println!("\n--- Detection with high-resolution preset ---");
    let high_res_config = StarDetectionConfig::builder()
        .for_high_resolution()
        .with_cosmic_ray_rejection(0.7)
        .build();
    let result = find_stars(&image, &high_res_config);

    println!("Stars detected: {}", result.stars.len());
    print_diagnostics(&result.diagnostics);

    // === Detection with crowded field preset ===
    println!("\n--- Detection with crowded field preset ---");
    let crowded_config = StarDetectionConfig::builder()
        .for_crowded_field()
        .with_min_snr(8.0)
        .build();
    let result = find_stars(&image, &crowded_config);

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
            star.x,
            star.y,
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
    println!(
        "  Background: min={:.4}, max={:.4}, mean={:.4}",
        diag.background_stats.0, diag.background_stats.1, diag.background_stats.2
    );
    println!(
        "  Noise: min={:.6}, max={:.6}, mean={:.6}",
        diag.noise_stats.0, diag.noise_stats.1, diag.noise_stats.2
    );
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
    println!("    Low SNR: {}", diag.rejected_low_snr);
    println!("    High eccentricity: {}", diag.rejected_high_eccentricity);
    println!("    Cosmic rays: {}", diag.rejected_cosmic_rays);
    println!("    Saturated: {}", diag.rejected_saturated);
    println!("    Roundness: {}", diag.rejected_roundness);
    println!("    FWHM outliers: {}", diag.rejected_fwhm_outliers);
    println!("    Duplicates: {}", diag.rejected_duplicates);
}
