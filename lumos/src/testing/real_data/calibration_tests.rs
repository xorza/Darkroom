//! Tests that require real calibration data from the environment.
//!
//! These tests require:
//! - `LUMOS_CALIBRATION_DIR` environment variable pointing to calibration data
//! - Optionally `LUMOS_TEST_CACHE_DIR` for caching test artifacts
//!
//! Run with: `cargo test -p lumos real_data_tests -- --ignored`
//!
//! The calibration directory should contain:
//! - `Lights/` - Light frames (RAW files like .RAF, .CR2, etc.)
//! - `Darks/` - Dark frames
//! - `Flats/` - Flat frames
//! - `Bias/` - Bias frames
//! - `calibrated_light.tiff` - A calibrated light frame
//! - `calibrated_light_500x500.tiff` - A cropped calibrated light frame
//! - `calibrated_light_500x500_stretched.tiff` - Stretched version
//! - `calibrated_light_only_sky.tiff` - Sky-only region for star detection

use crate::AstroImage;
use crate::testing::{calibration_dir, calibration_image_paths, init_tracing};

// =============================================================================
// Star Detection Tests
// =============================================================================

#[test]
#[ignore] // Requires LUMOS_CALIBRATION_DIR
fn test_find_stars_on_light_frame() {
    use crate::star_detection::{StarDetectionConfig, StarDetector};

    init_tracing();

    let Some(cal_dir) = calibration_dir() else {
        eprintln!("LUMOS_CALIBRATION_DIR not set, skipping test");
        return;
    };

    let lights_dir = cal_dir.join("Lights");
    if !lights_dir.exists() {
        eprintln!("Lights directory not found, skipping test");
        return;
    }

    let files = common::file_utils::astro_image_files(&lights_dir);
    let Some(first_file) = files.first() else {
        eprintln!("No image files in Lights, skipping test");
        return;
    };

    println!("Loading light frame: {:?}", first_file);
    let start = std::time::Instant::now();
    let image = AstroImage::from_file(first_file).expect("Failed to load image");
    println!(
        "Loaded {}x{} image in {:?}",
        image.width(),
        image.height(),
        start.elapsed()
    );

    let (width, height) = (image.width(), image.height());

    let config = StarDetectionConfig::default();
    let start = std::time::Instant::now();
    let mut detector = StarDetector::from_config(config);
    let result = detector.detect(&image);
    println!(
        "Found {} stars in {:?}",
        result.stars.len(),
        start.elapsed()
    );

    assert!(
        !result.stars.is_empty(),
        "Should find at least some stars in a light frame"
    );

    let stars = result.stars;

    println!("\nTop 10 brightest stars:");
    println!(
        "{:>8} {:>8} {:>10} {:>8} {:>8} {:>8}",
        "X", "Y", "Flux", "FWHM", "SNR", "Ecc"
    );
    for star in stars.iter().take(10) {
        println!(
            "{:>8.2} {:>8.2} {:>10.1} {:>8.2} {:>8.1} {:>8.3}",
            star.x, star.y, star.flux, star.fwhm, star.snr, star.eccentricity
        );
    }

    for star in &stars {
        assert!(
            star.x >= 0.0 && star.x < width as f32,
            "Star X out of bounds"
        );
        assert!(
            star.y >= 0.0 && star.y < height as f32,
            "Star Y out of bounds"
        );
        assert!(star.flux > 0.0, "Star flux should be positive");
        assert!(star.fwhm > 0.0, "Star FWHM should be positive");
        assert!(star.snr > 0.0, "Star SNR should be positive");
        assert!(
            star.eccentricity >= 0.0 && star.eccentricity <= 1.0,
            "Eccentricity should be in [0, 1]"
        );
    }

    let avg_fwhm: f32 = stars.iter().map(|s| s.fwhm).sum::<f32>() / stars.len() as f32;
    let avg_snr: f32 = stars.iter().map(|s| s.snr).sum::<f32>() / stars.len() as f32;
    let avg_ecc: f32 = stars.iter().map(|s| s.eccentricity).sum::<f32>() / stars.len() as f32;

    println!("\nStatistics:");
    println!("  Average FWHM: {:.2} pixels", avg_fwhm);
    println!("  Average SNR: {:.1}", avg_snr);
    println!("  Average eccentricity: {:.3}", avg_ecc);
    println!("  Total stars: {}", stars.len());
}

// =============================================================================
// Background Estimation Tests
// =============================================================================

