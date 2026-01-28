//! Example: Comet Stacking
//!
//! This example demonstrates tracking and stacking moving objects (comets, asteroids):
//! 1. Define comet positions at start and end of sequence
//! 2. Configure with `CometStackConfig`
//! 3. Register frames on stars
//! 4. Create star-aligned stack (sharp stars, blurred comet)
//! 5. Create comet-aligned stack (sharp comet, star trails rejected)
//! 6. Composite both stacks for final result
//!
//! # Usage
//!
//! ```bash
//! cargo run --release --example comet_stacking -- \
//!     --comet-start 512.5,384.2,0.0 \
//!     --comet-end 520.8,390.1,3600.0 \
//!     /path/to/frame1.fits /path/to/frame2.fits ...
//! ```
//!
//! Position format: x,y,timestamp (timestamp in seconds)

use std::env;
use std::path::Path;

use lumos::{
    AstroImage, CometStackConfig, CompositeMethod, ImageDimensions, InterpolationMethod,
    ObjectPosition, StarDetector, TransformMatrix, apply_comet_offset_to_transform,
    composite_stacks, create_comet_stack_result, quick_register_stars, warp_to_reference_image,
};

fn main() {
    let args: Vec<String> = env::args().collect();

    // Parse arguments
    let (comet_start, comet_end, frame_paths) = match parse_args(&args) {
        Ok(parsed) => parsed,
        Err(msg) => {
            eprintln!("Error: {}", msg);
            print_usage(&args[0]);
            std::process::exit(1);
        }
    };

    if frame_paths.len() < 3 {
        eprintln!("Error: Need at least 3 frames for comet stacking");
        print_usage(&args[0]);
        std::process::exit(1);
    }

    println!("Comet Stacking Example");
    println!("======================\n");

    // Create comet configuration
    let config =
        CometStackConfig::new(comet_start, comet_end).composite_method(CompositeMethod::Lighten);

    // Print comet motion info
    let (vx, vy) = config.velocity();
    let displacement = config.total_displacement();
    println!("Comet motion:");
    println!(
        "  Start: ({:.1}, {:.1}) at t={:.1}s",
        comet_start.x, comet_start.y, comet_start.timestamp
    );
    println!(
        "  End:   ({:.1}, {:.1}) at t={:.1}s",
        comet_end.x, comet_end.y, comet_end.timestamp
    );
    println!("  Velocity: ({:.4}, {:.4}) px/s", vx, vy);
    println!("  Total displacement: {:.2} px\n", displacement);

    // Load reference frame (first frame)
    println!("Loading {} frames...", frame_paths.len());
    let reference = AstroImage::from_file(frame_paths[0]).expect("Failed to load reference frame");
    let (width, height) = (reference.width(), reference.height());
    println!(
        "  Reference: {}x{} ({} channel{})",
        width,
        height,
        reference.channels(),
        if reference.channels() > 1 { "s" } else { "" }
    );

    // Generate timestamps for frames (evenly spaced from start to end)
    let ref_timestamp = comet_start.timestamp;
    let end_timestamp = comet_end.timestamp;
    let frame_timestamps: Vec<f64> = (0..frame_paths.len())
        .map(|i| {
            ref_timestamp
                + (end_timestamp - ref_timestamp) * (i as f64 / (frame_paths.len() - 1) as f64)
        })
        .collect();

    // Star detector
    let detector = StarDetector::new();

    // Detect stars in reference
    println!("\nDetecting stars in reference...");
    let ref_result = detector.detect(&reference);
    println!("  Found {} stars", ref_result.stars.len());

    // Process frames: register, warp, and collect for stacking
    println!("\nProcessing frames...");
    println!(
        "{:>3} {:>30} {:>6} {:>8} {:>8}",
        "#", "Frame", "Stars", "Matched", "RMS"
    );
    println!("{}", "-".repeat(60));

    let mut star_aligned_frames: Vec<AstroImage> = Vec::new();
    let mut comet_aligned_frames: Vec<AstroImage> = Vec::new();
    let mut star_transforms: Vec<TransformMatrix> = Vec::new();

    for (i, path) in frame_paths.iter().enumerate() {
        let frame = match AstroImage::from_file(path) {
            Ok(f) => f,
            Err(e) => {
                eprintln!("  Skipping {}: {}", path.display(), e);
                continue;
            }
        };

        // Register frame to reference
        let frame_result = detector.detect(&frame);
        let transform = if i == 0 {
            // Reference frame uses identity transform
            TransformMatrix::identity()
        } else {
            match quick_register_stars(&ref_result.stars, &frame_result.stars) {
                Ok(result) => {
                    let filename = path.file_name().unwrap_or_default().to_string_lossy();
                    println!(
                        "{:>3} {:>30} {:>6} {:>8} {:>8.3}",
                        i + 1,
                        truncate_str(&filename, 30),
                        frame_result.stars.len(),
                        result.num_inliers,
                        result.rms_error
                    );
                    result.transform
                }
                Err(e) => {
                    eprintln!("  Skipping frame {}: registration failed ({})", i, e);
                    continue;
                }
            }
        };

        // Print reference frame info
        if i == 0 {
            let filename = path.file_name().unwrap_or_default().to_string_lossy();
            println!(
                "{:>3} {:>30} {:>6} {:>8} {:>8}",
                i + 1,
                truncate_str(&filename, 30),
                frame_result.stars.len(),
                "-",
                "ref"
            );
        }

        // Create star-aligned warp
        let star_aligned =
            warp_to_reference_image(&frame, &transform, InterpolationMethod::Lanczos3);

        // Create comet-aligned transform and warp
        let frame_timestamp = frame_timestamps[i];
        let comet_transform =
            apply_comet_offset_to_transform(&transform, &config, frame_timestamp, ref_timestamp);
        let comet_aligned =
            warp_to_reference_image(&frame, &comet_transform, InterpolationMethod::Lanczos3);

        star_aligned_frames.push(star_aligned);
        comet_aligned_frames.push(comet_aligned);
        star_transforms.push(transform);
    }

    if star_aligned_frames.len() < 3 {
        eprintln!("\nError: Not enough frames successfully processed");
        std::process::exit(1);
    }

    // Stack star-aligned frames (mean)
    println!("\nStacking {} frames...", star_aligned_frames.len());
    let star_stack = stack_mean(&star_aligned_frames);
    println!("  Star-aligned stack: {} pixels", star_stack.len());

    // Stack comet-aligned frames (mean)
    let comet_stack = stack_mean(&comet_aligned_frames);
    println!("  Comet-aligned stack: {} pixels", comet_stack.len());

    // Create composite
    let result = create_comet_stack_result(star_stack, comet_stack, width, height, &config);

    println!("\n--- Results ---");
    println!(
        "Velocity: ({:.4}, {:.4}) px/s",
        result.velocity.0, result.velocity.1
    );
    println!("Displacement: {:.2} px", result.displacement);

    // Save results
    let star_image = AstroImage::from_pixels(
        ImageDimensions::new(width, height, 1),
        result.star_stack.clone(),
    );
    let comet_image = AstroImage::from_pixels(
        ImageDimensions::new(width, height, 1),
        result.comet_stack.clone(),
    );

    let star_path = Path::new("comet_star_aligned.tiff");
    let comet_path = Path::new("comet_comet_aligned.tiff");
    println!("\nSaving star-aligned stack to: {}", star_path.display());
    star_image.save(star_path).expect("Failed to save");

    println!("Saving comet-aligned stack to: {}", comet_path.display());
    comet_image.save(comet_path).expect("Failed to save");

    // Save composite if available
    if let Some(composite_pixels) = &result.composite {
        let composite_image = AstroImage::from_pixels(
            ImageDimensions::new(width, height, 1),
            composite_pixels.clone(),
        );
        let composite_path = Path::new("comet_composite.tiff");
        println!("Saving composite to: {}", composite_path.display());
        composite_image
            .save(composite_path)
            .expect("Failed to save");
    }

    // Demonstrate different composite methods
    println!("\n--- Composite Methods ---");

    // Lighten (already used above)
    println!("Lighten: max(star, comet) per pixel - good for dark backgrounds");

    // Additive
    if let Some(additive) = composite_stacks(
        &result.star_stack,
        &result.comet_stack,
        CompositeMethod::Additive,
    ) {
        let additive_image =
            AstroImage::from_pixels(ImageDimensions::new(width, height, 1), additive);
        let additive_path = Path::new("comet_additive.tiff");
        println!(
            "Additive: star + (comet - bg) - saved to: {}",
            additive_path.display()
        );
        additive_image.save(additive_path).expect("Failed to save");
    }

    // Separate
    let separate = composite_stacks(
        &result.star_stack,
        &result.comet_stack,
        CompositeMethod::Separate,
    );
    println!(
        "Separate: returns None (both stacks available separately) - is_none={}",
        separate.is_none()
    );

    println!("\nDone!");
}

