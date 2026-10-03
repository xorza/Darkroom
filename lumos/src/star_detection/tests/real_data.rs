//! Test star detection on rho-opiuchi.jpg real image.
//!
//! Run with: `cargo test -p lumos --features real-data rho_opiuchi -- --nocapture`; set
//! `DARKROOM_TEST_OUTPUT` to keep the annotated images.

use common::internals;
use std::time::Instant;

use crate::ImageDimensions;
use crate::internals::init_tracing;
use crate::internals::real_data::{LightPair, dataset_path, first_and_last_lights};
use crate::io::image::linear::LinearImage;
use crate::math::size2us::Size2us;
use crate::registration::config::Config as RegistrationConfig;
use crate::registration::register;
use crate::registration::transform::TransformModel;
use crate::star_detection::config::Config;
use crate::star_detection::config::measurement_config::{CentroidMethod, NoiseModel};
use crate::star_detection::detector::StarDetector;
use crate::star_detection::threshold_mask::ThresholdParams;
use glam::Vec2;
use imaginarium::Color;
use imaginarium::ColorFormat;
use imaginarium::drawing::draw_circle;
use std::path::PathBuf;

fn rho_opiuchi_path() -> PathBuf {
    dataset_path("rho-opiuchi.jpg")
}

/// The dataset's `rho-opiuchi.jpg` as one luminance plane.
pub(crate) fn rho_opiuchi() -> LinearImage {
    let image = imaginarium::Image::read_file(rho_opiuchi_path())
        .expect("Failed to load image")
        .convert(ColorFormat::L_F32);
    LinearImage::from_pixels(
        ImageDimensions::new((image.desc().width, image.desc().height), 1),
        bytemuck::cast_slice(image.bytes()).to_vec(),
    )
}

