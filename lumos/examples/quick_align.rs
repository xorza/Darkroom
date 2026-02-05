//! Example: Quick Image Alignment
//!
//! This example demonstrates simple two-image alignment:
//! 1. Load reference and target images
//! 2. Detect stars in both
//! 3. Align with `register(&ref_stars, &target_stars, &Config::default())`
//! 4. Warp target to reference with `warp()`
//! 5. Save the aligned result
//!
//! # Usage
//!
//! ```bash
//! cargo run --release --example quick_align -- /path/to/reference.fits /path/to/target.fits /path/to/output.tiff
//! ```

use std::env;
use std::path::Path;

use lumos::{AstroImage, InterpolationMethod, RegistrationConfig, StarDetector, register, warp};

fn main() {
    let args: Vec<String> = env::args().collect();
    if args.len() < 4 {
        eprintln!(
            "Usage: {} <reference_image> <target_image> <output_path>",
            args[0]
        );
        eprintln!("Supported formats: FITS, TIFF, PNG, RAW (RAF, CR2, CR3, NEF, ARW, DNG)");
        std::process::exit(1);
    }

    let ref_path = Path::new(&args[1]);
    let target_path = Path::new(&args[2]);
    let output_path = Path::new(&args[3]);

    // Load images
    println!("Loading reference: {}", ref_path.display());
    let ref_image = AstroImage::from_file(ref_path).expect("Failed to load reference image");
    println!(
        "  Dimensions: {}x{} ({} channel{})",
        ref_image.width(),
        ref_image.height(),
        ref_image.channels(),
        if ref_image.channels() > 1 { "s" } else { "" }
    );

    println!("Loading target: {}", target_path.display());
    let target_image = AstroImage::from_file(target_path).expect("Failed to load target image");
    println!(
        "  Dimensions: {}x{} ({} channel{})",
        target_image.width(),
        target_image.height(),
        target_image.channels(),
        if target_image.channels() > 1 { "s" } else { "" }
    );

    // Detect stars
    println!("\nDetecting stars...");
    let mut detector = StarDetector::new();

    let ref_result = detector.detect(&ref_image);
    println!("  Reference: {} stars detected", ref_result.stars.len());

    let target_result = detector.detect(&target_image);
    println!("  Target: {} stars detected", target_result.stars.len());

    // Align target to reference
    println!("\nAligning images...");
    let result = register(
        &ref_result.stars,
        &target_result.stars,
        &RegistrationConfig::default(),
    )
    .expect("Registration failed");

    println!("  Matched {} stars", result.num_inliers);
    println!("  RMS error: {:.3} pixels", result.rms_error);
    println!("  Transform: {}", result.transform);

    // Warp target to align with reference
    println!("\nWarping target image...");
    let warp_config = RegistrationConfig {
        interpolation: InterpolationMethod::Lanczos3,
        ..Default::default()
    };
    let aligned = warp(&target_image, &result.transform, &warp_config);

    // Save result
    println!("Saving aligned image to: {}", output_path.display());
    aligned
        .save(output_path)
        .expect("Failed to save aligned image");

    println!("\nDone!");
}
