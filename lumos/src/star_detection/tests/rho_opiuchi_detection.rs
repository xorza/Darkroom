//! Test star detection on dense4.png real image.
//!
//! Run with: `cargo test -p lumos --features real-data dense4 -- --ignored --nocapture`

use crate::AstroImage;
use crate::CentroidMethod;
use crate::star_detection::StarDetector;
use crate::star_detection::config::{
    AdaptiveSigmaConfig, BackgroundConfig, BackgroundRefinement, CentroidConfig, Connectivity,
    DeblendConfig, FilteringConfig, LocalBackgroundMethod, PsfConfig, StarDetectionConfig,
};
use crate::testing::{calibration_dir, init_tracing};
use common::test_utils::test_output_path;
use imaginarium::Color;
use imaginarium::ColorFormat;
use imaginarium::drawing::draw_circle;

#[test]
#[ignore] // Requires LUMOS_CALIBRATION_DIR
fn test_detect_rho_opiuchi() {
    init_tracing();

    let Some(cal_dir) = calibration_dir() else {
        eprintln!("LUMOS_CALIBRATION_DIR not set, skipping test");
        return;
    };

    let image_path = cal_dir.join("rho-opiuchi.jpg");
    if !image_path.exists() {
        panic!("rho-opiuchi.jpg not found in {:?}", cal_dir);
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

    // Config optimized for Rho Ophiuchi: nebulosity + dense star field (M4 cluster)
    let config = StarDetectionConfig {
        background: BackgroundConfig {
            sigma_threshold: 3.0,
            mask_dilation: 5,
            min_unmasked_fraction: 0.2,
            tile_size: 128,
            sigma_clip_iterations: 3,
            refinement: BackgroundRefinement::AdaptiveSigma(AdaptiveSigmaConfig {
                base_sigma: 3.0,
                max_sigma: 10.0,
                contrast_factor: 3.0,
            }),
        },
        filtering: FilteringConfig {
            min_area: 3,
            max_area: 2000,
            edge_margin: 15,
            min_snr: 5.0,
            max_eccentricity: 0.7,
            max_sharpness: 0.8,
            max_roundness: 1.0,
            max_fwhm_deviation: 4.0,
            duplicate_min_separation: 5.0,
            connectivity: Connectivity::Eight,
        },
        deblend: DeblendConfig {
            min_separation: 2,
            min_prominence: 0.2,
            n_thresholds: 64,
            min_contrast: 0.003,
        },
        centroid: CentroidConfig {
            method: CentroidMethod::WeightedMoments,
            local_background_method: LocalBackgroundMethod::LocalAnnulus,
        },
        psf: PsfConfig {
            expected_fwhm: 3.0,
            axis_ratio: 1.0,
            angle: 0.0,
            auto_estimate: true,
            min_stars_for_estimation: 20,
            estimation_sigma_factor: 2.5,
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

    // Load original image for visualization (RGB_F32 for drawing functions)
    let mut output_img = imaginarium::Image::read_file(&image_path)
        .expect("Failed to load image")
        .packed()
        .convert(ColorFormat::RGB_F32)
        .expect("Failed to convert to RGB_F32");

    // Draw circles around all detected stars
    for star in result.stars.iter() {
        let radius = (star.fwhm * 1.5).max(3.0);
        draw_circle(&mut output_img, star.x, star.y, radius, Color::GREEN, 1.0);
    }
    println!("Drew {} circles", result.stars.len());

    // Convert back to RGB_U8 for saving
    let output_img = output_img
        .convert(ColorFormat::RGB_U8)
        .expect("Failed to convert to RGB_U8");

    // Save output
    let output_path = test_output_path("rho-opiuchi-detection.jpg");
    output_img
        .save_file(&output_path)
        .expect("Failed to save output image");
    println!("\nSaved detection result to: {:?}", output_path);

    assert!(
        !result.stars.is_empty(),
        "Should find stars in rho-opiuchi.jpg"
    );
}