#[test]
fn detect_rho_opiuchi() {
    init_tracing();

    let linear_image = rho_opiuchi();
    println!(
        "Image size: {}x{}",
        linear_image.width(),
        linear_image.height()
    );

    let mut detector = StarDetector::from_config(Config::precise_ground()).unwrap();

    let start = Instant::now();
    let result = detector.detect(&linear_image);
    let elapsed = start.elapsed();

    println!("Detection time: {elapsed:?}");
    println!("Stars found: {}", result.stars.len());

    if !result.stars.is_empty() {
        let avg_fwhm: f32 =
            result.stars.iter().map(|s| s.fwhm).sum::<f32>() / result.stars.len() as f32;
        let avg_snr: f32 =
            result.stars.iter().map(|s| s.snr).sum::<f32>() / result.stars.len() as f32;

        println!("\nStatistics:");
        println!("  Average FWHM: {avg_fwhm:.2} px");
        println!("  Average SNR: {avg_snr:.1}");

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
    let mut output_img = imaginarium::Image::read_file(rho_opiuchi_path())
        .expect("Failed to load image")
        .convert(ColorFormat::RGB_F32);

    // Draw circles around all detected stars
    for star in &result.stars {
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
    let output_img = output_img.convert(ColorFormat::RGB_U8);

    if let Some(output_path) = internals::debug_output_path("rho-opiuchi-detection.jpg") {
        output_img
            .save_file(&output_path)
            .expect("Failed to save output image");
        println!("\nSaved detection result to: {}", output_path.display());
    }

    // Invariants every detection owes, whatever the field: stars inside the frame with a finite
    // positive width and signal, brightest first.
    assert!(
        !result.stars.is_empty(),
        "Should find stars in rho-opiuchi.jpg"
    );
    let (width, height) = (linear_image.width() as f64, linear_image.height() as f64);
    for star in &result.stars {
        assert!(
            (0.0..width).contains(&star.pos.x) && (0.0..height).contains(&star.pos.y),
            "{star:?} lies outside the frame"
        );
        assert!(star.fwhm.is_finite() && star.fwhm > 0.0, "{star:?}");
        assert!(star.flux > 0.0 && star.snr > 0.0, "{star:?}");
    }
    assert!(
        result.stars.is_sorted_by(|a, b| a.flux >= b.flux),
        "stars are brightest first"
    );
}

#[test]
fn inspect_pipeline_intermediates_rho_opiuchi() {
    use crate::internals::visual;
    use crate::star_detection::background::background_estimate::BackgroundEstimate;
    use crate::star_detection::convolution::{MatchedFilterBuffers, matched_filter};
    use crate::star_detection::detector::stages::detect::DetectResult;
    use crate::star_detection::detector::stages::fwhm;
    use crate::star_detection::detector::stages::prepare;
    use crate::star_detection::labeling::LabelMap;
    use crate::star_detection::resources::DetectionResources;
    use crate::star_detection::threshold_mask::create_residual_threshold_mask;
    use imaginarium::Buffer2;

    init_tracing();

    let linear_image = rho_opiuchi();
    let width = linear_image.width();
    let height = linear_image.height();
    println!("Image size: {width}x{height}");

    let config = Config::precise_ground();
    let mut pool = DetectionResources::new(Size2us::new(width, height));

    let out = |name: &str| format!("rho-opiuchi-inspect/{name}");

    // 1. Grayscale
    let grayscale = prepare::prepare(&linear_image, &mut pool);
    visual::save(
        grayscale.pixels(),
        Size2us::new(width, height),
        &out("01_grayscale"),
        visual::ToneMap::AutoRange,
    );
    println!("Saved: 01_grayscale");

    // 2. Background
    let background = BackgroundEstimate::estimate(&grayscale, &config.background, &mut pool);
    visual::save(
        background.background.pixels(),
        Size2us::new(width, height),
        &out("02_background"),
        visual::ToneMap::AutoRange,
    );
    println!("Saved: 02_background");

    // 3. Noise
    visual::save(
        background.noise.pixels(),
        Size2us::new(width, height),
        &out("03_noise"),
        visual::ToneMap::AutoRange,
    );
    println!("Saved: 03_noise");

    // 4. Background-subtracted image
    let subtracted: Vec<f32> = grayscale
        .pixels()
        .iter()
        .zip(background.background.pixels().iter())
        .map(|(&p, &bg)| (p - bg).max(0.0))
        .collect();
    visual::save(
        &subtracted,
        Size2us::new(width, height),
        &out("04_subtracted"),
        visual::ToneMap::AutoRange,
    );
    println!("Saved: 04_subtracted");

    // 5. FWHM estimation, on the residual with nothing marked saturated
    let residual = background.residual_of(&grayscale);
    let sky = background.sky_noise();
    let mut saturation = pool.acquire_bit();
    saturation.fill(false);
    let fwhm_source = fwhm::estimate(&residual, &sky, &saturation, &config, &mut pool);
    pool.release_bit(saturation);
    let fwhm = fwhm_source.value();
    println!("Estimated FWHM: {fwhm:?} ({fwhm_source:?})");

    // 6. Matched filter (if FWHM available)
    let filtered: Option<Buffer2<f32>> = fwhm.map(|fwhm_val| {
        let mut output = pool.acquire_f32();
        let mut temp = pool.acquire_f32();
        matched_filter(
            &residual,
            fwhm_val,
            config.detection.psf_axis_ratio,
            config.detection.psf_angle,
            &mut MatchedFilterBuffers {
                output: &mut output,
                temp: &mut temp,
            },
        );
        pool.release_f32(temp);
        visual::save(
            output.pixels(),
            Size2us::new(width, height),
            &out("05_matched_filter"),
            visual::ToneMap::AutoRange,
        );
        println!("Saved: 05_matched_filter");
        output
    });
    if filtered.is_none() {
        println!("No FWHM — matched filter skipped");
    }

    // 7. Threshold mask
    let mut mask = pool.acquire_bit();
    mask.fill(false);
    let threshold = ThresholdParams {
        sigma: config.detection.sigma_threshold,
        min_noise: sky.floor,
    };
    create_residual_threshold_mask(
        filtered.as_ref().unwrap_or(&residual),
        &sky.noise,
        threshold,
        &mut mask,
    );
    if let Some(filtered) = filtered {
        pool.release_f32(filtered);
    }
    let pixels_above = mask.count_ones();
    visual::save_mask(&mask, &out("06_threshold_mask"));
    println!("Saved: 06_threshold_mask ({pixels_above} pixels above threshold)");

    // 8. Label map, of the mask as it stands — the stage labels it undilated
    let label_map = LabelMap::from_pool(&mask, config.detection.connectivity, &mut pool);
    let num_labels = label_map.num_labels();
    let labels_buf = Buffer2::new(width, height, label_map.labels().to_vec());
    let labels_rgb = visual::labels_to_rgb(&labels_buf);
    visual::save_rgb(&labels_rgb, &out("07_label_map"));
    println!("Saved: 07_label_map ({num_labels} components)");
    label_map.release_to_pool(&mut pool);
    pool.release_bit(mask);

    // The stage itself, on the same residual and FWHM, saw what these images show.
    let stage = DetectResult::from_image(&residual, &sky, fwhm, &config.detection, &mut pool);
    assert_eq!(stage.pixels_above_threshold, pixels_above);
    assert_eq!(stage.connected_components, num_labels);
    background.release_to_pool(&mut pool);
    pool.release_f32(grayscale);
}

/// PR1 validation: inverse-variance-weighted PSF fitting should not worsen (and ideally
/// improves) registration RMS vs unweighted, by producing lower-variance sub-pixel
/// centroids. Runs the pair of lights through `GaussianFit` with and without a
/// `NoiseModel`, registers each, and compares.
#[test]
fn weighted_fit_registration_rms() {
    /// What one registration of the pair reported.
    #[derive(Debug)]
    struct Registered {
        rms: f64,
        inliers: usize,
    }

    let LightPair {
        first: img1,
        last: img2,
    } = first_and_last_lights();

    // 30,000 e-/normalized unit is representative of physical gain × the 14-bit signal range.
    let noise_model = NoiseModel::from_normalized(30_000.0, 30.0);

    let register_with = |noise: Option<NoiseModel>| {
        let mut config = Config::precise_ground();
        config.measurement.centroid_method = CentroidMethod::GaussianFit;
        config.measurement.noise_model = noise;
        let mut detector = StarDetector::from_config(config).unwrap();
        let s1 = detector.detect(&img1).stars;
        let s2 = detector.detect(&img2).stars;
        let mut reg_config = RegistrationConfig {
            transform_type: TransformModel::Auto,
            sip: None,
            ..RegistrationConfig::default()
        };
        // Seeded, so the two runs differ only in their centroids.
        reg_config.ransac.seed = Some(0x5EED);
        let r = register(&s1, &s2, &reg_config).expect("registration should succeed");
        Registered {
            rms: r.rms_error(),
            inliers: r.num_inliers(),
        }
    };

    let Registered {
        rms: unweighted_rms,
        inliers: unweighted_n,
    } = register_with(None);
    let Registered {
        rms: weighted_rms,
        inliers: weighted_n,
    } = register_with(Some(noise_model));

    println!("PR1 weighted-fit registration:");
    println!("  unweighted: RMS {unweighted_rms:.4} px, {unweighted_n} matches");
    println!("  weighted:   RMS {weighted_rms:.4} px, {weighted_n} matches");

    // Weighting must not meaningfully worsen registration. The two catalogs differ, so RANSAC keeps
    // different inlier sets; 5% is the margin "not meaningfully worse" allows on that.
    assert!(
        weighted_rms <= unweighted_rms * 1.05,
        "weighted RMS {weighted_rms:.4} should be ≤ unweighted {unweighted_rms:.4} ×1.05"
    );
}