#[test]
#[ignore] // Requires LUMOS_CALIBRATION_DIR
fn test_background_on_real_image() {
    use crate::common::Buffer2;
    use crate::star_detection::BackgroundConfig;
    use imaginarium::ColorFormat;

    init_tracing();

    let Some(cal_dir) = calibration_dir() else {
        eprintln!("LUMOS_CALIBRATION_DIR not set, skipping test");
        return;
    };

    let image_path = cal_dir.join("calibrated_light_500x500_stretched.tiff");
    if !image_path.exists() {
        eprintln!("calibrated_light_500x500_stretched.tiff not found, skipping");
        return;
    }

    let imag_image = imaginarium::Image::read_file(&image_path)
        .expect("Failed to load image")
        .packed();
    let astro_image: AstroImage = imag_image.convert(ColorFormat::L_F32).unwrap().into();

    let width = astro_image.width();
    let height = astro_image.height();
    let pixels = astro_image.channel(0);

    println!("Loaded image: {}x{}", width, height);

    use crate::star_detection::BackgroundMap;

    let pixels_buf = Buffer2::new(width, height, pixels.to_vec());
    let config = BackgroundConfig {
        tile_size: 64,
        ..Default::default()
    };
    let bg = crate::testing::estimate_background(&pixels_buf, &config);

    let bg_img = to_gray_image(bg.background.pixels(), width, height);
    let path = common::test_utils::test_output_path("real_data/background_map.tiff");
    bg_img.save(&path).unwrap();
    println!("Saved background map: {:?}", path);

    let noise_min = bg.noise.iter().cloned().fold(f32::INFINITY, f32::min);
    let noise_max = bg.noise.iter().cloned().fold(f32::NEG_INFINITY, f32::max);
    let noise_range = (noise_max - noise_min).max(f32::EPSILON);
    let noise_scaled: Vec<f32> = bg
        .noise
        .iter()
        .map(|n| (n - noise_min) / noise_range)
        .collect();
    let noise_img = to_gray_image(&noise_scaled, width, height);
    let path = common::test_utils::test_output_path("real_data/background_noise.tiff");
    noise_img.save(&path).unwrap();
    println!("Saved noise map: {:?}", path);

    let subtracted: Vec<f32> = pixels
        .iter()
        .zip(bg.background.iter())
        .map(|(p, b)| ((*p - *b) + 0.5).clamp(0.0, 1.0))
        .collect();
    let sub_img = to_gray_image(&subtracted, width, height);
    let path = common::test_utils::test_output_path("real_data/background_subtracted.png");
    sub_img.save(&path).unwrap();
    println!("Saved subtracted image: {:?}", path);

    let bg_min = bg.background.iter().cloned().fold(f32::INFINITY, f32::min);
    let bg_max = bg
        .background
        .iter()
        .cloned()
        .fold(f32::NEG_INFINITY, f32::max);
    let bg_mean: f32 = bg.background.iter().sum::<f32>() / bg.background.len() as f32;
    println!(
        "Background stats: min={:.4}, max={:.4}, mean={:.4}",
        bg_min, bg_max, bg_mean
    );
}

fn to_gray_image(pixels: &[f32], width: usize, height: usize) -> image::GrayImage {
    image::GrayImage::from_fn(width as u32, height as u32, |x, y| {
        let val = pixels[y as usize * width + x as usize];
        image::Luma([(val.clamp(0.0, 1.0) * 255.0) as u8])
    })
}

// =============================================================================
// Stacking Tests
// =============================================================================

mod stacking_tests {
    use super::*;
    use crate::stacking::{
        FrameType, ImageStack, MedianConfig, ProgressCallback, SigmaClippedConfig, StackingMethod,
    };

    fn test_stack_from_env(
        subdir: &str,
        frame_type: FrameType,
        method: StackingMethod,
        output_name: &str,
    ) {
        init_tracing();

        let Some(paths) = calibration_image_paths(subdir) else {
            eprintln!(
                "LUMOS_CALIBRATION_DIR not set or {} dir missing, skipping test",
                subdir
            );
            return;
        };

        if paths.is_empty() {
            eprintln!("No files found in {} directory, skipping test", subdir);
            return;
        }

        println!(
            "Stacking {} {}s with {:?} method...",
            paths.len(),
            subdir.to_lowercase(),
            method
        );
        let stack = ImageStack::new(frame_type, method, paths.clone());
        let master = stack.process(ProgressCallback::default()).unwrap();

        let first = AstroImage::from_file(&paths[0]).unwrap();
        println!(
            "Master {}: {}x{}x{}",
            subdir.to_lowercase(),
            master.width(),
            master.height(),
            master.channels()
        );

        assert_eq!(master.dimensions(), first.dimensions());
        assert!(!master.channel(0).is_empty());

        let img: imaginarium::Image = master.into();
        img.save_file(common::test_utils::test_output_path(&format!(
            "real_data/{}",
            output_name
        )))
        .unwrap();
    }

    #[test]
    #[ignore] // Requires LUMOS_CALIBRATION_DIR
    fn test_stack_darks_mean() {
        test_stack_from_env(
            "Darks",
            FrameType::Dark,
            StackingMethod::Mean,
            "master_dark_mean.tiff",
        );
    }