/// Stack frames using simple mean.
fn stack_mean(frames: &[AstroImage]) -> Vec<f32> {
    assert!(!frames.is_empty(), "Need at least one frame to stack");

    let pixels = frames[0].to_interleaved_pixels();
    let pixel_count = pixels.len();
    let mut sum = vec![0.0f64; pixel_count];

    for (i, &val) in pixels.iter().enumerate() {
        sum[i] += f64::from(val);
    }

    for frame in &frames[1..] {
        for (i, &val) in frame.to_interleaved_pixels().iter().enumerate() {
            sum[i] += f64::from(val);
        }
    }

    let n = frames.len() as f64;
    sum.into_iter().map(|s| (s / n) as f32).collect()
}

/// Parse command line arguments.
fn parse_args(args: &[String]) -> Result<(ObjectPosition, ObjectPosition, Vec<&Path>), String> {
    let mut comet_start: Option<ObjectPosition> = None;
    let mut comet_end: Option<ObjectPosition> = None;
    let mut frame_paths: Vec<&Path> = Vec::new();

    let mut i = 1;
    while i < args.len() {
        match args[i].as_str() {
            "--comet-start" => {
                i += 1;
                if i >= args.len() {
                    return Err("--comet-start requires a value".to_string());
                }
                comet_start = Some(parse_position(&args[i])?);
            }
            "--comet-end" => {
                i += 1;
                if i >= args.len() {
                    return Err("--comet-end requires a value".to_string());
                }
                comet_end = Some(parse_position(&args[i])?);
            }
            arg if arg.starts_with('-') => {
                return Err(format!("Unknown option: {}", arg));
            }
            _ => {
                frame_paths.push(Path::new(&args[i]));
            }
        }
        i += 1;
    }

    let comet_start = comet_start.ok_or("--comet-start is required")?;
    let comet_end = comet_end.ok_or("--comet-end is required")?;

    if frame_paths.is_empty() {
        return Err("No frame paths provided".to_string());
    }

    Ok((comet_start, comet_end, frame_paths))
}

