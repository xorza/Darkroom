//! Test star detection on rho-opiuchi.jpg real image.
//!
//! Run with: `cargo test -p lumos --features real-data rho_opiuchi -- --ignored --nocapture`

use crate::star_detection::StarDetector;
use crate::star_detection::config::Config;
use crate::testing::{calibration_dir, init_tracing};
use crate::{AstroImage, CentroidMethod};
use common::test_utils::test_output_path;
use glam::Vec2;
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

    let mut detector = StarDetector::from_config(Config::precise_ground());

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
                star.pos.x, star.pos.y, star.flux, star.fwhm, star.snr
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
        draw_circle(
            &mut output_img,
            Vec2::new(star.pos.x as f32, star.pos.y as f32),
            radius,
            Color::GREEN,
            1.0,
        );
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

#[test]
#[ignore] // Requires LUMOS_CALIBRATION_DIR
fn test_inspect_pipeline_intermediates_rho_opiuchi() {
    use super::common::output::image_writer;
    use crate::common::Buffer2;
    use crate::star_detection::BufferPool;
    use crate::star_detection::background::estimate_background;
    use crate::star_detection::convolution::matched_filter;
    use crate::star_detection::detector::stages::fwhm::estimate_fwhm;
    use crate::star_detection::detector::stages::prepare;
    use crate::star_detection::labeling::LabelMap;
    use crate::star_detection::mask_dilation::dilate_mask;
    use crate::star_detection::threshold_mask::create_threshold_mask_filtered;

    init_tracing();

    let Some(cal_dir) = calibration_dir() else {
        eprintln!("LUMOS_CALIBRATION_DIR not set, skipping test");
        return;
    };

    let image_path = cal_dir.join("rho-opiuchi.jpg");
    if !image_path.exists() {
        panic!("rho-opiuchi.jpg not found in {:?}", cal_dir);
    }

    let img = imaginarium::Image::read_file(&image_path)
        .expect("Failed to load image")
        .packed()
        .convert(ColorFormat::L_F32)
        .expect("Failed to convert to grayscale");

    let astro_image: AstroImage = img.into();
    let width = astro_image.width();
    let height = astro_image.height();
    println!("Image size: {width}x{height}");

    let config = Config::precise_ground();
    let mut pool = BufferPool::new(width, height);

    let out = |name: &str| test_output_path(&format!("rho-opiuchi-inspect/{name}"));

    // 1. Grayscale
    let grayscale = prepare::prepare(&astro_image, &mut pool);
    image_writer::save_grayscale_stretched(
        grayscale.pixels(),
        width,
        height,
        &out("01_grayscale.tiff"),
    );
    println!("Saved: 01_grayscale");

    // 2. Background
    let background = estimate_background(&grayscale, &config, &mut pool);
    image_writer::save_grayscale_stretched(
        background.background.pixels(),
        width,
        height,
        &out("02_background.tiff"),
    );
    println!("Saved: 02_background");

    // 3. Noise
    image_writer::save_grayscale_stretched(
        background.noise.pixels(),
        width,
        height,
        &out("03_noise.tiff"),
    );
    println!("Saved: 03_noise");

    // 4. Background-subtracted image
    let subtracted: Vec<f32> = grayscale
        .pixels()
        .iter()
        .zip(background.background.pixels().iter())
        .map(|(&p, &bg)| (p - bg).max(0.0))
        .collect();
    image_writer::save_grayscale_stretched(&subtracted, width, height, &out("04_subtracted.tiff"));
    println!("Saved: 04_subtracted");

    // 5. FWHM estimation
    let fwhm_result = estimate_fwhm(&grayscale, &background, &config, &mut pool);
    let fwhm = fwhm_result.fwhm;
    println!(
        "Estimated FWHM: {:?} (from {} stars)",
        fwhm, fwhm_result.stars_used
    );

    // 6. Matched filter (if FWHM available)
    let filtered_pixels: Option<Vec<f32>> = if let Some(fwhm_val) = fwhm {
        let mut scratch = pool.acquire_f32();
        let mut conv_scratch = pool.acquire_f32();
        let mut conv_temp = pool.acquire_f32();
        matched_filter(
            &grayscale,
            &background.background,
            fwhm_val,
            config.psf_axis_ratio,
            config.psf_angle,
            &mut scratch,
            &mut conv_scratch,
            &mut conv_temp,
        );
        pool.release_f32(conv_temp);
        pool.release_f32(conv_scratch);

        let pixels = scratch.pixels().to_vec();
        image_writer::save_grayscale_stretched(
            &pixels,
            width,
            height,
            &out("05_matched_filter.tiff"),
        );
        println!("Saved: 05_matched_filter");

        pool.release_f32(scratch);
        Some(pixels)
    } else {
        println!("No FWHM — matched filter skipped");
        None
    };

    // 7. Threshold mask
    let mut mask = pool.acquire_bit();
    mask.fill(false);
    if let Some(ref filtered) = filtered_pixels {
        let filtered_buf = Buffer2::new(width, height, filtered.clone());
        create_threshold_mask_filtered(
            &filtered_buf,
            &background.noise,
            config.sigma_threshold,
            &mut mask,
        );
    } else {
        crate::star_detection::threshold_mask::create_threshold_mask(
            &grayscale,
            &background.background,
            &background.noise,
            config.sigma_threshold,
            &mut mask,
        );
    }
    let mask_bools: Vec<bool> = mask.iter().collect();
    let pixels_above = mask_bools.iter().filter(|&&b| b).count();
    image_writer::save_mask(&mask_bools, width, height, &out("06_threshold_mask.tiff"));
    println!("Saved: 06_threshold_mask ({pixels_above} pixels above threshold)");

    // 8. Dilated mask
    let mut dilated = pool.acquire_bit();
    dilated.fill(false);
    dilate_mask(&mask, 1, &mut dilated);
    let dilated_bools: Vec<bool> = dilated.iter().collect();
    let dilated_count = dilated_bools.iter().filter(|&&b| b).count();
    image_writer::save_mask(&dilated_bools, width, height, &out("07_dilated_mask.tiff"));
    println!("Saved: 07_dilated_mask ({dilated_count} pixels)");

    // 9. Label map
    let label_map = LabelMap::from_pool(&dilated, config.connectivity, &mut pool);
    let num_labels = label_map.num_labels();
    let labels_buf = Buffer2::new(width, height, label_map.labels().to_vec());
    let labels_rgb = image_writer::labels_to_rgb(&labels_buf);
    image_writer::save_rgb(&labels_rgb, &out("08_label_map.tiff"));
    println!("Saved: 08_label_map ({num_labels} components)");

    // Clean up
    label_map.release_to_pool(&mut pool);
    pool.release_bit(dilated);
    pool.release_bit(mask);
    background.release_to_pool(&mut pool);
    pool.release_f32(grayscale);

    println!("\nAll intermediate images saved to test_output/rho-opiuchi-inspect/");
}

#[bench::quick_bench(warmup_iters = 1, iters = 10)]
fn quick_bench_detect_rho_opiuchi(b: bench::Bencher) {
    let Some(cal_dir) = calibration_dir() else {
        eprintln!("LUMOS_CALIBRATION_DIR not set, skipping bench");
        return;
    };

    let image_path = cal_dir.join("rho-opiuchi.jpg");
    if !image_path.exists() {
        panic!("rho-opiuchi.jpg not found in {:?}", cal_dir);
    }

    // Preload image outside of benchmark loop
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
    let mut config = Config::precise_ground();
    config.centroid_method = CentroidMethod::MoffatFit { beta: 2.5 };
    let mut detector = StarDetector::from_config(config);

    b.bench(|| detector.detect(&astro_image));
}
