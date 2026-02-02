//! Test star detection on dense4.png real image.
//!
//! Run with: `cargo test -p lumos --features real-data dense4 -- --ignored --nocapture`

use crate::AstroImage;
use crate::CentroidMethod;
use crate::star_detection::StarDetector;
use crate::star_detection::config::{
    BackgroundConfig, BackgroundRefinement, CentroidConfig, Connectivity, DeblendConfig,
    FilteringConfig, LocalBackgroundMethod, PsfConfig, StarDetectionConfig,
};
use crate::testing::{calibration_dir, init_tracing};
use common::test_utils::test_output_path;
use imaginarium::Color;
use imaginarium::ColorFormat;
use imaginarium::drawing::draw_circle;

#[test]
#[ignore] // Requires LUMOS_CALIBRATION_DIR
fn test_detect_dense4() {
    init_tracing();

    let Some(cal_dir) = calibration_dir() else {
        eprintln!("LUMOS_CALIBRATION_DIR not set, skipping test");
        return;
    };

    let image_path = cal_dir.join("dense4.png");
    if !image_path.exists() {
        panic!("dense4.png not found in {:?}", cal_dir);
    }

    println!("Loading: {:?}", image_path);

    let img = imaginarium::Image::read_file(&image_path)
        .expect("Failed to load image")
        .packed()
        .convert(ColorFormat::L_F32)
        .expect("Failed to convert to grayscale");

    let astro_image: AstroImage = img.into();
    println!(
        "Image size: {}x{}",
        astro_image.width(),
        astro_image.height()
    );

    // Fully expanded config - adjust values here to experiment
    let config = StarDetectionConfig {
        background: BackgroundConfig {
            sigma_threshold: 4.0,
            mask_dilation: 3,
            min_unmasked_fraction: 0.3,
            tile_size: 64,
            sigma_clip_iterations: 5,
            refinement: BackgroundRefinement::Iterative { iterations: 2 },
        },
        filtering: FilteringConfig {
            min_area: 5,
            max_area: 500,
            edge_margin: 10,
            min_snr: 10.0,
            max_eccentricity: 0.6,
            max_sharpness: 0.7,
            max_roundness: 1.0,
            max_fwhm_deviation: 3.0,
            duplicate_min_separation: 8.0,
            connectivity: Connectivity::Four,
        },
        deblend: DeblendConfig {
            min_separation: 2,
            min_prominence: 0.3,
            n_thresholds: 32,
            min_contrast: 0.005,
        },
        centroid: CentroidConfig {
            method: CentroidMethod::WeightedMoments,
            local_background_method: LocalBackgroundMethod::GlobalMap,
        },
        psf: PsfConfig {
            expected_fwhm: 4.0,
            axis_ratio: 1.0,
            angle: 0.0,
            auto_estimate: false,
            min_stars_for_estimation: 10,
            estimation_sigma_factor: 2.0,
        },
        noise_model: None,
        defect_map: None,
    };

    let mut detector = StarDetector::from_config(config);

    let start = std::time::Instant::now();
    let result = detector.detect(&astro_image);
    let elapsed = start.elapsed();

    println!("Detection time: {:?}", elapsed);
    println!("Stars found: {}", result.stars.len());

    if !result.stars.is_empty() {
        let avg_fwhm: f32 =
            result.stars.iter().map(|s| s.fwhm).sum::<f32>() / result.stars.len() as f32;
        let avg_snr: f32 =
            result.stars.iter().map(|s| s.snr).sum::<f32>() / result.stars.len() as f32;

        println!("\nStatistics:");
        println!("  Average FWHM: {:.2} px", avg_fwhm);
        println!("  Average SNR: {:.1}", avg_snr);

        println!("\nTop 10 brightest stars:");
        println!(
            "{:>8} {:>8} {:>10} {:>8} {:>8}",
            "X", "Y", "Flux", "FWHM", "SNR"
        );
        for star in result.stars.iter().take(10) {
            println!(
                "{:>8.1} {:>8.1} {:>10.0} {:>8.2} {:>8.1}",
                star.x, star.y, star.flux, star.fwhm, star.snr
            );
        }
    }

    // Load original image for visualization (RGB for colored circles)
    let mut output_img = imaginarium::Image::read_file(&image_path)
        .expect("Failed to load image")
        .packed()
        .convert(ColorFormat::RGB_U8)
        .expect("Failed to convert to RGB");

    // Draw circles around detected stars (top 500 by flux)
    let num_to_draw = result.stars.len().min(500);
    for star in result.stars.iter().take(num_to_draw) {
        let radius = (star.fwhm * 1.5).max(3.0);
        draw_circle(&mut output_img, star.x, star.y, radius, Color::GREEN, 1.0);
    }

    // Save output
    let output_path = test_output_path("dense4_detection.jpg");
    output_img
        .save_file(&output_path)
        .expect("Failed to save output image");
    println!("\nSaved detection result to: {:?}", output_path);

    assert!(!result.stars.is_empty(), "Should find stars in dense4.png");
}
