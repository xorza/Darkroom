//! Example: Live Stacking (EAA Workflow)
//!
//! This example demonstrates real-time Electronically Assisted Astronomy (EAA) workflow:
//! 1. Create accumulator from reference frame
//! 2. Configure stacking mode with builder pattern
//! 3. Add frames with quality metrics in real-time
//! 4. Track SNR improvement as frames accumulate
//! 5. Get preview and finalize result
//!
//! # Usage
//!
//! ```bash
//! # With image files
//! cargo run --release --example live_stacking -- /path/to/frame1.fits /path/to/frame2.fits ...
//!
//! # With a directory of frames
//! cargo run --release --example live_stacking -- /path/to/frames/*.fits
//! ```

use std::env;
use std::path::Path;

use lumos::{AstroImage, LiveFrameQuality, LiveStackAccumulator, LiveStackConfig, StarDetector};

fn main() {
    let args: Vec<String> = env::args().collect();
    if args.len() < 3 {
        eprintln!("Usage: {} <frame1> <frame2> [frame3 ...]", args[0]);
        eprintln!("Provide at least 2 frames to stack.");
        eprintln!("Supported formats: FITS, TIFF, PNG, RAW (RAF, CR2, CR3, NEF, ARW, DNG)");
        std::process::exit(1);
    }

    let frame_paths: Vec<&Path> = args[1..].iter().map(Path::new).collect();
    println!("Live stacking {} frames\n", frame_paths.len());

    // Load first frame as reference
    println!("Loading reference frame: {}", frame_paths[0].display());
    let reference = AstroImage::from_file(frame_paths[0]).expect("Failed to load reference frame");
    println!(
        "  Dimensions: {}x{} ({} channel{})\n",
        reference.width(),
        reference.height(),
        reference.channels(),
        if reference.channels() > 1 { "s" } else { "" }
    );

    // Configure live stacking with builder pattern
    // Using weighted mean for quality-based frame weighting
    let config = LiveStackConfig::builder()
        .weighted_mean()
        .normalize(true) // Normalize frames to match reference statistics
        .track_variance(true) // Track variance for quality estimation
        .build();

    // Create accumulator from reference frame dimensions
    let mut stack =
        LiveStackAccumulator::from_reference(&reference, config).expect("Failed to create stack");

    // Star detector for quality assessment
    let detector = StarDetector::new();

    // Process frames in real-time
    println!("Processing frames...");
    println!(
        "{:>3} {:>30} {:>6} {:>6} {:>8} {:>8}",
        "#", "Frame", "Stars", "SNR", "FWHM", "SNR×"
    );
    println!("{}", "-".repeat(70));

    for (i, path) in frame_paths.iter().enumerate() {
        // Load frame
        let frame = match AstroImage::from_file(path) {
            Ok(f) => f,
            Err(e) => {
                eprintln!("  Skipping {}: {}", path.display(), e);
                continue;
            }
        };

        // Compute frame quality from star detection
        let quality = compute_frame_quality(&frame, &detector);

        // Add frame to stack
        let stats = stack
            .add_frame(&frame, quality)
            .expect("Failed to add frame");

        // Display progress
        let filename = path.file_name().unwrap_or_default().to_string_lossy();
        println!(
            "{:>3} {:>30} {:>6} {:>6.1} {:>8.2} {:>8.2}",
            i + 1,
            truncate_str(&filename, 30),
            quality.star_count,
            quality.snr,
            quality.fwhm,
            stats.snr_improvement
        );
    }

    // Get final statistics
    let stats = stack.stats();
    println!("\n--- Stack Summary ---");
    println!("Total frames: {}", stats.frame_count);
    println!(
        "SNR improvement: {:.2}× (vs single frame)",
        stats.snr_improvement
    );
    println!("Mean FWHM: {:.2} px", stats.mean_fwhm);
    println!("Mean eccentricity: {:.3}", stats.mean_eccentricity);

    if let Some(variance) = stats.mean_variance {
        println!("Mean variance: {:.6}", variance);
    }

    // Finalize and save result
    let result = stack.finalize().expect("Failed to finalize stack");

    let output_path = Path::new("live_stack_result.tiff");
    println!("\nSaving result to: {}", output_path.display());
    result
        .image
        .save(output_path)
        .expect("Failed to save result");

    println!("\nDone! Final result: {}", result);
}

/// Compute frame quality metrics from star detection.
fn compute_frame_quality(frame: &AstroImage, detector: &StarDetector) -> LiveFrameQuality {
    let result = detector.detect(frame);

    if result.stars.is_empty() {
        return LiveFrameQuality::unknown();
    }

    // Use median values from detected stars
    let snr = result.diagnostics.median_snr;
    let fwhm = result.diagnostics.median_fwhm;

    // Compute median eccentricity
    let mut eccs: Vec<f32> = result.stars.iter().map(|s| s.eccentricity).collect();
    eccs.sort_by(|a: &f32, b: &f32| a.partial_cmp(b).unwrap());
    let median_ecc = eccs[eccs.len() / 2];

    // Estimate noise from background stats
    let noise = result.diagnostics.noise_stats.2; // mean noise

    LiveFrameQuality {
        snr,
        fwhm,
        eccentricity: median_ecc,
        noise,
        star_count: result.stars.len(),
    }
}

/// Truncate string to max length with ellipsis.
fn truncate_str(s: &str, max_len: usize) -> String {
    if s.len() <= max_len {
        s.to_string()
    } else {
        format!("...{}", &s[s.len() - (max_len - 3)..])
    }
}