    #[test]
    #[ignore] // Requires LUMOS_CALIBRATION_DIR
    fn test_stack_darks_median() {
        test_stack_from_env(
            "Darks",
            FrameType::Dark,
            StackingMethod::default(),
            "master_dark_median.tiff",
        );
    }

    #[test]
    #[ignore] // Requires LUMOS_CALIBRATION_DIR
    fn test_stack_darks_sigma_clipped() {
        test_stack_from_env(
            "Darks",
            FrameType::Dark,
            StackingMethod::SigmaClippedMean(SigmaClippedConfig::default()),
            "master_dark_sigma_clipped.tiff",
        );
    }

    #[test]
    #[ignore] // Requires LUMOS_CALIBRATION_DIR
    fn test_stack_darks_median_limited_ram() {
        init_tracing();

        let Some(paths) = calibration_image_paths("Darks") else {
            eprintln!("LUMOS_CALIBRATION_DIR not set or Darks dir missing, skipping test");
            return;
        };

        if paths.is_empty() {
            eprintln!("No files found in Darks directory, skipping test");
            return;
        }

        let config = MedianConfig {
            available_memory: Some(1),
            cache_dir: std::env::temp_dir().join("lumos_limited_ram_test"),
            ..Default::default()
        };

        println!(
            "Stacking {} darks with median method (simulated limited RAM)...",
            paths.len()
        );

        let stack = ImageStack::new(
            FrameType::Dark,
            StackingMethod::Median(config.clone()),
            paths.clone(),
        );
        let master = stack.process(ProgressCallback::default()).unwrap();

        let first = AstroImage::from_file(&paths[0]).unwrap();
        assert_eq!(master.dimensions(), first.dimensions());

        let img: imaginarium::Image = master.into();
        img.save_file(common::test_utils::test_output_path(
            "real_data/master_dark_median_limited_ram.tiff",
        ))
        .unwrap();
    }
}

// =============================================================================
// X-Trans Demosaic Tests
// =============================================================================

mod xtrans_tests {
    use super::*;
    use std::path::PathBuf;

    fn xtrans_test_file() -> Option<PathBuf> {
        let cal_dir = calibration_dir()?;
        let file_path = cal_dir.join("Lights").join("03_DSCF6799.RAF");
        if file_path.exists() {
            Some(file_path)
        } else {
            eprintln!("X-Trans test file not found: {}", file_path.display());
            None
        }
    }

    #[test]
    #[ignore] // Requires LUMOS_CALIBRATION_DIR with X-Trans RAF files
    fn test_load_xtrans_raf_file() {
        init_tracing();

        let Some(file_path) = xtrans_test_file() else {
            eprintln!("Skipping test: X-Trans test file not available");
            return;
        };

        let image = AstroImage::from_file(&file_path).expect("Failed to load X-Trans RAF file");

        assert!(image.dimensions().width > 0);
        assert!(image.dimensions().height > 0);
        assert_eq!(image.dimensions().channels, 3);

        let pixel_count_per_channel = image.width() * image.height();
        for c in 0..image.channels() {
            assert_eq!(image.channel(c).len(), pixel_count_per_channel);
        }

        for c in 0..image.channels() {
            for (i, &pixel) in image.channel(c).iter().enumerate() {
                assert!(
                    (0.0..=1.0).contains(&pixel),
                    "Channel {} pixel {} out of range: {}",
                    c,
                    i,
                    pixel
                );
                assert!(!pixel.is_nan(), "Channel {} pixel {} is NaN", c, i);
            }
        }

        tracing::info!(
            "Successfully loaded X-Trans image: {}x{} ({} channels)",
            image.dimensions().width,
            image.dimensions().height,
            image.dimensions().channels
        );
    }

    #[test]
    #[ignore] // Requires LUMOS_CALIBRATION_DIR with X-Trans RAF files
    fn test_xtrans_pixel_statistics() {
        init_tracing();

        let Some(file_path) = xtrans_test_file() else {
            return;
        };

        let image = AstroImage::from_file(&file_path).expect("Failed to load X-Trans RAF file");

        let channels = image.dimensions().channels;

        for ch in 0..channels {
            let channel_pixels = image.channel(ch);

            let sum: f64 = channel_pixels.iter().map(|&p| p as f64).sum();
            let mean = sum / channel_pixels.len() as f64;
            let min = channel_pixels.iter().copied().fold(f32::INFINITY, f32::min);
            let max = channel_pixels
                .iter()
                .copied()
                .fold(f32::NEG_INFINITY, f32::max);

            assert!((0.0..=1.0).contains(&mean));
            assert!(min >= 0.0);
            assert!(max <= 1.0);
            assert!(max > min);

            let channel_name = ["Red", "Green", "Blue"][ch];
            tracing::info!(
                "{} channel: min={:.4}, max={:.4}, mean={:.4}",
                channel_name,
                min,
                max,
                mean
            );
        }
    }
}
