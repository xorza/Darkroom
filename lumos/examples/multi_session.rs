//! Example: Multi-Session Stacking
//!
//! This example demonstrates combining data from multiple imaging sessions:
//! 1. Create `Session` objects with frame paths
//! 2. Assess quality with `session.assess_quality()`
//! 3. Create `MultiSessionStack` with sessions
//! 4. Configure with `SessionConfig`
//! 5. Filter low-quality frames
//! 6. Stack with `stack_session_weighted()`
//! 7. Print session contributions
//! 8. Save result
//!
//! # Usage
//!
//! ```bash
//! # With two directories (one per session)
//! cargo run --release --example multi_session -- /path/to/session1/*.fits /path/to/session2/*.fits
//!
//! # With explicit session markers (using -- separator)
//! cargo run --release --example multi_session -- frame1.fits frame2.fits -- frame3.fits frame4.fits
//! ```
//!
//! Each group of files separated by `--` is treated as a separate session.
//! If no `--` separators are found, all files are grouped into sessions by parent directory.

use std::collections::HashMap;
use std::env;
use std::path::{Path, PathBuf};

use lumos::{MultiSessionStack, Session, SessionConfig, StarDetector};

fn main() {
    let args: Vec<String> = env::args().skip(1).collect();
    if args.is_empty() {
        eprintln!("Usage: multi_session <session1_files...> [-- <session2_files...>] ...");
        eprintln!("  Groups files by '--' separators or by parent directory.");
        eprintln!("  Supported formats: FITS, TIFF, PNG, RAW (RAF, CR2, CR3, NEF, ARW, DNG)");
        std::process::exit(1);
    }

    // Parse arguments into sessions
    let session_groups = parse_sessions(&args);
    if session_groups.is_empty() {
        eprintln!("Error: No valid session groups found.");
        std::process::exit(1);
    }

    println!("Multi-Session Stacking Example");
    println!("==============================\n");
    println!("Found {} session(s)", session_groups.len());

    // Create sessions with quality assessment
    let mut detector = StarDetector::new();
    let mut sessions = Vec::new();

    for (idx, (session_id, paths)) in session_groups.iter().enumerate() {
        println!(
            "\n--- Session {}: '{}' ({} frames) ---",
            idx + 1,
            session_id,
            paths.len()
        );

        // Create session and assess quality
        let session = Session::new(session_id.clone())
            .with_frames(paths)
            .assess_quality(&mut detector)
            .expect("Failed to assess session quality");

        // Print session quality metrics
        let quality = &session.quality;
        println!("  Median FWHM: {:.2} px", quality.median_fwhm);
        println!("  Median SNR: {:.1}", quality.median_snr);
        println!("  Median eccentricity: {:.3}", quality.median_eccentricity);
        println!(
            "  Usable frames: {}/{}",
            quality.usable_frame_count, quality.frame_count
        );

        sessions.push(session);
    }

    // Configure multi-session stacking
    let config = SessionConfig::default()
        .with_quality_threshold(0.3) // Keep frames with >= 30% of median quality
        .with_normalization_tile_size(64); // Use smaller tiles for better local correction

    println!("\n--- Stacking Configuration ---");
    println!(
        "  Quality threshold: {:.0}%",
        config.quality_threshold * 100.0
    );
    println!(
        "  Session weighting: {}",
        if config.use_session_weights {
            "enabled"
        } else {
            "disabled"
        }
    );
    println!(
        "  Local normalization: {}",
        if config.use_local_normalization {
            "enabled"
        } else {
            "disabled"
        }
    );

    // Create multi-session stack
    let mut stack = MultiSessionStack::new(sessions).with_config(config);

    // Print summary before stacking
    let summary = stack.summary();
    println!("\n{}", summary);

    // Get filtered frame paths (applying quality threshold)
    let filtered_paths = stack.filter_all_frames();
    println!(
        "Frames passing quality filter: {}/{}",
        filtered_paths.len(),
        stack.total_frame_count()
    );

    // Perform session-weighted integration
    println!("\nStacking frames...");
    let result = stack
        .stack_session_weighted()
        .expect("Failed to stack sessions");

    // Print result summary
    println!("\n{}", result);

    // Print session contributions
    println!("Session Contributions:");
    let contributions = result.session_contributions(&stack);
    for (session_idx, weight) in contributions {
        let session = &stack.sessions[session_idx];
        println!(
            "  Session '{}': {:.1}% contribution ({} frames)",
            session.id,
            weight * 100.0,
            session.frame_count()
        );
    }

    // Save result
    let output_path = Path::new("multi_session_stack.tiff");
    println!("\nSaving result to: {}", output_path.display());
    result
        .image()
        .save(output_path)
        .expect("Failed to save stacked image");

    println!("\nDone!");
}

/// Parse command-line arguments into session groups.
///
/// If `--` separators are found, split by them.
/// Otherwise, group files by their parent directory.
fn parse_sessions(args: &[String]) -> Vec<(String, Vec<PathBuf>)> {
    // Check for explicit session separators
    let has_separators = args.iter().any(|a| a == "--");

    if has_separators {
        // Split by -- separators
        let mut sessions = Vec::new();
        let mut current_group = Vec::new();
        let mut session_idx = 1;

        for arg in args {
            if arg == "--" {
                if !current_group.is_empty() {
                    sessions.push((format!("session_{}", session_idx), current_group));
                    session_idx += 1;
                    current_group = Vec::new();
                }
            } else {
                let path = PathBuf::from(arg);
                if path.exists() {
                    current_group.push(path);
                }
            }
        }

        if !current_group.is_empty() {
            sessions.push((format!("session_{}", session_idx), current_group));
        }

        sessions
    } else {
        // Group by parent directory
        let mut dir_groups: HashMap<String, Vec<PathBuf>> = HashMap::new();

        for arg in args {
            let path = PathBuf::from(arg);
            if path.exists() {
                let dir_name = path
                    .parent()
                    .and_then(|p| p.file_name())
                    .map(|n| n.to_string_lossy().to_string())
                    .unwrap_or_else(|| "unknown".to_string());
                dir_groups.entry(dir_name).or_default().push(path);
            }
        }

        // Sort by directory name for consistent ordering
        let mut sessions: Vec<_> = dir_groups.into_iter().collect();
        sessions.sort_by(|a, b| a.0.cmp(&b.0));
        sessions
    }
}
