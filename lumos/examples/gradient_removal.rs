//! Example: Gradient Removal
//!
//! This example demonstrates light pollution and vignetting correction:
//! 1. Remove gradient with default configuration (polynomial degree 2)
//! 2. Use polynomial model for linear and quadratic gradients
//! 3. Use RBF model for complex, non-uniform gradients
//! 4. Compare subtraction vs division correction methods
//!
//! # Usage
//!
//! ```bash
//! cargo run --release --example gradient_removal -- /path/to/image.fits [output_dir]
//! ```

use std::env;
use std::path::Path;

use lumos::{
    AstroImage, CorrectionMethod, GradientModel, GradientRemovalConfig, remove_gradient_image,
};

fn main() {
    // Get image path from command line
    let args: Vec<String> = env::args().collect();
    if args.len() < 2 {
        eprintln!("Usage: {} <image_path> [output_dir]", args[0]);
        eprintln!("Supported formats: FITS, TIFF, PNG, RAW (RAF, CR2, CR3, NEF, ARW, DNG)");
        std::process::exit(1);
    }
    let image_path = Path::new(&args[1]);
    let output_dir = args.get(2).map(Path::new);

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

    // === Gradient removal with default configuration ===
    println!("\n--- Gradient removal with default config (polynomial degree 2) ---");
    let config = GradientRemovalConfig::default();
    print_config(&config);

    match remove_gradient_image(&image, &config) {
        Ok(corrected) => {
            print_correction_stats(&image, &corrected);
            if let Some(dir) = output_dir {
                save_result(&corrected, dir, "corrected_default.tiff");
            }
        }
        Err(e) => eprintln!("Error: {}", e),
    }

    // === Polynomial model: linear gradient (degree 1) ===
    println!("\n--- Linear gradient removal (polynomial degree 1) ---");
    let config = GradientRemovalConfig::polynomial(1);
    print_config(&config);

    match remove_gradient_image(&image, &config) {
        Ok(corrected) => {
            print_correction_stats(&image, &corrected);
            if let Some(dir) = output_dir {
                save_result(&corrected, dir, "corrected_linear.tiff");
            }
        }
        Err(e) => eprintln!("Error: {}", e),
    }

    // === Polynomial model: cubic gradient (degree 3) ===
    println!("\n--- Cubic gradient removal (polynomial degree 3) ---");
    let config = GradientRemovalConfig::polynomial(3)
        .with_samples_per_line(24) // More samples for higher degree
        .with_brightness_tolerance(1.5);
    print_config(&config);

    match remove_gradient_image(&image, &config) {
        Ok(corrected) => {
            print_correction_stats(&image, &corrected);
            if let Some(dir) = output_dir {
                save_result(&corrected, dir, "corrected_cubic.tiff");
            }
        }
        Err(e) => eprintln!("Error: {}", e),
    }

    // === RBF model: smooth interpolation ===
    println!("\n--- RBF gradient removal (smoothing 0.5) ---");
    let config = GradientRemovalConfig::rbf(0.5)
        .with_samples_per_line(20)
        .with_brightness_tolerance(1.2);
    print_config(&config);

    match remove_gradient_image(&image, &config) {
        Ok(corrected) => {
            print_correction_stats(&image, &corrected);
            if let Some(dir) = output_dir {
                save_result(&corrected, dir, "corrected_rbf.tiff");
            }
        }
        Err(e) => eprintln!("Error: {}", e),
    }

    // === Division correction for vignetting ===
    println!("\n--- Vignetting correction (division method) ---");
    let config = GradientRemovalConfig::polynomial(2)
        .with_correction(CorrectionMethod::Divide)
        .with_samples_per_line(20);
    print_config(&config);

    match remove_gradient_image(&image, &config) {
        Ok(corrected) => {
            print_correction_stats(&image, &corrected);
            if let Some(dir) = output_dir {
                save_result(&corrected, dir, "corrected_vignetting.tiff");
            }
        }
        Err(e) => eprintln!("Error: {}", e),
    }

    // === High-precision RBF with minimal smoothing ===
    println!("\n--- High-precision RBF (smoothing 0.1) ---");
    let config = GradientRemovalConfig::rbf(0.1)
        .with_samples_per_line(32)
        .with_brightness_tolerance(0.8)
        .with_min_samples(50);
    print_config(&config);

    match remove_gradient_image(&image, &config) {
        Ok(corrected) => {
            print_correction_stats(&image, &corrected);
            if let Some(dir) = output_dir {
                save_result(&corrected, dir, "corrected_rbf_precise.tiff");
            }
        }
        Err(e) => eprintln!("Error: {}", e),
    }

    if output_dir.is_some() {
        println!("\nResults saved to output directory.");
    } else {
        println!("\nTip: Provide an output directory to save corrected images.");
    }
}

fn print_config(config: &GradientRemovalConfig) {
    let model_str = match config.model {
        GradientModel::Polynomial(deg) => format!("Polynomial(degree={})", deg),
        GradientModel::Rbf(smooth) => format!("RBF(smoothing={:.2})", smooth),
    };
    let correction_str = match config.correction {
        CorrectionMethod::Subtract => "Subtract",
        CorrectionMethod::Divide => "Divide",
    };
    println!("  Model: {}", model_str);
    println!("  Correction: {}", correction_str);
    println!("  Samples per line: {}", config.samples_per_line);
    println!(
        "  Brightness tolerance: {:.2}σ",
        config.brightness_tolerance
    );
    println!("  Min samples: {}", config.min_samples);
}

fn print_correction_stats(original: &AstroImage, corrected: &AstroImage) {
    let orig_stats = compute_stats(&original.pixels());
    let corr_stats = compute_stats(&corrected.pixels());

    println!(
        "  Original: mean={:.4}, std={:.4}",
        orig_stats.0, orig_stats.1
    );
    println!(
        "  Corrected: mean={:.4}, std={:.4}",
        corr_stats.0, corr_stats.1
    );

    let variance_reduction = 1.0 - (corr_stats.1 / orig_stats.1);
    println!("  Variance reduction: {:.1}%", variance_reduction * 100.0);
}

fn compute_stats(pixels: &[f32]) -> (f32, f32) {
    let n = pixels.len() as f32;
    let mean = pixels.iter().sum::<f32>() / n;
    let variance = pixels.iter().map(|&x| (x - mean).powi(2)).sum::<f32>() / n;
    (mean, variance.sqrt())
}

fn save_result(image: &AstroImage, output_dir: &Path, filename: &str) {
    let output_path = output_dir.join(filename);
    if let Err(e) = image.save(&output_path) {
        eprintln!("Failed to save {}: {}", output_path.display(), e);
    } else {
        println!("  Saved: {}", output_path.display());
    }
}
