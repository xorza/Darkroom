//! Example: Plate Solving
//!
//! This example demonstrates astrometric plate solving workflows:
//! 1. Load an image and detect stars
//! 2. Configure a plate solver with approximate coordinates and scale
//! 3. Solve to get World Coordinate System (WCS)
//! 4. Convert between pixel and sky coordinates
//! 5. Print solution quality metrics
//!
//! # Usage
//!
//! ```bash
//! cargo run --release --example plate_solve -- /path/to/image.fits 180.0 45.0 1.5
//! ```
//!
//! Arguments:
//! - image_path: Path to astronomical image
//! - approx_ra: Approximate Right Ascension of field center (degrees)
//! - approx_dec: Approximate Declination of field center (degrees)
//! - approx_scale: Approximate pixel scale (arcsec/pixel)

use std::env;
use std::path::Path;

use lumos::{
    AstroImage, CatalogSource, PlateSolver, PlateSolverConfig, StarDetectionConfig, StarDetector,
    Wcs,
};

fn main() {
    // Parse command line arguments
    let args: Vec<String> = env::args().collect();
    if args.len() < 5 {
        eprintln!(
            "Usage: {} <image_path> <approx_ra> <approx_dec> <approx_scale>",
            args[0]
        );
        eprintln!();
        eprintln!("Arguments:");
        eprintln!("  image_path    Path to astronomical image (FITS, TIFF, PNG, RAW)");
        eprintln!("  approx_ra     Approximate field center RA in degrees (0-360)");
        eprintln!("  approx_dec    Approximate field center Dec in degrees (-90 to +90)");
        eprintln!("  approx_scale  Approximate pixel scale in arcsec/pixel");
        eprintln!();
        eprintln!("Example:");
        eprintln!("  {} m31.fits 10.68 41.27 1.5", args[0]);
        std::process::exit(1);
    }

    let image_path = Path::new(&args[1]);
    let approx_ra: f64 = args[2].parse().expect("Invalid RA value");
    let approx_dec: f64 = args[3].parse().expect("Invalid Dec value");
    let approx_scale: f64 = args[4].parse().expect("Invalid scale value");

    // Validate inputs
    assert!(
        (0.0..=360.0).contains(&approx_ra),
        "RA must be between 0 and 360 degrees"
    );
    assert!(
        (-90.0..=90.0).contains(&approx_dec),
        "Dec must be between -90 and +90 degrees"
    );
    assert!(approx_scale > 0.0, "Scale must be positive");

    // Load the image
    println!("Loading image: {}", image_path.display());
    let image = AstroImage::from_file(image_path).expect("Failed to load image");
    println!(
        "Image dimensions: {}x{} pixels",
        image.width(),
        image.height()
    );

    // Detect stars
    println!("\n--- Star Detection ---");
    let mut config = StarDetectionConfig::for_wide_field();
    config.filtering.min_snr = 10.0;
    let mut detector = StarDetector::from_config(config);
    let detection_result = detector.detect(&image);
    println!("Stars detected: {}", detection_result.stars.len());

    if detection_result.stars.len() < 10 {
        eprintln!(
            "Warning: Only {} stars detected. Plate solving may fail.",
            detection_result.stars.len()
        );
    }

    // Extract star positions (sorted by brightness from find_stars)
    let star_positions: Vec<(f64, f64)> = detection_result
        .stars
        .iter()
        .map(|s| (s.x as f64, s.y as f64))
        .collect();

    // Configure plate solver
    println!("\n--- Plate Solving ---");
    println!(
        "Approximate center: RA={:.4}°, Dec={:.4}°",
        approx_ra, approx_dec
    );
    println!("Approximate scale: {:.2} arcsec/pixel", approx_scale);

    let solver_config = PlateSolverConfig {
        catalog: CatalogSource::gaia_vizier(),
        search_radius: 2.0, // degrees
        mag_limit: 14.0,
        max_image_stars: 100,
        max_catalog_stars: 200,
        match_tolerance: 0.02,
        max_rms_arcsec: 2.0,
        min_matches: 10,
    };

    let solver = PlateSolver::new(solver_config);

    // Solve for WCS
    let image_size = (image.width() as u32, image.height() as u32);
    match solver.solve(
        &star_positions,
        (approx_ra, approx_dec),
        approx_scale,
        image_size,
    ) {
        Ok(solution) => {
            println!("\nSolve successful!");
            print_solution(&solution.wcs, solution.num_matches, solution.rms_arcsec);

            // Demonstrate coordinate conversions
            println!("\n--- Coordinate Conversions ---");
            demonstrate_coordinate_conversion(&solution.wcs);
        }
        Err(e) => {
            eprintln!("\nSolve failed: {}", e);
            eprintln!("\nTroubleshooting tips:");
            eprintln!(
                "  - Check that approximate coordinates are within ~2 degrees of actual center"
            );
            eprintln!("  - Verify pixel scale estimate is within ~50% of actual value");
            eprintln!("  - Ensure image has sufficient stars (>20 recommended)");
            eprintln!("  - Try increasing search_radius if field is uncertain");
            std::process::exit(1);
        }
    }

    // Show how to create WCS manually using builder
    println!("\n--- WCS Builder Example ---");
    let manual_wcs = Wcs::builder()
        .crpix(image.width() as f64 / 2.0, image.height() as f64 / 2.0)
        .crval(approx_ra, approx_dec)
        .naxis(image.width() as u32, image.height() as u32)
        .pixel_scale(approx_scale)
        .rotation(0.0)
        .build();

    println!("Created WCS manually:");
    println!(
        "  Reference pixel: ({:.1}, {:.1})",
        manual_wcs.crpix.0, manual_wcs.crpix.1
    );
    println!(
        "  Reference sky: RA={:.4}°, Dec={:.4}°",
        manual_wcs.crval.0, manual_wcs.crval.1
    );
}