/// Parse position string "x,y,timestamp".
fn parse_position(s: &str) -> Result<ObjectPosition, String> {
    let parts: Vec<&str> = s.split(',').collect();
    if parts.len() != 3 {
        return Err(format!("Position must be x,y,timestamp (got '{}')", s));
    }

    let x: f64 = parts[0]
        .trim()
        .parse()
        .map_err(|_| format!("Invalid x coordinate: {}", parts[0]))?;
    let y: f64 = parts[1]
        .trim()
        .parse()
        .map_err(|_| format!("Invalid y coordinate: {}", parts[1]))?;
    let timestamp: f64 = parts[2]
        .trim()
        .parse()
        .map_err(|_| format!("Invalid timestamp: {}", parts[2]))?;

    Ok(ObjectPosition::new(x, y, timestamp))
}

/// Print usage information.
fn print_usage(program: &str) {
    eprintln!(
        "\nUsage: {} --comet-start x,y,t --comet-end x,y,t <frame1> <frame2> ...",
        program
    );
    eprintln!("\nOptions:");
    eprintln!("  --comet-start x,y,t  Comet position at sequence start (pixels,timestamp)");
    eprintln!("  --comet-end x,y,t    Comet position at sequence end (pixels,timestamp)");
    eprintln!("\nExample:");
    eprintln!(
        "  {} --comet-start 512.5,384.2,0 --comet-end 520.8,390.1,3600 *.fits",
        program
    );
    eprintln!("\nSupported formats: FITS, TIFF, PNG, RAW (RAF, CR2, CR3, NEF, ARW, DNG)");
}

/// Truncate string to max length with ellipsis.
fn truncate_str(s: &str, max_len: usize) -> String {
    if s.len() <= max_len {
        s.to_string()
    } else {
        format!("...{}", &s[s.len() - (max_len - 3)..])
    }
}