fn print_solution(wcs: &Wcs, num_matches: usize, rms_arcsec: f64) {
    println!("\n--- Solution Quality ---");
    println!("Matched stars: {}", num_matches);
    println!("RMS error: {:.2} arcsec", rms_arcsec);

    println!("\n--- Solution Parameters ---");
    let (ra, dec) = wcs.center();
    println!("Field center: RA={:.6}°, Dec={:.6}°", ra, dec);
    println!(
        "Field center: RA={}, Dec={}",
        format_ra(ra),
        format_dec(dec)
    );
    println!("Pixel scale: {:.4} arcsec/pixel", wcs.pixel_scale_arcsec());
    println!(
        "Rotation: {:.2}° (from North through East)",
        wcs.rotation_degrees()
    );
    println!("Mirrored: {}", if wcs.is_mirrored() { "Yes" } else { "No" });

    let (fov_w, fov_h) = wcs.field_of_view();
    println!(
        "Field of view: {:.4}° × {:.4}° ({:.2}' × {:.2}')",
        fov_w,
        fov_h,
        fov_w * 60.0,
        fov_h * 60.0
    );

    println!("\n--- Field Corners (RA, Dec) ---");
    let corners = wcs.corners();
    let labels = ["Bottom-left", "Bottom-right", "Top-right", "Top-left"];
    for (i, (ra, dec)) in corners.iter().enumerate() {
        println!("  {}: ({:.6}°, {:.6}°)", labels[i], ra, dec);
    }
}

fn demonstrate_coordinate_conversion(wcs: &Wcs) {
    // Convert image center pixel to sky
    let center_x = wcs.naxis.0 as f64 / 2.0;
    let center_y = wcs.naxis.1 as f64 / 2.0;
    let (ra, dec) = wcs.pixel_to_sky(center_x, center_y);
    println!(
        "Image center ({:.1}, {:.1}) -> RA={:.6}°, Dec={:.6}°",
        center_x, center_y, ra, dec
    );

    // Convert back to pixel
    let (x, y) = wcs.sky_to_pixel(ra, dec);
    println!("RA={:.6}°, Dec={:.6}° -> ({:.1}, {:.1})", ra, dec, x, y);

    // Test a corner
    let (corner_ra, corner_dec) = wcs.pixel_to_sky(0.0, 0.0);
    println!(
        "Bottom-left corner (0, 0) -> RA={:.6}°, Dec={:.6}°",
        corner_ra, corner_dec
    );
}

/// Format Right Ascension in HMS notation
fn format_ra(ra_deg: f64) -> String {
    let ra_hours = ra_deg / 15.0;
    let h = ra_hours.floor() as i32;
    let m = ((ra_hours - h as f64) * 60.0).floor() as i32;
    let s = ((ra_hours - h as f64) * 60.0 - m as f64) * 60.0;
    format!("{:02}h {:02}m {:.2}s", h, m, s)
}

/// Format Declination in DMS notation
fn format_dec(dec_deg: f64) -> String {
    let sign = if dec_deg >= 0.0 { "+" } else { "-" };
    let dec_abs = dec_deg.abs();
    let d = dec_abs.floor() as i32;
    let m = ((dec_abs - d as f64) * 60.0).floor() as i32;
    let s = ((dec_abs - d as f64) * 60.0 - m as f64) * 60.0;
    format!("{}{}° {:02}' {:.2}\"", sign, d, m, s)
}
